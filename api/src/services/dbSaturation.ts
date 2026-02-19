type PoolStats = {
	totalConnections: number
	idleConnections: number
	waitingCount: number
	maxConnections: number
}

type SaturationReason = "waiting_clients" | "high_utilization" | "acquire_timeouts"

type SaturationSnapshot = {
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

const PRESSURE_WAITING_THRESHOLD = parseInt(process.env.PG_POOL_PRESSURE_WAITING_THRESHOLD || "1", 10)
const PRESSURE_UTILIZATION_THRESHOLD = parseInt(process.env.PG_POOL_PRESSURE_UTILIZATION_THRESHOLD || "90", 10)
const PRESSURE_TIMEOUT_THRESHOLD = parseInt(process.env.PG_POOL_PRESSURE_TIMEOUT_THRESHOLD || "2", 10)
const SATURATION_ACTIVATION_MS = parseInt(process.env.PG_POOL_SATURATION_ACTIVATION_MS || "15000", 10)
const SATURATION_RELEASE_MS = parseInt(process.env.PG_POOL_SATURATION_RELEASE_MS || "30000", 10)
const ACQUIRE_TIMEOUT_WINDOW_MS = parseInt(process.env.PG_POOL_ACQUIRE_TIMEOUT_WINDOW_MS || "30000", 10)

const perPoolState = new Map<string, SaturationState>()
const acquireTimeoutTimestamps = new Map<string, number[]>()

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

function pruneOldTimeouts(poolName: string, nowMs: number): number[] {
	const list = acquireTimeoutTimestamps.get(poolName) || []
	const cutoff = nowMs - ACQUIRE_TIMEOUT_WINDOW_MS
	const pruned = list.filter((timestamp) => timestamp >= cutoff)
	acquireTimeoutTimestamps.set(poolName, pruned)
	return pruned
}

function getPressureReasons(stats: PoolStats, nowMs: number, poolName: string): SaturationReason[] {
	const reasons: SaturationReason[] = []
	const utilizationPercent = getUtilizationPercent(stats)
	const recentTimeouts = pruneOldTimeouts(poolName, nowMs).length

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
	const list = acquireTimeoutTimestamps.get(poolName) || []
	list.push(nowMs)
	acquireTimeoutTimestamps.set(poolName, list)
	pruneOldTimeouts(poolName, nowMs)
}

export function getRecentPoolAcquireTimeoutCount(poolName: string, nowMs = Date.now()): number {
	return pruneOldTimeouts(poolName, nowMs).length
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
