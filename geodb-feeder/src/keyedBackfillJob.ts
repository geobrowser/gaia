/**
 * The one-shot backfill Job every keyed-record family shares (votes-backfill-main.ts,
 * proposal-votes-backfill-main.ts): same image as the feeder, run with the feeder scaled to 0. It writes every row of
 * one gaia Postgres table into the family's system Space through the path its consumer uses.
 *
 * Rerunnable: every row is written to the same ids, so a rerun after a crash converges. It never touches the
 * consumer's offsets except through the gate (keyedBackfillGate on the family's offsets directory).
 *
 * Env, beyond the feeder's own (Kafka, geodb-client, DATA_DIR):
 * - DATABASE_URL, DATABASE_SSL_CA: gaia Postgres, read only (every query runs in a READ ONLY transaction).
 * - BACKFILL_KEYED_RESET_CONSUMER=1 lets the backfill run after the family's consumer has: it deletes that consumer's
 *   offsets so it replays the retained topic from earliest when it next starts.
 * - BACKFILL_MAX_ROWS_PER_S caps the write rate in rows a second (unset or 0: unpaced). The tables behind these Jobs are small;
 *   the cap keeps a rerun from adding to geodb-0's load while serve is under read pressure.
 * - DRY_RUN=1 only counts: it needs DATABASE_URL (and DATABASE_SSL_CA) and nothing else, touches neither Kafka nor geo-db
 *   nor the state volume, prints one line of row counts per table and exits 0. A Job that gives no `count` refuses it.
 * - BACKFILL_SKIP_GROUP_CHECK=1 skips the check that the feeder's groups are empty. Only for a store no feeder will resume.
 */
import kafkajs from "kafkajs"
import {type Config, feederGroupIds, loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import type {KeyedBackfillResult, KeyedRecordDeps} from "./keyedRecord.js"
import {errorFields, log} from "./log.js"
import {keyedBackfillGate} from "./membershipKeyed.js"
import {createMetrics} from "./metrics.js"
import {ReadOnlyPg, pgConfig} from "./pgSource.js"
import {ProposalOffsets} from "./proposals.js"
import {SpaceMapPublisher, kubeConfigMapPut} from "./publish.js"
import {startServer} from "./server.js"
import {readSealer} from "./spaces.js"
import {Store} from "./state.js"
import {SYSTEM_SPACE_MAP_KEY, SystemSpaces} from "./systemSpaces.js"

const {Kafka, logLevel} = kafkajs
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

export type KeyedBackfillJob = {
	/** Log prefix and Kafka client id suffix, e.g. "votes". */
	name: string
	/** Why the Job refuses to run, or null when its flags are set. */
	refuse: (config: Config) => string | null
	offsetsDir: (dataDir: string) => string
	/** Row counts per table, read only; what DRY_RUN=1 prints. Absent: the Job has no dry run and DRY_RUN=1 refuses. */
	count?: (pg: ReadOnlyPg) => Promise<Record<string, number>>
	run: (
		deps: KeyedRecordDeps,
		pg: ReadOnlyPg,
		progress: (done: number) => void,
	) => Promise<KeyedBackfillResult>
}

/** A pacer that holds a write rate of `rate` rows a second: it sleeps to the next slot, never ahead of it. Rate <= 0: none. */
export function makePacer(rate: number, now: () => number = Date.now, wait: (ms: number) => Promise<void> = sleep): (() => Promise<void>) | undefined {
	if (!Number.isFinite(rate) || rate <= 0) return undefined
	const gap = 1000 / rate
	let next = now()
	return async () => {
		next += gap
		const d = next - now()
		if (d > 0) await wait(d)
		else next = now()
	}
}

/** DRY_RUN=1: count the source rows and stop. Postgres only, in READ ONLY transactions. */
async function dryRun(job: KeyedBackfillJob, env: NodeJS.ProcessEnv): Promise<void> {
	const databaseUrl = env.DATABASE_URL
	if (!databaseUrl) throw new Error("DATABASE_URL is required")
	if (!job.count) throw new Error(`${job.name} has no dry run`)
	const pg = new ReadOnlyPg(pgConfig(databaseUrl, env.DATABASE_SSL_CA))
	try {
		const counts = await job.count(pg)
		log("info", `${job.name} backfill dry run: nothing written`, {...counts, total: Object.values(counts).reduce((a, b) => a + b, 0)})
	} finally {
		await pg.close()
	}
}

async function runJob(job: KeyedBackfillJob): Promise<void> {
	const env = process.env
	if (env.DRY_RUN === "1") return dryRun(job, env)
	const config = loadConfig(env)
	const refused = job.refuse(config)
	if (refused) throw new Error(refused)
	const databaseUrl = env.DATABASE_URL
	if (!databaseUrl) throw new Error("DATABASE_URL is required")

	const metrics = createMetrics()
	const abort = new AbortController()
	const server = startServer(config.metricsPort, metrics, {ready: () => true})
	const store = new Store(`${config.dataDir}/feeder`)

	// The system-space record lives on the feeder's volume: refuse while any feeder consumer group has members.
	const kafka = new Kafka({
		clientId: `geodb-${job.name.replaceAll("_", "-")}-backfill`,
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
	const gate = keyedBackfillGate(job.offsetsDir(config.dataDir), env.BACKFILL_KEYED_RESET_CONSUMER === "1")
	log(gate === "reset" ? "warn" : "info", `${job.name} backfill gate`, {gate})

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
		const result = await job.run(
			{
				db,
				store,
				metrics,
				beat: async () => {},
				signal: abort.signal,
				sealer: () => readSealer(config.sealerFile),
				allowPut: true,
				pace: makePacer(Number(env.BACKFILL_MAX_ROWS_PER_S ?? 0)),
				systemSpaces,
				publishSystem: (t) => systemPublisher.publish(t),
				// The live consumer's offsets; the backfill never moves them (only the gate may delete them).
				offsets: new ProposalOffsets(job.offsetsDir(config.dataDir)),
			},
			pg,
			(done) => {
				if (done % 5000 === 0) log("info", `${job.name} backfill progress`, {done})
			},
		)
		log(result.deadLetter ? "error" : "info", `${job.name} backfill done`, {...result, seconds: Math.round((Date.now() - t0) / 1000)})
		if (result.deadLetter > 0) throw new Error(`${result.deadLetter} rows were refused (see errors.jsonl); fix and rerun`)
	} catch (e) {
		failure = e
	}
	// The family's Space id must reach the ConfigMap before exit, or no reader can find it.
	systemPublisher.publish(systemSpaces.render())
	for (let i = 0; i < 300 && systemPublisher.pending; i++) await sleep(1000)
	if (systemPublisher.pending) log("error", "system space map not yet published; the feeder republishes it on start")
	systemPublisher.stop()
	await pg.close()
	await admin.disconnect().catch(() => {})
	server.close()
	if (failure) throw failure
}

/** Run the Job and exit: 0 on success, 1 on any failure (refused rows included). */
export function mainKeyedBackfill(job: KeyedBackfillJob): void {
	runJob(job).then(
		() => process.exit(0),
		(e) => {
			log("error", `${job.name} backfill failed`, errorFields(e))
			process.exit(1)
		},
	)
}
