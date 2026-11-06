/**
 * `gaia_api_governance_shadow_*`: the proposal-status shadow's counters, in the style of gateway/metrics.ts. Empty until
 * the shadow is mounted (`activateGovernanceMetrics`), so a flag-off deploy exports nothing; once active every series
 * exists at zero.
 */
export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const UNSUPPORTED_REASONS = ["system_space_missing", "too_many_voters", "ambiguous_record", "busy"] as const
export const UNCOMPARED_INPUTS = [
	"snapshot_from_postgres",
	"total_editors_from_postgres",
	"votes_from_postgres",
] as const
export const SURFACES = ["detail", "list"] as const

export type Result = (typeof RESULTS)[number]
export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
export type UncomparedInput = (typeof UNCOMPARED_INPUTS)[number]
export type Surface = (typeof SURFACES)[number]

let active = false
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const uncompared = new Map<string, number>()

const bump = (map: Map<string, number>, key: string): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}

export const activateGovernanceMetrics = (): void => {
	active = true
}
export const recordGovernanceShadow = (surface: Surface, result: Result): void => bump(results, `${surface}\t${result}`)
export const recordGovernanceUnsupported = (surface: Surface, reason: UnsupportedReason): void =>
	bump(unsupported, `${surface}\t${reason}`)
export const recordGovernanceUncompared = (surface: Surface, input: UncomparedInput): void =>
	bump(uncompared, `${surface}\t${input}`)

export function renderGovernanceMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_governance_shadow_total Proposal status computed from geo-db records against the status Postgres served, by surface and result: match, mismatch, unsupported (geo-db cannot answer) or failed (geo-db read failed).",
		"# TYPE gaia_api_governance_shadow_total counter",
	]
	for (const s of SURFACES)
		for (const r of RESULTS)
			lines.push(
				`gaia_api_governance_shadow_total{surface="${s}",result="${r}"} ${results.get(`${s}\t${r}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_governance_shadow_unsupported_total Governance shadow comparisons geo-db could not answer, by surface and reason.",
		"# TYPE gaia_api_governance_shadow_unsupported_total counter",
	)
	for (const s of SURFACES)
		for (const r of UNSUPPORTED_REASONS)
			lines.push(
				`gaia_api_governance_shadow_unsupported_total{surface="${s}",reason="${r}"} ${unsupported.get(`${s}\t${r}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_governance_shadow_uncompared_total Status inputs a comparison took from Postgres instead of geo-db, by surface and input. A match or mismatch covers only the inputs geo-db supplied.",
		"# TYPE gaia_api_governance_shadow_uncompared_total counter",
	)
	for (const s of SURFACES)
		for (const i of UNCOMPARED_INPUTS)
			lines.push(
				`gaia_api_governance_shadow_uncompared_total{surface="${s}",input="${i}"} ${uncompared.get(`${s}\t${i}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetGovernanceMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
	uncompared.clear()
}
