/**
 * In-process counters for the geo-db twin, rendered as Prometheus text.
 *
 * The executor is a one-shot CronJob with no scrape endpoint, so these live for one run and leave it as the
 * `geodb_twin_summary` log line (see twin.ts), which a log-based metric counts. The names and labels are the ones a
 * scrape would use if the executor ever gains an endpoint.
 */

export type TwinResult = "match" | "mismatch" | "unsupported" | "failed"
export type MismatchKind = "extra_in_geodb" | "missing_in_geodb"

const compared = new Map<TwinResult, number>()
const mismatches = new Map<MismatchKind, number>()
const unsupportedBy = new Map<string, number>()
const runs = new Map<string, number>()

const bump = <K>(m: Map<K, number>, k: K, by = 1) => m.set(k, (m.get(k) ?? 0) + by)

export const recordCompared = (result: TwinResult) => bump(compared, result)
export const recordMismatch = (kind: MismatchKind) => bump(mismatches, kind)
export const recordUnsupported = (reason: string) => bump(unsupportedBy, reason)
/** `completed` or one of `aborted_failure`, `aborted_budget`, `skipped_no_candidates`, `error`. */
export const recordRun = (outcome: string) => bump(runs, outcome)

export function snapshot() {
	return {
		compared: Object.fromEntries(compared),
		mismatches: Object.fromEntries(mismatches),
		unsupported: Object.fromEntries(unsupportedBy),
		runs: Object.fromEntries(runs),
	}
}

export function resetTwinMetrics() {
	compared.clear()
	mismatches.clear()
	unsupportedBy.clear()
	runs.clear()
}

export function renderTwinMetrics(): string {
	const lines: string[] = []
	const family = (name: string, help: string, label: string, m: Map<string, number>) => {
		lines.push(`# HELP ${name} ${help}`, `# TYPE ${name} counter`)
		for (const [k, v] of [...m.entries()].sort()) lines.push(`${name}{${label}="${k}"} ${v}`)
	}
	family(
		"proposal_executor_geodb_twin_compared_total",
		"Candidate proposals compared with the SQL ready set, by result.",
		"result",
		compared,
	)
	family(
		"proposal_executor_geodb_twin_mismatch_total",
		"Proposals whose geo-db ready verdict differs from SQL, by direction.",
		"kind",
		mismatches,
	)
	family(
		"proposal_executor_geodb_twin_unsupported_total",
		"Proposals geo-db records could not decide, by reason.",
		"reason",
		unsupportedBy,
	)
	family("proposal_executor_geodb_twin_runs_total", "Twin runs, by outcome.", "outcome", runs)
	return `${lines.join("\n")}\n`
}
