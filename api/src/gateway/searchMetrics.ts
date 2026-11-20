/**
 * Prometheus series for the gateway's search shadows. Search results are ranked lists, so they are not judged
 * equal or different: each comparison records how much of PostGraphile's top ten geo-db also ranks, and how
 * many results each side returned. Rendered after the other gateway series (metrics.ts); empty until a search
 * shadow is mounted.
 */

/** Share of PostGraphile's top ten found in geo-db's answer, bucketed. */
export const OVERLAP_BUCKETS = ["0", "1-49", "50-99", "100"] as const
export const RESULT_COUNT_BUCKETS = ["0", "1-4", "5-9", "10-19", "20+"] as const
/** What a completed comparison found: both sides had results (`compared`) or one or both were empty. */
export const SEARCH_OUTCOMES = ["compared", "both_empty", "postgres_empty", "geodb_empty"] as const
export const TOP1_RESULTS = ["same", "different"] as const
/**
 * The judged verdict of a compared search. Top1 and overlap buckets alone call a ranking difference a miss, and
 * geo-db's lexical search differs from gaia's on six counted points (geo-db ISS-565-842), most of them ranking:
 * a recorded difference must not read as degraded.
 */
export const SEARCH_VERDICTS = ["ok", "degraded"] as const
export type SearchVerdict = (typeof SEARCH_VERDICTS)[number]
/** Fewest of PostGraphile's top ten that geo-db's top twenty must hold for the answer to be judged ok without the first result. */
export const OK_MIN_RECALL = 0.3
/** The shadow's window: how many of PostGraphile's results are checked, and how deep geo-db is asked. */
export const OVERLAP_K = 10
export const GEODB_LIMIT = 20

/** What the type exclusion did to a search: applied (hits read and filtered), unavailable (no card reader, or more Spaces than a line chunks), error (a card read failed). */
export const TYPE_FILTER_RESULTS = ["applied", "unavailable", "error"] as const
export type TypeFilterResult = (typeof TYPE_FILTER_RESULTS)[number]
/** Hits geo-db is asked for when the type exclusion is on: enough that dropping excluded types still leaves GEODB_LIMIT. */
export const GEODB_OVERFETCH = 60

export type OverlapBucket = (typeof OVERLAP_BUCKETS)[number]
export type ResultCountBucket = (typeof RESULT_COUNT_BUCKETS)[number]
export type SearchOutcome = (typeof SEARCH_OUTCOMES)[number]
export type SearchSource = "postgres" | "geodb"

const fields = new Set<string>()
const counts = new Map<string, number>()
const bump = (key: string) => counts.set(key, (counts.get(key) ?? 0) + 1)

export function activateSearchMetrics(field: string): void {
	fields.add(field)
}

/** Why a search shadow's geo-db read failed: the GeoDbError reason (timeout, overloaded, client_failed, ...) or `error`. */
export function recordSearchFailure(field: string, reason: string): void {
	bump(`failure\t${field}\t${/^[a-z_]{1,32}$/.test(reason) ? reason : "error"}`)
}

export function recordTypeFilter(field: string, result: TypeFilterResult): void {
	bump(`typefilter\t${field}\t${result}`)
}

export const overlapBucket = (ratio: number): OverlapBucket =>
	ratio >= 1 ? "100" : ratio >= 0.5 ? "50-99" : ratio > 0 ? "1-49" : "0"

export const resultCountBucket = (n: number): ResultCountBucket =>
	n === 0 ? "0" : n < 5 ? "1-4" : n < 10 ? "5-9" : n < 20 ? "10-19" : "20+"

/** Both lists best first, in one id space. `ratio`s are over PostGraphile's first OVERLAP_K ids. */
/**
 * The rule, over a compared search (both sides had results). ok when PostGraphile's first result is inside geo-db's
 * top twenty, or at least OK_MIN_RECALL (3 of 10) of PostGraphile's top ten are. degraded otherwise.
 *
 * Why: ISS-565-842 attributes 212 of 509 missed top-ten entities to ranking (the name has the query tokens but lost
 * on score: no entity or Space prior, point 6; exact-name and coverage boosts, point 3) and the rest to fuzzy,
 * prefix, stem and description matches. A different top1 that is still inside the top twenty, or a partial overlap,
 * is that known ranking difference, so it is ok. Degraded is a disagreement the ranking difference cannot explain:
 * PostGraphile's first result absent from geo-db's top twenty and fewer than three of its top ten present.
 */
export function judgeCompared(input: {top1Present: boolean; recall: number}): SearchVerdict {
	return input.top1Present || input.recall >= OK_MIN_RECALL ? "ok" : "degraded"
}

export function compareRanked(
	postgres: readonly string[],
	geodb: readonly string[],
): {
	outcome: SearchOutcome
	top10Overlap?: number
	recallInGeoDb?: number
	top1Same?: boolean
	verdict?: SearchVerdict
} {
	if (postgres.length === 0) return {outcome: geodb.length === 0 ? "both_empty" : "postgres_empty"}
	if (geodb.length === 0) return {outcome: "geodb_empty"}
	const top = postgres.slice(0, OVERLAP_K)
	const geoTop = new Set(geodb.slice(0, OVERLAP_K))
	const geoAll = new Set(geodb.slice(0, GEODB_LIMIT))
	const recall = top.filter((id) => geoAll.has(id)).length / top.length
	return {
		outcome: "compared",
		verdict: judgeCompared({top1Present: geoAll.has(postgres[0] as string), recall}),
		top10Overlap: top.filter((id) => geoTop.has(id)).length / top.length,
		recallInGeoDb: top.filter((id) => geoAll.has(id)).length / top.length,
		top1Same: postgres[0] === geodb[0],
	}
}

export function recordSearchComparison(
	field: string,
	postgres: readonly string[],
	geodb: readonly string[],
): ReturnType<typeof compareRanked> {
	const verdict = compareRanked(postgres, geodb)
	bump(`outcome\t${field}\t${verdict.outcome}`)
	bump(`results\t${field}\tpostgres\t${resultCountBucket(postgres.length)}`)
	bump(`results\t${field}\tgeodb\t${resultCountBucket(geodb.length)}`)
	if (verdict.outcome === "compared") {
		bump(`overlap\t${field}\ttop10\t${overlapBucket(verdict.top10Overlap ?? 0)}`)
		bump(`overlap\t${field}\trecall_top20\t${overlapBucket(verdict.recallInGeoDb ?? 0)}`)
		bump(`top1\t${field}\t${verdict.top1Same ? "same" : "different"}`)
		bump(`judged\t${field}\t${verdict.verdict ?? "degraded"}`)
	}
	return verdict
}

export function renderSearchMetrics(): string {
	if (fields.size === 0) return ""
	const get = (key: string) => counts.get(key) ?? 0
	const lines = [
		"# HELP gaia_api_gateway_shadow_search_total Completed search shadow comparisons by root field and outcome: compared (both sides had results) or an empty side.",
		"# TYPE gaia_api_gateway_shadow_search_total counter",
	]
	for (const f of fields)
		for (const o of SEARCH_OUTCOMES)
			lines.push(
				`gaia_api_gateway_shadow_search_total{field="${f}",outcome="${o}"} ${get(`outcome\t${f}\t${o}`)}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_search_overlap_total Compared searches by how much of PostGraphile's top ten geo-db ranks: kind top10 (inside geo-db's top ten) or recall_top20 (inside geo-db's top twenty), by share bucket.",
		"# TYPE gaia_api_gateway_shadow_search_overlap_total counter",
	)
	for (const f of fields)
		for (const kind of ["top10", "recall_top20"])
			for (const b of OVERLAP_BUCKETS)
				lines.push(
					`gaia_api_gateway_shadow_search_overlap_total{field="${f}",kind="${kind}",bucket="${b}"} ${get(`overlap\t${f}\t${kind}\t${b}`)}`,
				)
	lines.push(
		"# HELP gaia_api_gateway_shadow_search_top1_total Compared searches whose first result is the same entity on both sides.",
		"# TYPE gaia_api_gateway_shadow_search_top1_total counter",
	)
	for (const f of fields)
		for (const r of TOP1_RESULTS)
			lines.push(
				`gaia_api_gateway_shadow_search_top1_total{field="${f}",result="${r}"} ${get(`top1\t${f}\t${r}`)}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_search_judged_total Compared searches judged ok or degraded: ok when PostGraphile's first result is in geo-db's top twenty or at least 3 of its top ten are; degraded otherwise (a known ranking difference, geo-db ISS-565-842, is ok). Empty sides are not judged.",
		"# TYPE gaia_api_gateway_shadow_search_judged_total counter",
	)
	for (const f of fields)
		for (const v of SEARCH_VERDICTS)
			lines.push(
				`gaia_api_gateway_shadow_search_judged_total{field="${f}",verdict="${v}"} ${get(`judged\t${f}\t${v}`)}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_search_results_total Result counts per completed search comparison, by source (postgres, geodb) and count bucket.",
		"# TYPE gaia_api_gateway_shadow_search_results_total counter",
	)
	for (const f of fields)
		for (const s of ["postgres", "geodb"])
			for (const b of RESULT_COUNT_BUCKETS)
				lines.push(
					`gaia_api_gateway_shadow_search_results_total{field="${f}",source="${s}",bucket="${b}"} ${get(`results\t${f}\t${s}\t${b}`)}`,
				)
	lines.push(
		"# HELP gaia_api_gateway_shadow_search_type_filter_total Search shadows' type exclusion (Postgres's default excluded types and Claim) by result: applied, unavailable (not attempted) or error (a card read failed; the comparison is skipped, not counted failed).",
		"# TYPE gaia_api_gateway_shadow_search_type_filter_total counter",
	)
	for (const f of fields)
		for (const r of TYPE_FILTER_RESULTS)
			lines.push(
				`gaia_api_gateway_shadow_search_type_filter_total{field="${f}",result="${r}"} ${get(`typefilter\t${f}\t${r}`)}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_search_failed_total Search shadows whose geo-db read failed, by root field and reason (the GeoDbError reason: timeout, overloaded, client_failed, unmapped_space, ...; error when not a GeoDbError).",
		"# TYPE gaia_api_gateway_shadow_search_failed_total counter",
	)
	for (const f of fields)
		for (const key of [...counts.keys()].filter((k) => k.startsWith(`failure\t${f}\t`)).sort())
			lines.push(
				`gaia_api_gateway_shadow_search_failed_total{field="${f}",reason="${key.split("\t")[2]}"} ${get(key)}`,
			)
	return `${lines.join("\n")}\n`
}

/** Reset accumulated state. Tests only. */
export function __resetSearchMetricsForTests(): void {
	fields.clear()
	counts.clear()
}
