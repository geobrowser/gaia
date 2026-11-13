/**
 * geodb-replay-deadletters: a one-shot operator Job, same image as the feeder. See replayDeadLetters.ts for the rule
 * and geodb/RUNBOOK.md §2b-quater-replay for how to run it.
 *
 * Arguments: see `parseArgs` in replayDeadLetters.ts. Dry run unless --apply.
 * Exit 0: dry run ok, applied, or nothing matched. Exit 2: refused (a document's target is unknown, a pair under an
 * error that may have been sent, newer state at the target, or state unchecked under --apply). Exit 1: failed.
 *
 * Reads DATA_DIR/feeder/errors.jsonl, spaces.json and system-spaces.json, never writes them; with --apply appends
 * DATA_DIR/feeder/replay-deadletters.done.jsonl (fsync per chunk). With --apply the feeder must be scaled to 0: the
 * Job mounts the feeder's RWO state volume, and a live consumer could write a cell this replay would then revert.
 */
import {existsSync, readFileSync} from "node:fs"
import kafkajs from "kafkajs"
import {feederGroupIds, loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {PipeReader} from "./geodbPipe.js"
import {errorFields, log} from "./log.js"
import type {SystemFamily} from "./recordKinds.js"
import {parseArgs, runReplayDeadLetters} from "./replayDeadLetters.js"
import {type SpaceRecord, appendDurable} from "./state.js"

const {Kafka, logLevel} = kafkajs

const readJson = <T>(file: string, empty: T): T => (existsSync(file) ? (JSON.parse(readFileSync(file, "utf8")) as T) : empty)

async function main(): Promise<void> {
	const env = process.env
	const args = parseArgs(process.argv.slice(2))
	const config = loadConfig(env)
	const dir = `${config.dataDir}/feeder`
	const errorsPath = `${dir}/errors.jsonl`
	const donePath = `${dir}/replay-deadletters.done.jsonl`
	if (!existsSync(errorsPath)) throw new Error(`no dead-letter record at ${errorsPath}`)
	const record = readFileSync(errorsPath, "utf8")
	const done = existsSync(donePath) ? readFileSync(donePath, "utf8") : ""
	const spaces = readJson<Record<string, SpaceRecord>>(`${dir}/spaces.json`, {})
	const system = readJson<Partial<Record<SystemFamily, SpaceRecord>>>(`${dir}/system-spaces.json`, {})
	log("info", "replay-deadletters starting", {...args, record: errorsPath, recordBytes: record.length, done: donePath, doneBytes: done.length})

	if (args.apply) {
		if (config.recordKeys !== true) throw new Error("GEODB_RECORD_KEYS=1 is required, as for the live feeder")
		const kafka = new Kafka({
			clientId: "geodb-replay-deadletters",
			brokers: [config.kafka.broker],
			logLevel: logLevel.WARN,
			ssl: {ca: [config.kafka.caPem]},
			sasl: {mechanism: "plain", username: config.kafka.username, password: config.kafka.password},
		})
		const admin = kafka.admin()
		await admin.connect()
		try {
			for (const g of (await admin.describeGroups(feederGroupIds(config))).groups) {
				if (g.members.length > 0) throw new Error(`group ${g.groupId} has ${g.members.length} members: scale the feeder to 0 first`)
			}
		} finally {
			await admin.disconnect().catch(() => {})
		}
	}

	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	const reader = new PipeReader({clientBin: config.clientBin, keyPath: env.GEODB_READER_KEY_PATH ?? config.keyPath, bindingPath: config.bindingPath})
	await db.waitReady()
	try {
		const result = await runReplayDeadLetters(
			{
				record,
				done,
				spaces: (s) => spaces[s],
				system: (f) => system[f],
				reader,
				db,
				appendDone: (text) => appendDurable(donePath, text),
				chunk: config.chunk,
				log,
			},
			args,
		)
		const {docs, ...rest} = result
		log(result.outcome === "refused" || result.outcome === "failed" ? "error" : "info", `replay-deadletters ${result.outcome}`, {
			...rest,
			documents: docs.map((d) => ({doc: d.doc, family: d.family, entries: d.entries, done: d.done, pending: d.pending, state: d.state, refuse: d.refuse})),
		})
		process.exitCode = result.outcome === "refused" ? 2 : result.outcome === "failed" ? 1 : 0
	} finally {
		reader.reset()
	}
}

main().then(
	() => process.exit(),
	(e) => {
		log("error", "replay-deadletters failed", errorFields(e))
		process.exit(1)
	},
)
