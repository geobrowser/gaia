/**
 * geodb-feeder: topic `<env>.knowledge.edits` -> HermesEdit -> GRC-20 ops -> geodb-client apply -> geodb-serve.
 *
 * Delivery is at-least-once with a resumable apply: see state.ts for what is stored and why a replayed
 * message is safe. The feeder is its own Deployment and reaches geodb-serve over TCP; it founds Spaces online.
 */
import {existsSync, readFileSync} from "node:fs"
import * as zstd from "@bokuweb/zstd-wasm"
import * as grc from "@geoprotocol/grc-20"
import kafkajs from "kafkajs"
import {type Config, loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {type Decode, handleMessage} from "./handle.js"
import {errorFields, log} from "./log.js"
import {createMetrics} from "./metrics.js"
import {SpaceMapPublisher, kubeConfigMapPut} from "./publish.js"
import {startServer} from "./server.js"
import {ensureSpace} from "./spaces.js"
import {Store, renderSpaceMap} from "./state.js"

const {Kafka, logLevel, CompressionTypes, CompressionCodecs} = kafkajs
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

async function main(config: Config): Promise<void> {
	const metrics = createMetrics()
	const abort = new AbortController()
	let consumerRunning = false
	let serveUp = false
	const server = startServer(config.metricsPort, metrics, {ready: () => consumerRunning && serveUp})

	await zstd.init()
	CompressionCodecs[CompressionTypes.ZSTD] = () => ({
		compress: async (enc: {buf: Uint8Array}) => Buffer.from(zstd.compress(enc.buf)),
		decompress: async (buf: Uint8Array) => Buffer.from(zstd.decompress(buf)),
	})
	const decode: Decode = async (payload) => (await grc.decodeEditAuto(payload)) as unknown as {ops: never[]}

	const store = new Store(`${config.dataDir}/feeder`)
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	const publisher = new SpaceMapPublisher(config.spaceMapPath, kubeConfigMapPut(config.spaceMapConfigMap), metrics)
	publisher.start()
	publisher.publish(renderSpaceMap(store.spaces))
	log("info", "state loaded", {
		spaces: Object.keys(store.spaces).length,
		entities: store.entityCount,
		statements: store.statementCount,
		hadOffsets: store.hadOffsets,
		topic: config.kafka.topic,
		group: config.kafka.groupId,
	})

	log("info", "waiting for geodb-serve")
	await db.waitReady()
	serveUp = true
	metrics.spaces.set({}, Object.values(store.spaces).filter((s) => s.geo).length)

	const kafka = new Kafka({
		clientId: "geodb-feeder",
		brokers: [config.kafka.broker],
		logLevel: logLevel.WARN,
		ssl: {ca: [config.kafka.caPem]},
		sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
	})
	const admin = kafka.admin()
	await admin.connect()
	if (!store.hadOffsets) {
		// An empty store must not resume from a group commit left by an earlier store: that would skip edits.
		try {
			await admin.resetOffsets({groupId: config.kafka.groupId, topic: config.kafka.topic, earliest: true})
			log("info", "empty store: group offsets reset to earliest")
		} catch (e) {
			log("warn", "could not reset group offsets (group still active?)", errorFields(e))
		}
	}

	const spaceDeps = {db, store, metrics, sealer: () => readSealer(config.sealerFile), publish: (t: string) => publisher.publish(t)}
	if (config.spacesListPath && existsSync(config.spacesListPath)) {
		// Found the Spaces already known up front, so the first messages do not wait on a founding.
		for (const g of readFileSync(config.spacesListPath, "utf8").split("\n").map((l) => l.trim()).filter(Boolean)) {
			await ensureSpace(spaceDeps, g)
		}
	}

	const consumer = kafka.consumer({
		groupId: config.kafka.groupId,
		sessionTimeout: 120_000,
		heartbeatInterval: 5000,
		maxBytesPerPartition: 4 * 1024 * 1024,
	})
	await consumer.connect()
	await consumer.subscribe({topic: config.kafka.topic, fromBeginning: true})
	consumer.on(consumer.events.CRASH, (e) => {
		log("error", "consumer crashed", {error: String(e.payload?.error).slice(0, 300)})
		process.exit(1)
	})

	let lastCommit = 0
	await consumer.run({
		autoCommit: false,
		partitionsConsumedConcurrently: 1,
		eachBatch: async ({batch, resolveOffset, heartbeat}) => {
			for (const m of batch.messages) {
				if (abort.signal.aborted) return
				const offset = Number(m.offset)
				const last = store.lastDone(batch.partition)
				if (last !== null && offset <= last) {
					resolveOffset(m.offset)
					continue
				}
				if (last !== null && offset > last + 1) metrics.offsetGaps.inc({}, offset - last - 1)
				const deps = {...spaceDeps, beat: heartbeat, signal: abort.signal, decode, chunk: config.chunk}
				let outcome: string
				for (;;) {
					try {
						outcome = await handleMessage(deps, batch.partition, m)
						break
					} catch (e) {
						if (abort.signal.aborted) return
						log("error", "message failed, retrying (never skipped)", {partition: batch.partition, offset, ...errorFields(e)})
						await sleep(5000)
						await heartbeat()
					}
				}
				store.markDone(batch.partition, offset)
				metrics.edits.inc({outcome})
				metrics.lastApplied.set({}, Date.now() / 1000)
				metrics.lastOffset.set({partition: String(batch.partition)}, offset)
				resolveOffset(m.offset)
				await heartbeat()
				if (Date.now() - lastCommit > 5000) {
					lastCommit = Date.now()
					await consumer
						.commitOffsets([{topic: config.kafka.topic, partition: batch.partition, offset: String(offset + 1)}])
						.catch((e) => log("warn", "group commit failed", errorFields(e)))
				}
			}
		},
	})
	consumerRunning = true
	metrics.consumerUp.set({}, 1)

	const poll = async () => {
		try {
			const s = await db.spaces(10_000)
			serveUp = s !== null
			metrics.serveUp.set({}, serveUp ? 1 : 0)
			for (const p of await admin.fetchTopicOffsets(config.kafka.topic)) {
				const last = store.lastDone(p.partition)
				const next = last === null ? Number(p.low) : last + 1
				metrics.lag.set({partition: String(p.partition)}, Math.max(0, Number(p.high) - next))
			}
		} catch (e) {
			log("warn", "lag poll failed", errorFields(e))
		}
	}
	const pollTimer = setInterval(() => void poll(), config.lagPollMs)
	await poll()

	const stop = async () => {
		log("info", "shutting down")
		abort.abort()
		clearInterval(pollTimer)
		publisher.stop()
		await consumer.disconnect().catch(() => {})
		await admin.disconnect().catch(() => {})
		server.close()
		process.exit(0)
	}
	process.on("SIGTERM", () => void stop())
	process.on("SIGINT", () => void stop())
}

main(loadConfig()).catch((e) => {
	log("error", "fatal", errorFields(e))
	process.exit(1)
})

/** serve's sealer key id (public). The supervisor publishes it to the geodb-space-map ConfigMap, mounted here. */
async function readSealer(path: string): Promise<string> {
	for (;;) {
		try {
			const id = readFileSync(path, "utf8").trim()
			if (/^[0-9a-f]{64}$/.test(id)) return id
		} catch {
			// not published yet
		}
		log("warn", "waiting for serve's sealer id", {path})
		await sleep(5000)
	}
}
