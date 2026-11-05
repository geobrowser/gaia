/**
 * geodb-membership-backfill: a one-shot Job, same image as the feeder, run with the feeder scaled to 0. It writes
 * every current member and editor row of gaia Postgres into geo-db through backfillMembership (membership.ts),
 * the path the dual write uses, and records it in the same membership journal. Run it before turning on
 * GEODB_MEMBERSHIP_DUAL_WRITE; see geodb/RUNBOOK.md §2c.
 *
 * Idempotent: rows the journal already holds are no-ops, so a rerun after a crash resumes. It adds and
 * re-activates only, and never touches the consumer's offsets.
 *
 * Env, beyond the feeder's own (Kafka, geodb-client, DATA_DIR):
 * - DATABASE_URL, DATABASE_SSL_CA: gaia Postgres, read only (every query runs in a READ ONLY transaction).
 * - GEODB_RECORD_KEYS=1 writes the keyed records instead (backfillMembershipKeyed, membershipKeyed.ts): every row
 *   into the `governance` system Space under its natural-key id, no journal. Needs serve at geo-db 9d97069a78 or later.
 *   Required before GEODB_RECORD_KEYS goes on: the membership topic's retention is finite (7 days on testnet), so
 *   the stream alone cannot rebuild older rows. Refuses once the keyed consumer has offsets (keyedBackfillGate).
 * - BACKFILL_KEYED_RESET_CONSUMER=1 lets the keyed backfill run after the keyed consumer has: it deletes the keyed
 *   offsets so the consumer replays the retained topic from earliest when it next starts.
 * - BACKFILL_SKIP_GROUP_CHECK=1 skips the check that the feeder's groups are empty. Only for a store no feeder will resume.
 */
import kafkajs from "kafkajs"
import {feederGroupIds, loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {errorFields, log} from "./log.js"
import {MembershipState, backfillMembership} from "./membership.js"
import {backfillMembershipKeyed, keyedBackfillGate, keyedOffsetsDir} from "./membershipKeyed.js"
import {ProposalOffsets} from "./proposals.js"
import {createMetrics} from "./metrics.js"
import {ReadOnlyPg, pgConfig, readMembershipRows} from "./pgSource.js"
import {SpaceMapPublisher, kubeConfigMapPut} from "./publish.js"
import {startServer} from "./server.js"
import {readSealer} from "./spaces.js"
import {SYSTEM_SPACE_MAP_KEY, SystemSpaces} from "./systemSpaces.js"
import {Store, backfilledSpaces, renderSpaceMap} from "./state.js"

const {Kafka, logLevel} = kafkajs
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

async function main(): Promise<void> {
	const env = process.env
	const config = loadConfig(env)
	const databaseUrl = env.DATABASE_URL
	if (!databaseUrl) throw new Error("DATABASE_URL is required")

	const metrics = createMetrics()
	const abort = new AbortController()
	const server = startServer(config.metricsPort, metrics, {ready: () => true})
	const store = new Store(`${config.dataDir}/feeder`)
	const state = new MembershipState(`${config.dataDir}/feeder/membership`)
	log("info", "membership backfill starting", {statements: state.statementCount, spaces: state.spaces().length})

	// The journal and the space map live on the feeder's volume: refuse while either consumer group has members.
	const kafka = new Kafka({
		clientId: "geodb-membership-backfill",
		brokers: [config.kafka.broker],
		logLevel: logLevel.WARN,
		ssl: {ca: [config.kafka.caPem]},
		sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
	})
	const admin = kafka.admin()
	await admin.connect()
	if (env.BACKFILL_SKIP_GROUP_CHECK !== "1") {
		const groups = feederGroupIds(config)
		for (const g of (await admin.describeGroups(groups)).groups) {
			if (g.members.length > 0) throw new Error(`group ${g.groupId} has ${g.members.length} members: scale the feeder to 0 first`)
		}
	}

	if (config.recordKeys) {
		const gate = keyedBackfillGate(keyedOffsetsDir(config.dataDir), env.BACKFILL_KEYED_RESET_CONSUMER === "1")
		log(gate === "reset" ? "warn" : "info", "keyed backfill gate", {gate})
	}

	const pg = new ReadOnlyPg(pgConfig(databaseUrl, env.DATABASE_SSL_CA))
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	const publisher = new SpaceMapPublisher(config.spaceMapPath, kubeConfigMapPut(config.spaceMapConfigMap), metrics)
	publisher.start()
	const systemSpaces = new SystemSpaces(`${config.dataDir}/feeder`)
	const systemPublisher = new SpaceMapPublisher(
		config.systemSpaceMapPath,
		kubeConfigMapPut(config.spaceMapConfigMap),
		metrics,
		SYSTEM_SPACE_MAP_KEY,
	)
	systemPublisher.start()
	const stop = () => {
		log("info", "shutting down: a rerun resumes from the journal")
		abort.abort()
	}
	process.on("SIGTERM", stop)
	process.on("SIGINT", stop)

	log("info", "waiting for geodb-serve")
	await db.waitReady()
	const t0 = Date.now()
	let failure: unknown = null
	try {
		const rows = await readMembershipRows(pg)
		log("info", "membership rows read", {rows: rows.length})
		const progress = (done: number) => {
			if (done % 1000 === 0) log("info", "membership backfill progress", {done, of: rows.length, keyed: config.recordKeys})
		}
		const base = {db, store, metrics, beat: async () => {}, signal: abort.signal, sealer: () => readSealer(config.sealerFile)}
		const result = config.recordKeys
			? await backfillMembershipKeyed(
					{
						...base,
						allowPut: true,
						systemSpaces,
						publishSystem: (t) => systemPublisher.publish(t),
						// Offsets are the live keyed consumer's; the backfill never moves them (only the gate may delete them).
						offsets: new ProposalOffsets(keyedOffsetsDir(config.dataDir)),
					},
					rows,
					progress,
				)
			: await backfillMembership({...base, state, publish: (t) => publisher.publish(t)}, rows, progress)
		log(result.deadLetter ? "error" : "info", "membership backfill done", {
			...result,
			seconds: Math.round((Date.now() - t0) / 1000),
			...(config.recordKeys ? {} : {statementsKnown: state.statementCount}),
		})
		if (result.deadLetter > 0) throw new Error(`${result.deadLetter} rows were refused (see errors.jsonl); fix and rerun`)
	} catch (e) {
		failure = e
	}
	// The space map must reach the ConfigMap before exit, or the api cannot map a Space just founded.
	publisher.publish(renderSpaceMap(store.spaces, backfilledSpaces(store.dir)))
	if (config.recordKeys) systemPublisher.publish(systemSpaces.render())
	for (let i = 0; i < 300 && (publisher.pending || systemPublisher.pending); i++) await sleep(1000)
	if (publisher.pending || systemPublisher.pending) log("error", "space map not yet published; the feeder republishes it on start")
	publisher.stop()
	systemPublisher.stop()
	await pg.close()
	await admin.disconnect().catch(() => {})
	server.close()
	if (failure) throw failure
}

main().then(
	() => process.exit(0),
	(e) => {
		log("error", "membership backfill failed", errorFields(e))
		process.exit(1)
	},
)
