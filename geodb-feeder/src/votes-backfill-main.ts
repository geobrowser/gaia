/**
 * geodb-votes-backfill: a one-shot Job, same image as the feeder, run with the feeder scaled to 0. It writes every
 * `user_votes` row of gaia Postgres into the `votes` system Space as a keyed record (backfillUserVotes,
 * votesKeyed.ts), the path the votes consumer uses. Run it before GEODB_VOTES goes on; see geodb/RUNBOOK.md
 * §2c-ter. Required, not optional: `curation.votes` keeps 7 days on testnet, so the stream alone cannot rebuild
 * older votes.
 *
 * Rerunnable: every row is written to the same ids, so a rerun after a crash converges. It never touches the
 * consumer's offsets except through the gate (keyedBackfillGate on the votes offsets directory).
 *
 * Env, beyond the feeder's own (Kafka, geodb-client, DATA_DIR):
 * - GEODB_RECORD_KEYS=1 and GEODB_VOTES=1: both required (the Job refuses otherwise), as for the consumer. Needs serve
 *   at geo-db 9d97069a78 or later (`put`).
 * - DATABASE_URL, DATABASE_SSL_CA: gaia Postgres, read only (every query runs in a READ ONLY transaction).
 * - BACKFILL_KEYED_RESET_CONSUMER=1 lets the backfill run after the votes consumer has: it deletes the votes
 *   offsets so the consumer replays the retained topic from earliest when it next starts.
 * - BACKFILL_SKIP_GROUP_CHECK=1 skips the check that the feeder's groups are empty. Only for a store no feeder will resume.
 */
import kafkajs from "kafkajs"
import {loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {errorFields, log} from "./log.js"
import {keyedBackfillGate} from "./membershipKeyed.js"
import {createMetrics} from "./metrics.js"
import {ReadOnlyPg, pgConfig, readUserVoteRows} from "./pgSource.js"
import {ProposalOffsets} from "./proposals.js"
import {SpaceMapPublisher, kubeConfigMapPut} from "./publish.js"
import {startServer} from "./server.js"
import {readSealer} from "./spaces.js"
import {Store} from "./state.js"
import {SYSTEM_SPACE_MAP_KEY, SystemSpaces} from "./systemSpaces.js"
import {backfillUserVotes, votesKeyedEnabled, votesOffsetsDir} from "./votesKeyed.js"

const {Kafka, logLevel} = kafkajs
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

async function main(): Promise<void> {
	const env = process.env
	const config = loadConfig(env)
	if (!votesKeyedEnabled(config)) throw new Error("GEODB_RECORD_KEYS=1 and GEODB_VOTES=1 are both required")
	const databaseUrl = env.DATABASE_URL
	if (!databaseUrl) throw new Error("DATABASE_URL is required")

	const metrics = createMetrics()
	const abort = new AbortController()
	const server = startServer(config.metricsPort, metrics, {ready: () => true})
	const store = new Store(`${config.dataDir}/feeder`)

	// The system-space record lives on the feeder's volume: refuse while any feeder consumer group has members.
	const kafka = new Kafka({
		clientId: "geodb-votes-backfill",
		brokers: [config.kafka.broker],
		logLevel: logLevel.WARN,
		ssl: {ca: [config.kafka.caPem]},
		sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
	})
	const admin = kafka.admin()
	await admin.connect()
	if (env.BACKFILL_SKIP_GROUP_CHECK !== "1") {
		const groups = [config.kafka.groupId, config.membership.groupId, config.membership.keyedGroupId, config.votes.groupId]
		for (const g of (await admin.describeGroups(groups)).groups) {
			if (g.members.length > 0) throw new Error(`group ${g.groupId} has ${g.members.length} members: scale the feeder to 0 first`)
		}
	}
	const gate = keyedBackfillGate(votesOffsetsDir(config.dataDir), env.BACKFILL_KEYED_RESET_CONSUMER === "1")
	log(gate === "reset" ? "warn" : "info", "votes backfill gate", {gate})

	const pg = new ReadOnlyPg(pgConfig(databaseUrl, env.DATABASE_SSL_CA))
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	const systemSpaces = new SystemSpaces(`${config.dataDir}/feeder`)
	const systemPublisher = new SpaceMapPublisher(
		config.systemSpaceMapPath,
		kubeConfigMapPut(config.spaceMapConfigMap),
		metrics,
		SYSTEM_SPACE_MAP_KEY,
	)
	systemPublisher.start()
	const stop = () => {
		log("info", "shutting down: a rerun writes the same ids again")
		abort.abort()
	}
	process.on("SIGTERM", stop)
	process.on("SIGINT", stop)

	log("info", "waiting for geodb-serve")
	await db.waitReady()
	const t0 = Date.now()
	let failure: unknown = null
	try {
		const result = await backfillUserVotes(
			{
				db,
				store,
				metrics,
				beat: async () => {},
				signal: abort.signal,
				sealer: () => readSealer(config.sealerFile),
				allowPut: true,
				systemSpaces,
				publishSystem: (t) => systemPublisher.publish(t),
				// The live consumer's offsets; the backfill never moves them (only the gate may delete them).
				offsets: new ProposalOffsets(votesOffsetsDir(config.dataDir)),
			},
			readUserVoteRows(pg),
			(done) => {
				if (done % 5000 === 0) log("info", "votes backfill progress", {done})
			},
		)
		log(result.deadLetter ? "error" : "info", "votes backfill done", {...result, seconds: Math.round((Date.now() - t0) / 1000)})
		if (result.deadLetter > 0) throw new Error(`${result.deadLetter} rows were refused (see errors.jsonl); fix and rerun`)
	} catch (e) {
		failure = e
	}
	// The votes Space id must reach the ConfigMap before exit, or no reader can find it.
	systemPublisher.publish(systemSpaces.render())
	for (let i = 0; i < 300 && systemPublisher.pending; i++) await sleep(1000)
	if (systemPublisher.pending) log("error", "system space map not yet published; the feeder republishes it on start")
	systemPublisher.stop()
	await pg.close()
	await admin.disconnect().catch(() => {})
	server.close()
	if (failure) throw failure
}

main().then(
	() => process.exit(0),
	(e) => {
		log("error", "votes backfill failed", errorFields(e))
		process.exit(1)
	},
)
