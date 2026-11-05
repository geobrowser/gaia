/**
 * geodb-times-backfill: a one-shot Job, same image as the feeder, run with the feeder scaled to 0. It stamps each
 * entity geo-db holds with createdAt / updatedAt from gaia Postgres (`entities`, read only), writing only what
 * geo-db lacks. See timesBackfill.ts for why that source, and geodb/RUNBOOK.md §2e for how to run it. Run it when
 * GEODB_ENTITY_TIMES is turned on (or just before).
 *
 * Idempotent: a rerun reads each entity back and writes only the missing cell; a cursor on the state volume resumes.
 *
 * Env, beyond the feeder's own (Kafka, geodb-client, DATA_DIR):
 * - DATABASE_URL, DATABASE_SSL_CA: gaia Postgres, read only (every query runs in a READ ONLY transaction).
 * - GEODB_READER_KEY_PATH: the key the read-back uses (the api's reader key).
 * - TIMES_SPACES: comma list of gaia space ids; unset or "mapped" is every Space geo-db has.
 * - TIMES_BATCH (100 entities), TIMES_PAUSE_MS (100 after each apply), TIMES_MAX_ENTITIES (0: all).
 * - TIMES_FRESH=1 discards the saved cursor and entity lists, re-listing from Postgres. The default resumes them, so a
 *   second run on the same volume never sees new entities or a changed Space set without it.
 * - BACKFILL_SKIP_GROUP_CHECK=1 skips the check that the feeder's group is empty. Only for a store no feeder will resume.
 */
import kafkajs from "kafkajs"
import {loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {PipeReader} from "./geodbPipe.js"
import {errorFields, log} from "./log.js"
import {createMetrics} from "./metrics.js"
import {ReadOnlyPg, pgConfig, pgTimesSource} from "./pgSource.js"
import {startServer} from "./server.js"
import {Store} from "./state.js"
import {runTimesBackfill} from "./timesBackfill.js"

const {Kafka, logLevel} = kafkajs

function intEnv(env: NodeJS.ProcessEnv, name: string, fallback: number, min: number): number {
	const v = Number(env[name] ?? fallback)
	if (!Number.isInteger(v) || v < min) throw new Error(`${name} must be an integer >= ${min}`)
	return v
}

async function main(): Promise<void> {
	const env = process.env
	const config = loadConfig(env)
	const databaseUrl = env.DATABASE_URL
	if (!databaseUrl) throw new Error("DATABASE_URL is required")
	const spacesText = (env.TIMES_SPACES ?? "mapped").trim()
	const spaces = spacesText === "mapped" || spacesText === "" ? [] : spacesText.split(",").map((s) => s.trim())
	if (spaces.some((s) => !/^[0-9a-fA-F-]{32,36}$/.test(s))) throw new Error('TIMES_SPACES must be "mapped" or a comma list of space ids')
	const opts = {
		spaces,
		batch: intEnv(env, "TIMES_BATCH", 100, 1),
		chunk: config.chunk,
		pauseMs: intEnv(env, "TIMES_PAUSE_MS", 100, 0),
		maxEntities: intEnv(env, "TIMES_MAX_ENTITIES", 0, 0),
		fresh: env.TIMES_FRESH === "1",
	}

	const metrics = createMetrics()
	const abort = new AbortController()
	const server = startServer(config.metricsPort, metrics, {ready: () => true})
	const store = new Store(`${config.dataDir}/feeder`)
	log("info", "times backfill starting", {...opts, spaces: opts.spaces.length || "mapped"})

	const kafka = new Kafka({
		clientId: "geodb-times-backfill",
		brokers: [config.kafka.broker],
		logLevel: logLevel.WARN,
		ssl: {ca: [config.kafka.caPem]},
		sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
	})
	const admin = kafka.admin()
	await admin.connect()
	const skipCheck = env.BACKFILL_SKIP_GROUP_CHECK === "1"
	const checkGroup = async () => {
		const g = (await admin.describeGroups([config.kafka.groupId])).groups[0]
		if (g && g.members.length > 0) throw new Error(`group ${g.groupId} has ${g.members.length} members: scale the feeder to 0 first`)
	}
	if (!skipCheck) await checkGroup()
	let lastGuard = Date.now()
	const guard = async () => {
		if (skipCheck || Date.now() - lastGuard < 60_000) return
		lastGuard = Date.now()
		await checkGroup()
	}

	const pg = new ReadOnlyPg(pgConfig(databaseUrl, env.DATABASE_SSL_CA))
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	const reader = new PipeReader({clientBin: config.clientBin, keyPath: env.GEODB_READER_KEY_PATH ?? config.keyPath, bindingPath: config.bindingPath})
	process.on("SIGTERM", () => abort.abort())
	process.on("SIGINT", () => abort.abort())

	log("info", "waiting for geodb-serve")
	await db.waitReady()
	const t0 = Date.now()
	let failure: unknown = null
	try {
		const stats = await runTimesBackfill({source: pgTimesSource(pg), reader, db, store, metrics, guard, signal: abort.signal}, opts)
		log(stats.deadLettered ? "error" : "info", "times backfill done", {...stats, seconds: Math.round((Date.now() - t0) / 1000)})
		if (stats.deadLettered > 0) throw new Error(`${stats.deadLettered} intents were refused (see errors.jsonl); fix and rerun`)
	} catch (e) {
		failure = e
	}
	reader.reset()
	await pg.close()
	await admin.disconnect().catch(() => {})
	server.close()
	if (failure) throw failure
}

main().then(
	() => process.exit(0),
	(e) => {
		log("error", "times backfill failed", errorFields(e))
		process.exit(1)
	},
)
