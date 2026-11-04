/**
 * geo-db lexical search for the gateway's search shadow.
 *
 * The long-lived `geodb-client pipe` workers (client.ts) only answer GeoQL documents, and GeoQL has no lexical
 * root: a lexical search is its own request (`LexicalSearch`, docs/api/09 §1), which the client tool sends with
 * its `search` verb. So each search starts one `geodb-client ... search` process, bounded in number and time,
 * and reads the ranked hits it prints, one per line: the entity first (32 hex digits), then the score, the
 * statement and its value, tab-separated. Searches are only ever started for a sampled fraction of shadowed
 * operations, so a process start and handshake per search is affordable; past `maxConcurrent` a search is
 * refused `overloaded` instead of queued.
 */

import {execFile} from "node:child_process"
import {GeoDbError} from "./client"
import type {GeoDbCardConfig} from "./config"

const HEX32 = /^[0-9a-f]{32}$/
const DEFAULT_TIMEOUT_MS = 3000
const DEFAULT_MAX_CONCURRENT = 2
/** geo-db's bound on the Spaces of one scope (`source.MAX_SOURCES`). */
const MAX_SPACES = 1024
const MAX_OUTPUT_BYTES = 4 * 1024 * 1024

export type GeoDbSearchMode = "typeahead" | "bm25"

export type GeoDbSearchClient = {
	/**
	 * Entity ids (geo-db's id space, 32 hex digits) best first, each once, across `geoSpaceIds`: one request, one
	 * ranking (geo-db scores a several-Space search against the Spaces' summed statistics, docs/api/09 §1).
	 */
	searchLexical(args: {geoSpaceIds: string[]; text: string; mode: GeoDbSearchMode; limit: number}): Promise<string[]>
}

/** Runs the client tool with these arguments after its global flags; resolves to its standard output. */
export type RunSearch = (config: GeoDbCardConfig, args: string[], timeoutMs: number) => Promise<string>

const defaultRun: RunSearch = (config, args, timeoutMs) =>
	new Promise((resolve, reject) => {
		execFile(
			config.clientBin,
			["--key", config.keyPath, "--binding", config.bindingPath, ...args],
			{timeout: timeoutMs, maxBuffer: MAX_OUTPUT_BYTES, killSignal: "SIGKILL"},
			(error, stdout, stderr) => {
				if (!error) return resolve(stdout)
				const killed = (error as {killed?: boolean}).killed === true
				const detail = (stdout.trim() || stderr.trim() || String(error)).slice(0, 300)
				reject(
					killed
						? new GeoDbError("timeout", `geo-db search exceeded ${timeoutMs} ms`)
						: new GeoDbError("client_failed", detail),
				)
			},
		)
	})

/** The entities of the tool's `search` output, in order, each once. A `partial: <n>` line is not a hit. */
export function parseSearchOutput(output: string): string[] {
	const seen = new Set<string>()
	for (const line of output.split("\n")) {
		const entity = line.split("\t", 1)[0] ?? ""
		if (HEX32.test(entity)) seen.add(entity)
	}
	return [...seen]
}

export function createCliGeoDbSearchClient(
	config: GeoDbCardConfig,
	options: {run?: RunSearch; timeoutMs?: number; maxConcurrent?: number} = {},
): GeoDbSearchClient {
	const run = options.run ?? defaultRun
	const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS
	const maxConcurrent = options.maxConcurrent ?? DEFAULT_MAX_CONCURRENT
	let inFlight = 0
	return {
		async searchLexical({geoSpaceIds, text, mode, limit}) {
			if (geoSpaceIds.length === 0 || geoSpaceIds.length > MAX_SPACES)
				throw new GeoDbError("bad_request", `a search takes 1 to ${MAX_SPACES} spaces`)
			if (!geoSpaceIds.every((id) => HEX32.test(id)))
				throw new GeoDbError("bad_request", "space id must be 32 hex digits")
			// The tool reads any later argument that looks like a flag as a flag, so text cannot start with `-`.
			if (text.length === 0 || text.startsWith("-")) throw new GeoDbError("bad_request", "unusable query text")
			if (inFlight >= maxConcurrent) throw new GeoDbError("overloaded", `${inFlight} geo-db searches running`)
			inFlight++
			try {
				const n = String(Math.max(1, Math.min(Math.floor(limit), 100)))
				const output = await run(
					config,
					[
						"search",
						...[...new Set(geoSpaceIds)].flatMap((id) => ["--space", id]),
						...["--mode", mode, "--limit", n, "--first", n, "--pages", "1", text],
					],
					timeoutMs,
				)
				return parseSearchOutput(output)
			} finally {
				inFlight--
			}
		},
	}
}
