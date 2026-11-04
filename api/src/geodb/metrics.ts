/**
 * Prometheus metrics for the geo-db entity-card read, in the text format `/health/metrics` serves.
 *
 * Empty until the route is mounted (`activateGeoDbMetrics`), so a flag-off deploy exports nothing and
 * a removed route cannot look healthy through zero-valued series. Once active every series is
 * registered at zero, so `increase()` sees the first event after a deploy.
 */

export const FALLBACK_REASONS = [
	"timeout",
	"unmapped_space",
	"client_failed",
	"bad_request",
	"overloaded",
	"error",
] as const
export const SHADOW_RESULTS = ["match", "only_postgres", "only_geodb", "both", "failed"] as const
export const SERVED_SOURCES = ["geodb", "postgres"] as const

export type FallbackReason = (typeof FALLBACK_REASONS)[number]
export type ShadowResult = (typeof SHADOW_RESULTS)[number]
export type ServedSource = (typeof SERVED_SOURCES)[number]

/** Upper bounds in seconds; the default read budget is 0.5 s. */
export const LATENCY_BUCKETS_SECONDS = [0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1] as const

type Outcome = "ok" | "error"

let active = false
const served = new Map<string, number>()
const fallbacks = new Map<string, number>()
const shadow = new Map<string, number>()
const buckets = new Map<string, number[]>()
const sums = new Map<string, number>()
const pgBuckets = new Map<string, number[]>()
const pgSums = new Map<string, number>()

const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

export function activateGeoDbMetrics(): void {
	active = true
}

export const recordServed = (source: ServedSource) => bump(served, source)

export function recordFallback(reason: string): void {
	bump(fallbacks, (FALLBACK_REASONS as readonly string[]).includes(reason) ? reason : "error")
}

export const recordShadow = (result: ShadowResult) => bump(shadow, result)

function observe(bucketMap: Map<string, number[]>, sumMap: Map<string, number>, seconds: number, outcome: Outcome) {
	const counts = bucketMap.get(outcome) ?? new Array<number>(LATENCY_BUCKETS_SECONDS.length + 1).fill(0)
	const index = LATENCY_BUCKETS_SECONDS.findIndex((edge) => seconds <= edge)
	const slot = index === -1 ? LATENCY_BUCKETS_SECONDS.length : index
	counts[slot] = (counts[slot] ?? 0) + 1
	bucketMap.set(outcome, counts)
	sumMap.set(outcome, (sumMap.get(outcome) ?? 0) + seconds)
}

export const observeGeoDbRead = (seconds: number, outcome: Outcome) => observe(buckets, sums, seconds, outcome)

/** The Postgres read of the same card in shadow mode, on the same buckets, so the two histograms compare. */
export const observePostgresCardRead = (seconds: number, outcome: Outcome) => observe(pgBuckets, pgSums, seconds, outcome)

function histogram(name: string, help: string, bucketMap: Map<string, number[]>, sumMap: Map<string, number>) {
	const lines = [`# HELP ${name} ${help}`, `# TYPE ${name} histogram`]
	for (const outcome of ["ok", "error"] as const) {
		const counts = bucketMap.get(outcome) ?? new Array<number>(LATENCY_BUCKETS_SECONDS.length + 1).fill(0)
		let cumulative = 0
		LATENCY_BUCKETS_SECONDS.forEach((edge, i) => {
			cumulative += counts[i] ?? 0
			lines.push(`${name}_bucket{outcome="${outcome}",le="${edge}"} ${cumulative}`)
		})
		cumulative += counts[LATENCY_BUCKETS_SECONDS.length] ?? 0
		lines.push(`${name}_bucket{outcome="${outcome}",le="+Inf"} ${cumulative}`)
		lines.push(`${name}_sum{outcome="${outcome}"} ${sumMap.get(outcome) ?? 0}`)
		lines.push(`${name}_count{outcome="${outcome}"} ${cumulative}`)
	}
	return lines
}

function counter(name: string, help: string, label: string, keys: readonly string[], map: Map<string, number>) {
	const lines = [`# HELP ${name} ${help}`, `# TYPE ${name} counter`]
	for (const key of keys) lines.push(`${name}{${label}="${key}"} ${map.get(key) ?? 0}`)
	return lines
}

export function renderGeoDbCardMetrics(): string {
	if (!active) return ""
	const lines = [
		...counter(
			"gaia_api_geodb_card_served_total",
			"Entity-card responses by the store that produced them (mode on and shadow).",
			"source",
			SERVED_SOURCES,
			served,
		),
		...counter(
			"gaia_api_geodb_card_fallback_total",
			"Mode-on card reads answered from Postgres because geo-db failed, by reason.",
			"reason",
			FALLBACK_REASONS,
			fallbacks,
		),
		...counter(
			"gaia_api_geodb_card_shadow_total",
			"Mode-shadow comparisons by result: match, rows only Postgres has, rows only geo-db has, both, or geo-db failed.",
			"result",
			SHADOW_RESULTS,
			shadow,
		),
		...histogram(
			"gaia_api_geodb_read_seconds",
			"Latency of one geo-db card read through the pipe pool, queue wait included.",
			buckets,
			sums,
		),
		...histogram(
			"gaia_api_postgres_card_read_seconds",
			"Latency of the Postgres read of one entity card in shadow mode, the counterpart of gaia_api_geodb_read_seconds.",
			pgBuckets,
			pgSums,
		),
	]
	return `${lines.join("\n")}\n`
}

/** Reset accumulated state. Tests only. */
export function __resetGeoDbMetricsForTests(): void {
	active = false
	for (const m of [served, fallbacks, shadow, buckets, sums, pgBuckets, pgSums] as Map<string, unknown>[]) m.clear()
}
