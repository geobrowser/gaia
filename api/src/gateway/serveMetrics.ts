/**
 * Prometheus metrics for GraphQL root fields served from geo-db (field mode "on"), in the text format
 * `/health/metrics` serves. The counterpart of the card route's gaia_api_geodb_card_served_total /
 * _fallback_total, per root field.
 *
 * Empty until a field is served (`activateServeMetrics`), so a deploy with every field off or in shadow
 * exports nothing. Once a field is active, each of its series is registered at zero so `increase()` sees the
 * first event after a deploy.
 */

export const SERVE_SOURCES = ["geodb", "postgres"] as const

/**
 * Why an operation for a served field was answered by PostgreSQL instead. A closed set, so it is safe as a
 * label. The first group is the resolver's own verdict on the query (geo-db could not answer it exactly); the
 * second is geo-db failing a query it could have answered.
 */
export const SERVE_FALLBACK_REASONS = [
	// The query asks for something the translator cannot answer exactly.
	"not_single_root",
	"args",
	"directives",
	"variables",
	"selection",
	"filter",
	"no_space",
	"many_spaces",
	"truncated",
	"unmapped_space",
	"legacy_id",
	"empty",
	// geo-db could not answer in time, or at all.
	"timeout",
	"overloaded",
	"client_failed",
	"bad_request",
	"error",
] as const

export type ServeSource = (typeof SERVE_SOURCES)[number]
export type ServeFallbackReason = (typeof SERVE_FALLBACK_REASONS)[number]

/** Upper bounds in seconds of the time a served field spends deciding and reading geo-db. */
export const SERVE_BUCKETS_SECONDS = [0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1] as const

type Outcome = "served" | "fallback"

const fields = new Set<string>()
const served = new Map<string, number>()
const fallbacks = new Map<string, number>()
const buckets = new Map<string, number[]>()
const sums = new Map<string, number>()

const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

export const asServeFallbackReason = (reason: string): ServeFallbackReason =>
	(SERVE_FALLBACK_REASONS as readonly string[]).includes(reason) ? (reason as ServeFallbackReason) : "error"

export function activateServeMetrics(field: string): void {
	fields.add(field)
}

export function recordServeSource(field: string, source: ServeSource): void {
	bump(served, `${field}\t${source}`)
}

export function recordServeFallback(field: string, reason: string): void {
	bump(fallbacks, `${field}\t${asServeFallbackReason(reason)}`)
}

/** The gateway's time on a served field before it answered or gave up: planning, the geo-db read, rendering. */
export function observeServe(field: string, seconds: number, outcome: Outcome): void {
	const key = `${field}\t${outcome}`
	const counts = buckets.get(key) ?? new Array<number>(SERVE_BUCKETS_SECONDS.length + 1).fill(0)
	const index = SERVE_BUCKETS_SECONDS.findIndex((edge) => seconds <= edge)
	const slot = index === -1 ? SERVE_BUCKETS_SECONDS.length : index
	counts[slot] = (counts[slot] ?? 0) + 1
	buckets.set(key, counts)
	sums.set(key, (sums.get(key) ?? 0) + seconds)
}

export function renderServeMetrics(): string {
	if (fields.size === 0) return ""
	const lines = [
		"# HELP gaia_api_gateway_served_total GraphQL operations of a served root field (mode on) by the store that answered them.",
		"# TYPE gaia_api_gateway_served_total counter",
	]
	for (const field of fields)
		for (const source of SERVE_SOURCES)
			lines.push(
				`gaia_api_gateway_served_total{field="${field}",source="${source}"} ${served.get(`${field}\t${source}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_serve_fallback_total Operations of a served root field answered by PostgreSQL instead of geo-db, by field and reason.",
		"# TYPE gaia_api_gateway_serve_fallback_total counter",
	)
	for (const field of fields)
		for (const reason of SERVE_FALLBACK_REASONS)
			lines.push(
				`gaia_api_gateway_serve_fallback_total{field="${field}",reason="${reason}"} ${fallbacks.get(`${field}\t${reason}`) ?? 0}`,
			)
	const name = "gaia_api_gateway_serve_seconds"
	lines.push(
		`# HELP ${name} Time the gateway spent on a served root field before answering from geo-db (served) or handing the operation to PostgreSQL (fallback; the PostgreSQL read is not included).`,
		`# TYPE ${name} histogram`,
	)
	for (const field of fields)
		for (const outcome of ["served", "fallback"] as const) {
			const key = `${field}\t${outcome}`
			const counts = buckets.get(key) ?? new Array<number>(SERVE_BUCKETS_SECONDS.length + 1).fill(0)
			let cumulative = 0
			SERVE_BUCKETS_SECONDS.forEach((edge, i) => {
				cumulative += counts[i] ?? 0
				lines.push(`${name}_bucket{field="${field}",outcome="${outcome}",le="${edge}"} ${cumulative}`)
			})
			cumulative += counts[SERVE_BUCKETS_SECONDS.length] ?? 0
			lines.push(`${name}_bucket{field="${field}",outcome="${outcome}",le="+Inf"} ${cumulative}`)
			lines.push(`${name}_sum{field="${field}",outcome="${outcome}"} ${sums.get(key) ?? 0}`)
			lines.push(`${name}_count{field="${field}",outcome="${outcome}"} ${cumulative}`)
		}
	return `${lines.join("\n")}\n`
}

/** Reset accumulated state. Tests only. */
export function __resetServeMetricsForTests(): void {
	fields.clear()
	served.clear()
	fallbacks.clear()
	buckets.clear()
	sums.clear()
}
