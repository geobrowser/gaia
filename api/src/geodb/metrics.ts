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
/** Why a `geodb-client pipe` worker started: a read found no worker, or the pool replaced one ahead of need. */
export const WORKER_START_REASONS = ["demand", "ahead"] as const
/**
 * Why a worker ended: its connection or token died under a read, a real refusal, it exited, a read timed out,
 * it broke the protocol, or the pool retired it before its token expired.
 */
export const WORKER_END_REASONS = ["dead_session", "refusal", "exit", "timeout", "broken", "recycled"] as const

export type FallbackReason = (typeof FALLBACK_REASONS)[number]
export type ShadowResult = (typeof SHADOW_RESULTS)[number]
export type ServedSource = (typeof SERVED_SOURCES)[number]
export type WorkerStartReason = (typeof WORKER_START_REASONS)[number]
export type WorkerEndReason = (typeof WORKER_END_REASONS)[number]

/**
 * Upper bounds in seconds; the default read budget is 0.5 s. Dense between 1 and 5 ms, where the warm card
 * reads of both stores sit, so a p50 is not interpolated inside one bucket. Both histograms share it.
 */
export const LATENCY_BUCKETS_SECONDS = [
	0.001, 0.0015, 0.002, 0.003, 0.004, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1,
] as const

type Outcome = "ok" | "error"

let active = false
const served = new Map<string, number>()
const fallbacks = new Map<string, number>()
const shadow = new Map<string, number>()
const buckets = new Map<string, number[]>()
const sums = new Map<string, number>()
const pgBuckets = new Map<string, number[]>()
const pgSums = new Map<string, number>()
const workerStarts = new Map<string, number>()
const workerEnds = new Map<string, number>()

const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

export function activateGeoDbMetrics(): void {
	active = true
	startLoopLag()
}

/**
 * The stages of one pipe read, by shape (`one` or `several` Spaces on the line, with or without `names`):
 * `wait` from submit until the line is written (queue and worker start), `answer` from the write until the
 * whole frame is in (the connection, the serve, and any delay in the event loop delivering the reply), and
 * `parse` turning the frame into rows.
 */
export const READ_STAGES = ["wait", "answer", "parse"] as const
export const READ_SHAPES = ["one", "one_names", "several", "several_names"] as const
export type ReadStage = (typeof READ_STAGES)[number]
export type ReadShape = (typeof READ_SHAPES)[number]
const stageBuckets = new Map<string, number[]>()
const stageSums = new Map<string, number>()

export function observeReadStage(stage: ReadStage, shape: ReadShape, seconds: number): void {
	observe(stageBuckets, stageSums, seconds, `stage="${stage}",shape="${shape}"`)
}

/** Event-loop lag: how late a 100 ms timer fires. A late loop delays every reply the pipe delivers. */
const LOOP_LAG_INTERVAL_MS = 100
const lagBuckets = new Map<string, number[]>()
const lagSums = new Map<string, number>()
let lagTimer: ReturnType<typeof setInterval> | null = null

function startLoopLag() {
	if (lagTimer) return
	let expected = performance.now() + LOOP_LAG_INTERVAL_MS
	lagTimer = setInterval(() => {
		const t = performance.now()
		observe(lagBuckets, lagSums, Math.max(0, t - expected) / 1000, "")
		expected = t + LOOP_LAG_INTERVAL_MS
	}, LOOP_LAG_INTERVAL_MS)
	lagTimer.unref?.()
}

export const recordServed = (source: ServedSource) => bump(served, source)

export function recordFallback(reason: string): void {
	bump(fallbacks, (FALLBACK_REASONS as readonly string[]).includes(reason) ? reason : "error")
}

export const recordShadow = (result: ShadowResult) => bump(shadow, result)
export const recordWorkerStart = (reason: WorkerStartReason) => bump(workerStarts, reason)
export const recordWorkerEnd = (reason: WorkerEndReason) => bump(workerEnds, reason)

function observe(bucketMap: Map<string, number[]>, sumMap: Map<string, number>, seconds: number, outcome: string) {
	const counts = bucketMap.get(outcome) ?? new Array<number>(LATENCY_BUCKETS_SECONDS.length + 1).fill(0)
	const index = LATENCY_BUCKETS_SECONDS.findIndex((edge) => seconds <= edge)
	const slot = index === -1 ? LATENCY_BUCKETS_SECONDS.length : index
	counts[slot] = (counts[slot] ?? 0) + 1
	bucketMap.set(outcome, counts)
	sumMap.set(outcome, (sumMap.get(outcome) ?? 0) + seconds)
}

export const observeGeoDbRead = (seconds: number, outcome: Outcome) => observe(buckets, sums, seconds, outcome)

/** The Postgres read of the same card in shadow mode, on the same buckets, so the two histograms compare. */
export const observePostgresCardRead = (seconds: number, outcome: Outcome) =>
	observe(pgBuckets, pgSums, seconds, outcome)

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

/** A histogram whose series are keyed by a ready-made label list (`a="x",b="y"`, or empty for none). */
function keyedHistogram(
	name: string,
	help: string,
	keys: string[],
	bucketMap: Map<string, number[]>,
	sumMap: Map<string, number>,
) {
	const lines = [`# HELP ${name} ${help}`, `# TYPE ${name} histogram`]
	for (const key of keys) {
		const counts = bucketMap.get(key) ?? new Array<number>(LATENCY_BUCKETS_SECONDS.length + 1).fill(0)
		const sep = key ? "," : ""
		let cumulative = 0
		LATENCY_BUCKETS_SECONDS.forEach((edge, i) => {
			cumulative += counts[i] ?? 0
			lines.push(`${name}_bucket{${key}${sep}le="${edge}"} ${cumulative}`)
		})
		cumulative += counts[LATENCY_BUCKETS_SECONDS.length] ?? 0
		lines.push(`${name}_bucket{${key}${sep}le="+Inf"} ${cumulative}`)
		lines.push(`${name}_sum${key ? `{${key}}` : ""} ${sumMap.get(key) ?? 0}`)
		lines.push(`${name}_count${key ? `{${key}}` : ""} ${cumulative}`)
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
		...counter(
			"gaia_api_geodb_worker_starts_total",
			"geodb-client pipe workers started: demand (a read found none) or ahead (replaced before a read needed it).",
			"reason",
			WORKER_START_REASONS,
			workerStarts,
		),
		...counter(
			"gaia_api_geodb_worker_ends_total",
			"geodb-client pipe workers ended, by reason.",
			"reason",
			WORKER_END_REASONS,
			workerEnds,
		),
		...histogram(
			"gaia_api_geodb_read_seconds",
			"Latency of one geo-db card read through the pipe pool, queue wait included.",
			buckets,
			sums,
		),
		...keyedHistogram(
			"gaia_api_geodb_read_stage_seconds",
			"One geo-db pipe read by stage (wait: queue and worker start; answer: write to whole reply; parse) and shape.",
			READ_STAGES.flatMap((stage) => READ_SHAPES.map((shape) => `stage="${stage}",shape="${shape}"`)),
			stageBuckets,
			stageSums,
		),
		...keyedHistogram(
			"gaia_api_event_loop_lag_seconds",
			"How late a 100 ms timer fires on the api's event loop.",
			[""],
			lagBuckets,
			lagSums,
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
	if (lagTimer) clearInterval(lagTimer)
	lagTimer = null
	for (const m of [
		served,
		fallbacks,
		shadow,
		buckets,
		sums,
		pgBuckets,
		pgSums,
		workerStarts,
		workerEnds,
		stageBuckets,
		stageSums,
		lagBuckets,
		lagSums,
	] as Map<string, unknown>[])
		m.clear()
}
