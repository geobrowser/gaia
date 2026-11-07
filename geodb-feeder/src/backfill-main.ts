/**
 * geodb-backfill: a one-shot Job, same image as the feeder, run with the feeder scaled to 0. See backfill.ts for
 * what it writes and why that is safe against the live stream, and geodb/RUNBOOK.md §2b for how to run it.
 *
 * Env, beyond the feeder's own (Kafka, geodb-client, DATA_DIR):
 * - DATABASE_URL, DATABASE_SSL_CA: gaia Postgres, read only (every query runs in a READ ONLY transaction).
 * - BACKFILL_SPACES: "mapped" (Spaces geo-db already has), "all" (also founds new ones, largest first, up to
 *   BACKFILL_SPACE_CAP), or a comma list of gaia space ids. Required: the manifest's placeholder refuses.
 * - BACKFILL_SPACE_CAP (192), BACKFILL_BATCH (100 entities), BACKFILL_PAUSE_MS (100 after each apply),
 *   BACKFILL_MAX_ENTITIES (0: all), BACKFILL_EXCLUDE_PROPERTIES (default: the derived properties no edit
 *   carries; "none" for none), INDEXER_GROUP_ID (kg-indexer-v8-<env>).
 * - GEODB_READER_KEY_PATH: the key the read-back of known entities uses (the api's reader key).
 * - BACKFILL_SKIP_ORDERING_CHECK=1 skips the Kafka precondition. Only for a store no feeder will ever resume.
 * - BACKFILL_MODE=held-repair: not a Space backfill (BACKFILL_SPACES is not read). Writes only the values the pre-#1244
 *   backfill skipped (see heldRepair.ts), under its own cursor directory. BACKFILL_BATCH, BACKFILL_PAUSE_MS,
 *   BACKFILL_MAX_ENTITIES and BACKFILL_EXCLUDE_PROPERTIES apply. DRY_RUN=1 reads and logs counts only, writes nothing
 *   (no geo-db, journal, cursor or space-map write), and skips the Kafka precondition (it needs no stopped feeder).
 */
import kafkajs from "kafkajs"
import {excludeFrom, parseScope, runBackfill} from "./backfill.js"
import {loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {runHeldRepair} from "./heldRepair.js"
import {PipeReader} from "./geodbPipe.js"
import {errorFields, log} from "./log.js"
import {createMetrics} from "./metrics.js"
import {checkOrdering, type OffsetsAdmin} from "./ordering.js"
import {ReadOnlyPg, pgBackfillSource, pgConfig, pgHeldRepairSource} from "./pgSource.js"
import {SpaceMapPublisher, kubeConfigMapPut} from "./publish.js"
import {startServer} from "./server.js"
import {readSealer} from "./spaces.js"
import {Store, backfilledSpaces, renderSpaceMap} from "./state.js"

const {Kafka, logLevel} = kafkajs
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

function intEnv(env: NodeJS.ProcessEnv, name: string, fallback: number, min: number): number {
	const v = Number(env[name] ?? fallback)
	if (!Number.isInteger(v) || v < min) throw new Error(`${name} must be an integer >= ${min}`)
	return v
}

async function main(): Promise<void> {
	const env = process.env
	const config = loadConfig(env)
	const environment = env.ENVIRONMENT ?? "testnet"
	const mode = env.BACKFILL_MODE ?? "spaces"
	if (mode !== "spaces" && mode !== "held-repair") throw new Error(`BACKFILL_MODE must be "spaces" or "held-repair", not ${JSON.stringify(mode)}`)
	const heldRepair = mode === "held-repair"
	if (heldRepair && env.DRY_RUN !== "0" && env.DRY_RUN !== "1") {
		throw new Error('DRY_RUN is required ("1" counts only, "0" writes); the manifest placeholder was not replaced')
	}
	const dryRun = env.DRY_RUN === "1"
	if (dryRun && !heldRepair) throw new Error("DRY_RUN is only for BACKFILL_MODE=held-repair")
	if (!heldRepair && (!env.BACKFILL_SPACES || env.BACKFILL_SPACES.includes("__"))) {
		throw new Error('BACKFILL_SPACES is required ("mapped", "all" or a comma list); the manifest placeholder was not replaced')
	}
	const scope = parseScope(heldRepair ? "mapped" : env.BACKFILL_SPACES)
	const databaseUrl = env.DATABASE_URL
	if (!databaseUrl) throw new Error("DATABASE_URL is required")
	const opts = {
		scope,
		spaceCap: intEnv(env, "BACKFILL_SPACE_CAP", 192, 1),
		batch: intEnv(env, "BACKFILL_BATCH", 100, 1),
		chunk: config.chunk,
		pauseMs: intEnv(env, "BACKFILL_PAUSE_MS", 100, 0),
		exclude: excludeFrom(env.BACKFILL_EXCLUDE_PROPERTIES),
		positions: config.relationPositions,
		legacyMap: config.legacyIdMap,
		maxEntities: intEnv(env, "BACKFILL_MAX_ENTITIES", 0, 0),
	}
	const indexerGroup = env.INDEXER_GROUP_ID ?? `kg-indexer-v8-${environment}`
	const skipOrdering = env.BACKFILL_SKIP_ORDERING_CHECK === "1" || dryRun

	const metrics = createMetrics()
	const abort = new AbortController()
	const server = startServer(config.metricsPort, metrics, {ready: () => true})
	const store = new Store(`${config.dataDir}/feeder`)
	log("info", "backfill starting", {
		mode,
		dryRun,
		scope: heldRepair ? undefined : env.BACKFILL_SPACES,
		spaceCap: opts.spaceCap,
		batch: opts.batch,
		chunk: opts.chunk,
		pauseMs: opts.pauseMs,
		maxEntities: opts.maxEntities,
		excludedProperties: opts.exclude.size,
		entities: store.entityCount,
		statements: store.statementCount,
		spaces: Object.values(store.spaces).filter((s) => s.geo).length,
	})

	const kafka = new Kafka({
		clientId: "geodb-backfill",
		brokers: [config.kafka.broker],
		logLevel: logLevel.WARN,
		ssl: {ca: [config.kafka.caPem]},
		sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
	})
	const admin = kafka.admin()
	if (!skipOrdering) await admin.connect()
	const check = () =>
		checkOrdering({
			admin: admin as unknown as OffsetsAdmin,
			topic: config.kafka.topic,
			feederGroup: config.kafka.groupId,
			indexerGroup,
			store,
		})
	if (!skipOrdering) {
		const c = await check()
		log(c.ok ? "info" : "error", "ordering precondition", {...c})
		if (!c.ok) throw new Error(`ordering precondition failed: ${c.problems.join("; ")}`)
	} else log("warn", "ordering precondition skipped (BACKFILL_SKIP_ORDERING_CHECK=1)")

	// Between batches: stop at once if the feeder came back (its group has members again).
	let lastGuard = 0
	const guard = async () => {
		if (skipOrdering || Date.now() - lastGuard < 60_000) return
		lastGuard = Date.now()
		const g = (await admin.describeGroups([config.kafka.groupId])).groups[0]
		if (g && g.members.length > 0) throw new Error(`feeder group ${config.kafka.groupId} has ${g.members.length} members again: stopping`)
	}

	const pg = new ReadOnlyPg(pgConfig(databaseUrl, env.DATABASE_SSL_CA))
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	// Reads use the api's read-only key, the same grant and path the card route reads with.
	const reader = new PipeReader({
		clientBin: config.clientBin,
		keyPath: env.GEODB_READER_KEY_PATH ?? config.keyPath,
		bindingPath: config.bindingPath,
	})
	const publisher = new SpaceMapPublisher(config.spaceMapPath, kubeConfigMapPut(config.spaceMapConfigMap), metrics)
	if (!heldRepair) publisher.start()

	const stop = () => {
		log("info", "shutting down: the cursor resumes from the last finished batch")
		abort.abort()
	}
	process.on("SIGTERM", stop)
	process.on("SIGINT", stop)

	if (!dryRun) {
		log("info", "waiting for geodb-serve")
		await db.waitReady()
	}
	const t0 = Date.now()
	let failure: unknown = null
	try {
		if (heldRepair) {
			const stats = await runHeldRepair(
				{source: pgHeldRepairSource(pg), reader, db, store, metrics, guard, signal: abort.signal},
				{
					batch: opts.batch,
					chunk: opts.chunk,
					pauseMs: opts.pauseMs,
					exclude: opts.exclude,
					positions: opts.positions,
					maxEntities: opts.maxEntities,
					dryRun,
				},
			)
			log("info", "held-repair done", {...stats, seconds: Math.round((Date.now() - t0) / 1000)})
		} else {
			const stats = await runBackfill(
				{
					source: pgBackfillSource(pg),
					reader,
					db,
					store,
					metrics,
					sealer: () => readSealer(config.sealerFile),
					publish: (t) => publisher.publish(t),
					guard,
					signal: abort.signal,
				},
				opts,
			)
			log("info", "backfill done", {
				...stats,
				spacesSkippedCap: stats.spacesSkippedCap.length,
				seconds: Math.round((Date.now() - t0) / 1000),
				entitiesKnown: store.entityCount,
				statementsKnown: store.statementCount,
			})
		}
	} catch (e) {
		failure = e
	}
	// The space map must reach the ConfigMap before exit, or the api cannot map the Spaces just founded.
	// (held-repair founds no Space, so it has no map to publish.)
	if (!heldRepair) {
		publisher.publish(renderSpaceMap(store.spaces, backfilledSpaces(store.dir)))
		for (let i = 0; i < 300 && publisher.pending; i++) await sleep(1000)
		if (publisher.pending) log("error", "space map not yet published; the feeder republishes it on start")
	}
	publisher.stop()
	reader.reset()
	await pg.close()
	if (!skipOrdering) await admin.disconnect().catch(() => {})
	server.close()
	if (failure) throw failure
}

main().then(
	() => process.exit(0),
	(e) => {
		log("error", "backfill failed", errorFields(e))
		process.exit(1)
	},
)
