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
/** The shadow's window: how many of PostGraphile's results are checked, and how deep geo-db is asked. */
export const OVERLAP_K = 10
export const GEODB_LIMIT = 20

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

export const overlapBucket = (ratio: number): OverlapBucket =>
	ratio >= 1 ? "100" : ratio >= 0.5 ? "50-99" : ratio > 0 ? "1-49" : "0"

export const resultCountBucket = (n: number): ResultCountBucket =>
	n === 0 ? "0" : n < 5 ? "1-4" : n < 10 ? "5-9" : n < 20 ? "10-19" : "20+"

/** Both lists best first, in one id space. `ratio`s are over PostGraphile's first OVERLAP_K ids. */
export function compareRanked(
	postgres: readonly string[],
	geodb: readonly string[],
): {
	outcome: SearchOutcome
	top10Overlap?: number
	recallInGeoDb?: number
	top1Same?: boolean
} {
	if (postgres.length === 0) return {outcome: geodb.length === 0 ? "both_empty" : "postgres_empty"}
	if (geodb.length === 0) return {outcome: "geodb_empty"}
	const top = postgres.slice(0, OVERLAP_K)
	const geoTop = new Set(geodb.slice(0, OVERLAP_K))
	const geoAll = new Set(geodb.slice(0, GEODB_LIMIT))
	return {
		outcome: "compared",
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
		"# HELP gaia_api_gateway_shadow_search_results_total Result counts per completed search comparison, by source (postgres, geodb) and count bucket.",
		"# TYPE gaia_api_gateway_shadow_search_results_total counter",
	)
	for (const f of fields)
		for (const s of ["postgres", "geodb"])
			for (const b of RESULT_COUNT_BUCKETS)
				lines.push(
					`gaia_api_gateway_shadow_search_results_total{field="${f}",source="${s}",bucket="${b}"} ${get(`results\t${f}\t${s}\t${b}`)}`,
				)
	return `${lines.join("\n")}\n`
}

/** Reset accumulated state. Tests only. */
export function __resetSearchMetricsForTests(): void {
	fields.clear()
	counts.clear()
}
