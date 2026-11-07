/**
 * `gaia_api_governance_queries_shadow_*`: counters of the proposal queries shadow (queriesShadow.ts), in the style of
 * metrics.ts. Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists
 * at zero. Counts only: no id, no query text and no record content is ever a label.
 */
export const QUERY_RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
/** Which read was twinned: the detail's actions and votes, the list's actions and filter, the two active checks. */
export const QUERIES = ["detail", "list", "active_member", "active_editor"] as const
export const QUERY_UNSUPPORTED_REASONS = [
	"system_space_missing",
	"backlinks_unavailable",
	"version_missing",
	"too_many_records",
	"too_many_candidates",
	"ambiguous_record",
	"status_inputs_not_read",
	"busy",
] as const
export const QUERY_UNCOMPARED_INPUTS = [
	"list_membership_from_postgres",
	"list_order_and_cursor_from_postgres",
	"list_status_filter_from_postgres",
	"list_vote_counts_from_postgres",
	"action_settings_fields_from_postgres",
	"active_space_scope_from_postgres",
	"active_vote_counts_from_postgres",
] as const

export type QueryResult = (typeof QUERY_RESULTS)[number]
export type Query = (typeof QUERIES)[number]
export type QueryUnsupportedReason = (typeof QUERY_UNSUPPORTED_REASONS)[number]
export type QueryUncomparedInput = (typeof QUERY_UNCOMPARED_INPUTS)[number]

let active = false
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const uncompared = new Map<string, number>()

const bump = (map: Map<string, number>, key: string): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}

export const activateQueriesMetrics = (): void => {
	active = true
}
export const recordQueriesShadow = (query: Query, result: QueryResult): void => bump(results, `${query}\t${result}`)
export const recordQueriesUnsupported = (query: Query, reason: QueryUnsupportedReason): void =>
	bump(unsupported, `${query}\t${reason}`)
export const recordQueriesUncompared = (query: Query, input: QueryUncomparedInput): void =>
	bump(uncompared, `${query}\t${input}`)

export function renderQueriesMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_governance_queries_shadow_total Proposal list, detail and active-check reads twinned from geo-db records against what Postgres served, by query and result: match, mismatch, unsupported (geo-db cannot answer) or failed (geo-db read failed).",
		"# TYPE gaia_api_governance_queries_shadow_total counter",
	]
	for (const q of QUERIES)
		for (const r of QUERY_RESULTS)
			lines.push(
				`gaia_api_governance_queries_shadow_total{query="${q}",result="${r}"} ${results.get(`${q}\t${r}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_governance_queries_shadow_unsupported_total Queries shadow comparisons geo-db could not answer, by query and reason.",
		"# TYPE gaia_api_governance_queries_shadow_unsupported_total counter",
	)
	for (const q of QUERIES)
		for (const r of QUERY_UNSUPPORTED_REASONS)
			lines.push(
				`gaia_api_governance_queries_shadow_unsupported_total{query="${q}",reason="${r}"} ${unsupported.get(`${q}\t${r}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_governance_queries_shadow_uncompared_total Inputs of a queries comparison taken from Postgres instead of geo-db, by query and input. A match or mismatch covers only what geo-db supplied.",
		"# TYPE gaia_api_governance_queries_shadow_uncompared_total counter",
	)
	for (const q of QUERIES)
		for (const i of QUERY_UNCOMPARED_INPUTS)
			lines.push(
				`gaia_api_governance_queries_shadow_uncompared_total{query="${q}",input="${i}"} ${uncompared.get(`${q}\t${i}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetQueriesMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
	uncompared.clear()
}
