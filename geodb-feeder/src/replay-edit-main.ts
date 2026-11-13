/**
 * geodb-replay-edit: a one-shot operator Job, same image as the feeder. See replayEdit.ts for the rule and
 * geodb/RUNBOOK.md for how to run it.
 *
 * Arguments: --uri ipfs://<cid> --space <gaia space id> --block <n> --created-at <unix seconds>
 *            [--sequence <n>] [--file <path>] [--edit <edit id>] [--apply]
 * Dry run unless --apply. Exit 0: dry-run ok or applied. Exit 2: refused (older than the target's newer state, or a
 * chunk that already applied holds an ISS-679-318 pair). `--edit` keys the feeder's dead-letter record
 * (DATA_DIR/feeder/errors.jsonl, read only), which says which chunks never applied; see replayHazard.ts.
 * Env, beyond the feeder's own (geodb-client, DATA_DIR): REPLAY_EDIT_IPFS_GATEWAY (https URL, default
 * https://gateway.mesh3.network/ipfs), GEODB_READER_KEY_PATH (the api's read-only key, used for the safety reads).
 * With --apply the feeder must be scaled to 0: the Job shares its state volume and journals.
 */
import {existsSync, readFileSync} from "node:fs"
import * as grc from "@geoprotocol/grc-20"
import {createLock} from "./apply.js"
import {loadConfig} from "./config.js"
import {GeoDb, spawnRunner} from "./geodb.js"
import {PipeReader} from "./geodbPipe.js"
import type {Decode} from "./handle.js"
import {errorFields, log} from "./log.js"
import {createMetrics} from "./metrics.js"
import {loadPayload, parseArgs, runReplayEdit} from "./replayEdit.js"
import {Store} from "./state.js"

async function main(): Promise<void> {
	const env = process.env
	const args = parseArgs(process.argv.slice(2))
	const config = loadConfig(env)
	const store = new Store(`${config.dataDir}/feeder`)
	const errorsPath = `${config.dataDir}/feeder/errors.jsonl`
	const deadLetters = existsSync(errorsPath) ? readFileSync(errorsPath, "utf8") : ""
	const decode: Decode = async (payload) => (await grc.decodeEditAuto(payload)) as unknown as {ops: never[]}
	const payload = await loadPayload(args, {
		gateway: env.REPLAY_EDIT_IPFS_GATEWAY ?? "https://gateway.mesh3.network/ipfs",
		fetch: fetch as never,
		readFile: (p) => readFileSync(p),
	})
	const db = new GeoDb(
		{runDir: config.runDir},
		spawnRunner({clientBin: config.clientBin, keyPath: config.keyPath, bindingPath: config.bindingPath, cwd: config.runDir}),
	)
	const reader = new PipeReader({clientBin: config.clientBin, keyPath: env.GEODB_READER_KEY_PATH ?? config.keyPath, bindingPath: config.bindingPath})
	log("info", "replay-edit starting", {uri: args.uri, space: args.space, block: args.block, edit: args.edit ?? null, apply: args.apply, bytes: payload.length, deadLetterRecord: errorsPath, deadLetterBytes: deadLetters.length})
	await db.waitReady()
	try {
		const result = await runReplayEdit(
			{
				db,
				store,
				metrics: createMetrics(),
				beat: async () => {},
				lock: createLock(),
				decode,
				reader,
				spaceRecord: (s) => store.spaces[s],
				payload,
				chunk: config.chunk,
				positions: config.relationPositions,
				facets: config.relationFacets,
				entityTimes: config.entityTimes,
				legacyMap: config.legacyIdMap,
				allowPut: config.recordKeys,
				deadLetters,
			},
			args,
			store,
		)
		log(result.outcome === "refused" ? "error" : "info", `replay-edit ${result.outcome}`, {...result, conflicts: "conflicts" in result ? result.conflicts.slice(0, 20) : undefined})
		process.exitCode = result.outcome === "refused" ? 2 : 0
	} finally {
		reader.reset()
	}
}

main().then(
	() => process.exit(),
	(e) => {
		log("error", "replay-edit failed", errorFields(e))
		process.exit(1)
	},
)
