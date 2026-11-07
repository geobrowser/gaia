/**
 * The reverse id map's geo-db twin (GEODB_ID_MAP_TWIN, off by default; plan 10.12).
 *
 * `idMap.ts` turns geo-db ids back into gaia ids with Postgres (`geodb.conform`, migration 0116). geo-db now holds that
 * map itself: a legacy id's keyed id and a `sys.legacy.Id` map statement, read by the card root's `.args.by_legacy`
 * (geo-db ISS-629-174, bac1c75004). This twin asks that read about the pairs Postgres just answered and counts whether
 * geo-db agrees. Postgres stays the answer: the twin runs after it, in the background, and a request never waits for it.
 *
 * Direction: Postgres maps geo id -> gaia id; the twin asks `by_legacy` with each gaia id and expects the geo id back.
 * (`legacy_of`, the same direction as Postgres, cannot be compared: the client prints a `Bytes` original as `<bytes>`.)
 * Ids Postgres misses are not asked: there is no pair to hold geo-db to.
 *
 * `gaia_api_geodb_id_map_twin_ids_total{result}`, one count a pair of ids (ids as asked, so a pair is counted once a read):
 *  - match: geo-db answered the expected keyed id (a v4 id as itself);
 *  - mismatch: geo-db answered a different keyed id, or no row for a v4 id (which always answers as itself);
 *  - unmapped_in_geodb: a non-v4 id geo-db has no map statement for. Expected until the feeder writes the map statements,
 *    so it is kept apart from a mismatch;
 *  - conflict: a keyed id nothing asked for, beyond the mismatches: two Spaces map one original to two keyed ids;
 *  - failed: the read failed or timed out (and the twin cools down).
 *
 * Bounds: a sampled fraction of calls, one read in flight, at most 32 ids a read, a timeout, and a cool-down after a failure.
 * `gaia_api_geodb_id_map_twin_skipped_total{reason}` counts the calls left alone.
 */

import type {GeoDbCardClient} from "./client"
import {MAX_CARD_IDS} from "./client"
import type {GaiaIdsOf} from "./idMap"

export const TWIN_RESULTS = ["match", "mismatch", "unmapped_in_geodb", "conflict", "failed"] as const
export type TwinResult = (typeof TWIN_RESULTS)[number]

export const TWIN_SKIPS = ["sampled_out", "busy", "cooldown"] as const
export type TwinSkip = (typeof TWIN_SKIPS)[number]

export type IdMapTwinConfig = {
	enabled: boolean
	/** Fraction of resolved calls the twin checks, 0 to 1. */
	sample: number
	/** Budget of one geo-db read, in ms. */
	timeoutMs: number
	/** How long the twin stays quiet after a failed read, in ms. */
	cooldownMs: number
}

const fraction = (raw: string | undefined, fallback: number) => {
	const n = Number(raw)
	return raw !== undefined && raw.trim() !== "" && Number.isFinite(n) && n >= 0 && n <= 1 ? n : fallback
}
const positive = (raw: string | undefined, fallback: number) => {
	const n = Number(raw)
	return Number.isFinite(n) && n > 0 ? n : fallback
}

export function readIdMapTwinConfig(env: Record<string, string | undefined>): IdMapTwinConfig {
	const flag = env.GEODB_ID_MAP_TWIN?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		sample: fraction(env.GEODB_ID_MAP_TWIN_SAMPLE, 0.05),
		timeoutMs: positive(env.GEODB_ID_MAP_TWIN_TIMEOUT_MS, 2000),
		cooldownMs: positive(env.GEODB_ID_MAP_TWIN_COOLDOWN_MS, 30_000),
	}
}

// --- metrics -------------------------------------------------------------------------------------------------

let active = false
const results = new Map<TwinResult, number>()
const skips = new Map<TwinSkip, number>()
let readsOk = 0
let readsFailed = 0
let readSeconds = 0

export function renderIdMapTwinMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_geodb_id_map_twin_ids_total Ids the reverse id map's geo-db twin checked, by result: match, mismatch, unmapped_in_geodb (no map statement yet), conflict (two keyed ids) or failed. Postgres stays the answer.",
		"# TYPE gaia_api_geodb_id_map_twin_ids_total counter",
		...TWIN_RESULTS.map((r) => `gaia_api_geodb_id_map_twin_ids_total{result="${r}"} ${results.get(r) ?? 0}`),
		"# HELP gaia_api_geodb_id_map_twin_skipped_total Resolved calls the twin did not check, by reason.",
		"# TYPE gaia_api_geodb_id_map_twin_skipped_total counter",
		...TWIN_SKIPS.map((r) => `gaia_api_geodb_id_map_twin_skipped_total{reason="${r}"} ${skips.get(r) ?? 0}`),
		"# HELP gaia_api_geodb_id_map_twin_reads_total geo-db legacy-id reads the twin made, by outcome.",
		"# TYPE gaia_api_geodb_id_map_twin_reads_total counter",
		`gaia_api_geodb_id_map_twin_reads_total{outcome="ok"} ${readsOk}`,
		`gaia_api_geodb_id_map_twin_reads_total{outcome="error"} ${readsFailed}`,
		"# HELP gaia_api_geodb_id_map_twin_read_seconds_total Time spent in the twin's geo-db reads.",
		"# TYPE gaia_api_geodb_id_map_twin_read_seconds_total counter",
		`gaia_api_geodb_id_map_twin_read_seconds_total ${readSeconds}`,
	]
	return `${lines.join("\n")}\n`
}

/** Tests only. */
export function __resetIdMapTwinMetricsForTests(): void {
	active = false
	results.clear()
	skips.clear()
	readsOk = 0
	readsFailed = 0
	readSeconds = 0
}

const count = (result: TwinResult, n: number) => {
	if (n > 0) results.set(result, (results.get(result) ?? 0) + n)
}
const skip = (reason: TwinSkip) => skips.set(reason, (skips.get(reason) ?? 0) + 1)

// --- comparison ----------------------------------------------------------------------------------------------

export type IdPair = {geo: string; gaia: string}
export type TwinVerdict = {
	match: number
	mismatch: number
	unmapped_in_geodb: number
	conflict: number
	/** The gaia ids of the mismatches. */
	mismatched: string[]
}

/**
 * Hold what geo-db answered (`keyed`, the entity cells of its rows) to what Postgres said (`pairs`). Rows are not tied to
 * the id asked, so an id is a match when its expected keyed id is among them. A non-v4 id left over is paired with a keyed
 * id nobody expected as a mismatch (geo-db keys it differently), else it is unmapped; a keyed id left over after that is a
 * conflict. A v4 id left over is a mismatch, as it always answers as itself.
 */
export function compareLegacyRows(pairs: readonly IdPair[], keyed: readonly string[]): TwinVerdict {
	const answered = new Set(keyed)
	const expected = new Set(pairs.map((p) => p.geo))
	const verdict: TwinVerdict = {match: 0, mismatch: 0, unmapped_in_geodb: 0, conflict: 0, mismatched: []}
	const unmatched: IdPair[] = []
	for (const pair of pairs) {
		if (answered.has(pair.geo)) verdict.match++
		else if (pair.geo === pair.gaia) {
			verdict.mismatch++
			verdict.mismatched.push(pair.gaia)
		} else unmatched.push(pair)
	}
	let unexpected = [...answered].filter((id) => !expected.has(id)).length
	for (const pair of unmatched) {
		if (unexpected > 0) {
			unexpected--
			verdict.mismatch++
			verdict.mismatched.push(pair.gaia)
		} else verdict.unmapped_in_geodb++
	}
	verdict.conflict = unexpected
	return verdict
}

// --- the twin ------------------------------------------------------------------------------------------------

type Log = {warn: (message: string, fields?: Record<string, unknown>) => void}

export type IdMapTwinDeps = {
	client: Pick<GeoDbCardClient, "readLegacyIds">
	config: IdMapTwinConfig
	log: Log
	now?: () => number
	random?: () => number
}

/**
 * Wrap `inner` (the Postgres map) so each answer it gives is also checked against geo-db, in the background. The wrapper
 * returns `inner`'s answer untouched and as soon as `inner` does: the check is started after it, never awaited, and cannot
 * throw into the caller. `inner` unwrapped when the twin is off or the client cannot read legacy ids.
 */
export function withIdMapTwin(inner: GaiaIdsOf, deps: IdMapTwinDeps): GaiaIdsOf {
	const {client, config, log} = deps
	if (!config.enabled) return inner
	const reader = client.readLegacyIds?.bind(client)
	if (!reader) {
		log.warn("GEODB_ID_MAP_TWIN is on but the geo-db client cannot read legacy ids; twin not started")
		return inner
	}
	const read = reader
	const now = deps.now ?? Date.now
	const random = deps.random ?? Math.random
	active = true
	let inFlight = false
	let coolUntil = Number.NEGATIVE_INFINITY

	async function check(pairs: IdPair[]): Promise<void> {
		const start = performance.now()
		try {
			const rows = await read({originalIds: pairs.map((p) => p.gaia), timeoutMs: config.timeoutMs})
			readsOk++
			readSeconds += (performance.now() - start) / 1000
			const verdict = compareLegacyRows(
				pairs,
				rows.flatMap((r) => (r.entity ? [r.entity] : [])),
			)
			count("match", verdict.match)
			count("mismatch", verdict.mismatch)
			count("unmapped_in_geodb", verdict.unmapped_in_geodb)
			count("conflict", verdict.conflict)
			if (verdict.mismatch > 0 || verdict.conflict > 0) {
				log.warn("geodb id map twin disagrees with Postgres", {
					event: "geodb_id_map_twin_mismatch",
					mismatch: verdict.mismatch,
					conflict: verdict.conflict,
					gaiaIds: verdict.mismatched.slice(0, 5),
				})
			}
		} catch (error) {
			readsFailed++
			readSeconds += (performance.now() - start) / 1000
			count("failed", pairs.length)
			coolUntil = now() + config.cooldownMs
			log.warn("geodb id map twin read failed", {
				event: "geodb_id_map_twin_failed",
				reason: (error as {reason?: string})?.reason ?? "error",
				message: String((error as Error)?.message ?? error).slice(0, 200),
			})
		} finally {
			inFlight = false
		}
	}

	return async (geoIds) => {
		const answer = await inner(geoIds)
		try {
			if (answer.size === 0) return answer
			if (now() < coolUntil) skip("cooldown")
			else if (inFlight) skip("busy")
			else if (random() >= config.sample) skip("sampled_out")
			else {
				const seen = new Set<string>()
				const pairs: IdPair[] = []
				for (const [geo, gaia] of answer) {
					if (seen.has(gaia)) continue
					seen.add(gaia)
					pairs.push({geo, gaia})
					if (pairs.length === MAX_CARD_IDS) break
				}
				inFlight = true
				void check(pairs)
			}
		} catch {
			// The twin never fails the request.
		}
		return answer
	}
}
