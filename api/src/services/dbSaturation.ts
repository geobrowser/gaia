type PoolStats = {
	totalConnections: number
	idleConnections: number
	waitingCount: number
	maxConnections: number
}

export type SaturationReason = "waiting_clients" | "high_utilization" | "acquire_timeouts"

export type SaturationSnapshot = {
	isPressured: boolean
	isSaturated: boolean
	reasons: SaturationReason[]
	utilizationPercent: number
	waitingCount: number
	recentAcquireTimeouts: number
	activeSince: string | null
	lastSignalAt: string | null
}

type SaturationState = {
	firstSignalAtMs: number | null
	activeSinceMs: number | null
	lastSignalAtMs: number | null
}

function readIntEnv(name: string, defaultValue: number, minValue: number, maxValue: number): number {
	const raw = process.env[name]
	if (raw === undefined || raw.trim() === "") {
		return defaultValue
	}

	const parsed = Number(raw)
	if (!Number.isInteger(parsed) || parsed < minValue || parsed > maxValue) {
		throw new Error(
			`${name} must be an integer between ${minValue} and ${maxValue}. Received: ${JSON.stringify(raw)}`,
		)
	}

	return parsed
}

// waitingCount=1 is normal under bursty traffic; 5 is a better signal that
// requests are actually queuing up. Overridable via env for incident tuning.
const PRESSURE_WAITING_THRESHOLD = readIntEnv("PG_POOL_PRESSURE_WAITING_THRESHOLD", 5, 1, 500)
const PRESSURE_UTILIZATION_THRESHOLD = readIntEnv("PG_POOL_PRESSURE_UTILIZATION_THRESHOLD", 90, 1, 100)
const PRESSURE_TIMEOUT_THRESHOLD = readIntEnv("PG_POOL_PRESSURE_TIMEOUT_THRESHOLD", 2, 1, 500)
const SATURATION_ACTIVATION_MS = readIntEnv("PG_POOL_SATURATION_ACTIVATION_MS", 15000, 1000, 300000)
const SATURATION_RELEASE_MS = readIntEnv("PG_POOL_SATURATION_RELEASE_MS", 30000, 1000, 600000)
const ACQUIRE_TIMEOUT_WINDOW_MS = readIntEnv("PG_POOL_ACQUIRE_TIMEOUT_WINDOW_MS", 30000, 1000, 600000)
const ACQUIRE_TIMEOUT_BUCKET_MS = 1000

const perPoolState = new Map<string, SaturationState>()
const acquireTimeoutBuckets = new Map<string, Map<number, number>>()

function toIsoOrNull(valueMs: number | null): string | null {
	return valueMs === null ? null : new Date(valueMs).toISOString()
}

function getOrCreateState(poolName: string): SaturationState {
	const existing = perPoolState.get(poolName)
	if (existing) {
		return existing
	}

	const created: SaturationState = {
		firstSignalAtMs: null,
		activeSinceMs: null,
		lastSignalAtMs: null,
	}
	perPoolState.set(poolName, created)
	return created
}

function getUtilizationPercent(stats: PoolStats): number {
	if (stats.maxConnections <= 0) {
		return 0
	}

	return Math.round((stats.totalConnections / stats.maxConnections) * 100)
}

function getBucketSecond(nowMs: number): number {
	return Math.floor(nowMs / ACQUIRE_TIMEOUT_BUCKET_MS)
}

function getOrCreateTimeoutBuckets(poolName: string): Map<number, number> {
	const existing = acquireTimeoutBuckets.get(poolName)
	if (existing) {
		return existing
	}

	const created = new Map<number, number>()
	acquireTimeoutBuckets.set(poolName, created)
	return created
}

function pruneOldTimeoutBuckets(poolName: string, nowMs: number): Map<number, number> {
	const buckets = getOrCreateTimeoutBuckets(poolName)
	const oldestAllowedSecond = getBucketSecond(nowMs - ACQUIRE_TIMEOUT_WINDOW_MS)

	for (const second of buckets.keys()) {
		if (second < oldestAllowedSecond) {
			buckets.delete(second)
		}
	}

	return buckets
}

function getRecentTimeoutCount(poolName: string, nowMs: number): number {
	const buckets = pruneOldTimeoutBuckets(poolName, nowMs)
	let total = 0
	for (const count of buckets.values()) {
		total += count
	}
	return total
}

function getPressureReasons(stats: PoolStats, nowMs: number, poolName: string): SaturationReason[] {
	const reasons: SaturationReason[] = []
	const utilizationPercent = getUtilizationPercent(stats)
	const recentTimeouts = getRecentTimeoutCount(poolName, nowMs)

	if (stats.waitingCount >= PRESSURE_WAITING_THRESHOLD) {
		reasons.push("waiting_clients")
	}

	if (utilizationPercent >= PRESSURE_UTILIZATION_THRESHOLD) {
		reasons.push("high_utilization")
	}

	if (recentTimeouts >= PRESSURE_TIMEOUT_THRESHOLD) {
		reasons.push("acquire_timeouts")
	}

	return reasons
}

export function recordPoolAcquireTimeout(poolName: string, nowMs = Date.now()): void {
	const buckets = pruneOldTimeoutBuckets(poolName, nowMs)
	const second = getBucketSecond(nowMs)
	buckets.set(second, (buckets.get(second) || 0) + 1)
}

export function getRecentPoolAcquireTimeoutCount(poolName: string, nowMs = Date.now()): number {
	return getRecentTimeoutCount(poolName, nowMs)
}

export function getPoolSaturationSnapshot(poolName: string, stats: PoolStats, nowMs = Date.now()): SaturationSnapshot {
	const state = getOrCreateState(poolName)
	const reasons = getPressureReasons(stats, nowMs, poolName)
	const hasSignal = reasons.length > 0

	if (hasSignal) {
		if (state.firstSignalAtMs === null) {
			state.firstSignalAtMs = nowMs
		}
		state.lastSignalAtMs = nowMs

		if (state.activeSinceMs === null && nowMs - state.firstSignalAtMs >= SATURATION_ACTIVATION_MS) {
			state.activeSinceMs = nowMs
		}
	} else if (state.activeSinceMs === null) {
		state.firstSignalAtMs = null
	}

	if (!hasSignal && state.activeSinceMs !== null && state.lastSignalAtMs !== null) {
		if (nowMs - state.lastSignalAtMs >= SATURATION_RELEASE_MS) {
			state.activeSinceMs = null
			state.firstSignalAtMs = null
			state.lastSignalAtMs = null
		}
	}

	const recentAcquireTimeouts = getRecentPoolAcquireTimeoutCount(poolName, nowMs)

	return {
		isPressured: hasSignal,
		isSaturated: state.activeSinceMs !== null,
		reasons,
		utilizationPercent: getUtilizationPercent(stats),
		waitingCount: stats.waitingCount,
		recentAcquireTimeouts,
		activeSince: toIsoOrNull(state.activeSinceMs),
		lastSignalAt: toIsoOrNull(state.lastSignalAtMs),
	}
}

export function getGraphqlPressureSnapshot(stats: PoolStats, nowMs = Date.now()): SaturationSnapshot {
	return getPoolSaturationSnapshot("graphql", stats, nowMs)
}

export function recordGraphqlAcquireTimeout(nowMs = Date.now()): void {
	recordPoolAcquireTimeout("graphql", nowMs)
}

/**
 * Shed traffic only when saturation has been sustained through the activation
 * window. Raw `waiting_clients` / `acquire_timeouts` reasons are transient
 * signals — they advance the saturation FSM so a sustained signal will flip
 * `isSaturated` — but shedding on them directly bypasses the activation delay
 * and drops requests on momentary bursts (e.g. `waitingCount=1` for 30ms under
 * normal bursty traffic). The pool's own `connectionTimeoutMillis` is the
 * short-circuit fuse for truly stuck acquires; this gate is for sustained
 * pressure.
 */
export function shouldShedPoolTraffic(snapshot: SaturationSnapshot): boolean {
	return snapshot.isSaturated
}
