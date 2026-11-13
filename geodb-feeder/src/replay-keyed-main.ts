/**
 * geodb-replay-keyed: a one-shot operator Job, same image as the feeder. It replays a few Kafka offsets through the
 * keyed-record handlers whose intents were dead-lettered (replayKeyed.ts says why a consumer-group rewind is not enough).
 *
 * Arguments: --family governance|edit-meta  --partition <n>  --offsets N|N-M  [--apply]
 * - governance: topic `<env>.space.governance`, through the proposal-status, proposal-versions and proposal-votes
 *   handlers (each ignores events that are not its own).
 * - edit-meta: topic `<env>.knowledge.edits`, through the edit-meta handler.
 * Dry run unless --apply: it reads the messages and prints their event types; it writes nothing and needs no stopped
 * feeder. --apply writes geo-db and the feeder's state volume (system-space map), so the feeder must be scaled to 0:
 * it refuses while any feeder consumer group has members. It never reads or moves a consumer's offsets.
 * Replay hazard (geo-db ISS-679-318): the keyed families here write add, set_active true and put only, none of which ends
 * one of the six non-idempotent pairs; every document is still checked before it is sent and refused if it holds one
 * (replayHazard.ts refusingHazardPairs), since no record says which keyed records already applied.
 * Exit 0: replayed with no dead letter. Exit 1: failed, or a handler dead-lettered (see errors.jsonl).
 * Env: the feeder's (Kafka, geodb-client, DATA_DIR, the GEODB_* flags, which must equal the live feeder's).
 */
import kafkajs from "kafkajs"
import {createLock} from "./apply.js"
import {feederGroupIds, loadConfig} from "./config.js"
import {editMetaOffsetsDir, handleEditMetaMessage} from "./editMetaKeyed.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import type {KeyedRecordDeps} from "./keyedRecord.js"
import {errorFields, log} from "./log.js"
import {createMetrics} from "./metrics.js"
import {proposalStatusOffsetsDir, handleProposalStatusMessage} from "./proposalStatusKeyed.js"
import {handleProposalVersionMessage, proposalVersionsOffsetsDir} from "./proposalVersionsKeyed.js"
import {handleProposalVoteMessage, proposalVotesOffsetsDir} from "./proposalVotesKeyed.js"
import {ProposalOffsets} from "./proposals.js"
import {SpaceMapPublisher, kubeConfigMapPut} from "./publish.js"
import {refusingHazardPairs} from "./replayHazard.js"
import {type ReplayHandler, type ReplayMessage, describeMessage, parseRange, replayKeyedMessages} from "./replayKeyed.js"
import {readSealer} from "./spaces.js"
import {Store} from "./state.js"
import {SYSTEM_SPACE_MAP_KEY, SystemSpaces} from "./systemSpaces.js"

const {Kafka, logLevel} = kafkajs
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

type Args = {family: "governance" | "edit-meta"; partition: number; from: number; to: number; apply: boolean}

function parseArgs(argv: string[]): Args {
	const v = new Map<string, string>()
	let apply = false
	for (let i = 0; i < argv.length; i++) {
		const a = argv[i] as string
		if (a === "--apply") apply = true
		else if (a === "--dry-run") apply = false
		else if (a === "--family" || a === "--partition" || a === "--offsets") v.set(a, argv[++i] ?? "")
		else throw new Error(`unknown argument ${a.slice(0, 40)}`)
	}
	const family = v.get("--family")
	if (family !== "governance" && family !== "edit-meta") throw new Error("--family must be governance or edit-meta")
	const partition = Number(v.get("--partition"))
	if (!Number.isInteger(partition) || partition < 0) throw new Error("--partition must be a non-negative integer")
	const {from, to} = parseRange(v.get("--offsets") ?? "")
	return {family, partition, from, to, apply}
}

/** Read offsets [from, to] of one partition with a throwaway consumer that commits nothing. */
async function readRange(kafka: InstanceType<typeof Kafka>, topic: string, partition: number, from: number, to: number): Promise<ReplayMessage[]> {
	const admin = kafka.admin()
	await admin.connect()
	try {
		const p = (await admin.fetchTopicOffsets(topic)).find((o) => o.partition === partition)
		if (!p) throw new Error(`${topic} has no partition ${partition}`)
		if (from < Number(p.low)) throw new Error(`offset ${from} is gone: ${topic} partition ${partition} starts at ${p.low}`)
		if (to >= Number(p.high)) throw new Error(`offset ${to} does not exist yet: ${topic} partition ${partition} ends at ${p.high}`)
	} finally {
		await admin.disconnect().catch(() => {})
	}
	const consumer = kafka.consumer({groupId: `geodb-feeder-replay-keyed-v1-${process.env.ENVIRONMENT ?? "testnet"}`, sessionTimeout: 60_000, heartbeatInterval: 5000})
	const got = new Map<number, ReplayMessage>()
	let finish: () => void = () => {}
	const done = new Promise<void>((r) => {
		finish = r
	})
	await consumer.connect()
	try {
		await consumer.subscribe({topic, fromBeginning: false})
		await consumer.run({
			autoCommit: false,
			eachMessage: async ({partition: p, message}) => {
				if (p !== partition) return
				const o = Number(message.offset)
				if (o >= from && o <= to) got.set(o, {offset: message.offset, value: message.value, headers: message.headers as Record<string, unknown>})
				if (o >= to) finish()
			},
		})
		consumer.seek({topic, partition, offset: String(from)})
		await Promise.race([done, sleep(120_000)])
	} finally {
		await consumer.disconnect().catch(() => {})
	}
	const missing = []
	for (let o = from; o <= to; o++) if (!got.has(o)) missing.push(o)
	if (missing.length > 0) throw new Error(`did not read offsets ${missing.join(",")} of ${topic} partition ${partition} (compacted away or timed out)`)
	return [...got.values()]
}

async function main(): Promise<void> {
	const env = process.env
	const args = parseArgs(process.argv.slice(2))
	const config = loadConfig(env)
	const topic = args.family === "governance" ? config.proposalStatus.topic : config.editMeta.topic
	const kafka = new Kafka({
		clientId: "geodb-replay-keyed",
		brokers: [config.kafka.broker],
		logLevel: logLevel.WARN,
		ssl: {ca: [config.kafka.caPem]},
		sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
	})
	log("info", "replay-keyed starting", {family: args.family, topic, partition: args.partition, from: args.from, to: args.to, apply: args.apply})
	const messages = await readRange(kafka, topic, args.partition, args.from, args.to)
	for (const m of messages) log("info", "replay-keyed message", describeMessage(m))
	if (!args.apply) {
		log("info", "replay-keyed dry-run: nothing written", {
			messages: messages.length,
			hazard: "ISS-679-318: these families write add/set_active/put only, so no chunk can hold one of the six pairs; --apply refuses any chunk that does",
		})
		return
	}
	if (config.recordKeys !== true) throw new Error("GEODB_RECORD_KEYS=1 is required, as for the live feeder")
	const admin = kafka.admin()
	await admin.connect()
	try {
		for (const g of (await admin.describeGroups(feederGroupIds(config))).groups) {
			if (g.members.length > 0) throw new Error(`group ${g.groupId} has ${g.members.length} members: scale the feeder to 0 first`)
		}
	} finally {
		await admin.disconnect().catch(() => {})
	}

	const metrics = createMetrics()
	const abort = new AbortController()
	const store = new Store(`${config.dataDir}/feeder`)
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	db.apply = refusingHazardPairs(db.apply.bind(db))
	const systemSpaces = new SystemSpaces(`${config.dataDir}/feeder`)
	const systemPublisher = new SpaceMapPublisher(config.systemSpaceMapPath, kubeConfigMapPut(config.spaceMapConfigMap), metrics, SYSTEM_SPACE_MAP_KEY)
	systemPublisher.start()
	process.on("SIGTERM", () => abort.abort())
	process.on("SIGINT", () => abort.abort())
	const base = {
		db,
		store,
		metrics,
		beat: async () => {},
		signal: abort.signal,
		sealer: () => readSealer(config.sealerFile),
		allowPut: true,
		lock: createLock(),
		systemSpaces,
		publishSystem: (t: string) => systemPublisher.publish(t),
	}
	// The live consumers' offsets files are read, never written: a handler does not touch them.
	const deps = (dir: (d: string) => string): KeyedRecordDeps => ({...base, offsets: new ProposalOffsets(dir(config.dataDir))})
	const handlers: ReplayHandler<KeyedRecordDeps>[] =
		args.family === "governance"
			? [
					{name: "proposal_status", handle: handleProposalStatusMessage, deps: deps(proposalStatusOffsetsDir)},
					{
						name: "proposal_versions",
						handle: handleProposalVersionMessage,
						deps: {...deps(proposalVersionsOffsetsDir), storeProposerSpace: config.proposalVersions.storeProposerSpace} as KeyedRecordDeps,
					},
					{name: "proposal_votes", handle: handleProposalVoteMessage, deps: deps(proposalVotesOffsetsDir)},
				]
			: [{name: "edit_meta", handle: handleEditMetaMessage, deps: deps(editMetaOffsetsDir)}]
	log("info", "waiting for geodb-serve")
	await db.waitReady()
	let failure: unknown = null
	let deadLettered = 0
	try {
		const lines = await replayKeyedMessages(args.partition, messages, handlers)
		for (const l of lines) log(l.outcome === "dead_letter" ? "error" : "info", "replay-keyed outcome", l)
		deadLettered = lines.filter((l) => l.outcome === "dead_letter").length
		log(deadLettered ? "error" : "info", "replay-keyed applied", {messages: messages.length, applied: lines.filter((l) => l.outcome === "applied").length, deadLettered})
	} catch (e) {
		failure = e
	}
	systemPublisher.publish(systemSpaces.render())
	for (let i = 0; i < 300 && systemPublisher.pending; i++) await sleep(1000)
	systemPublisher.stop()
	if (failure) throw failure
	if (deadLettered > 0) process.exitCode = 1
}

main().then(
	() => process.exit(),
	(e) => {
		log("error", "replay-keyed failed", errorFields(e))
		process.exit(1)
	},
)
