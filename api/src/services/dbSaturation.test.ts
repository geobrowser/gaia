import {describe, expect, it} from "vitest"

import {
	getPoolSaturationSnapshot,
	recordPoolAcquireTimeout,
	recordPoolSlowAcquire,
	shouldShedPoolTraffic,
} from "./dbSaturation"

type PoolStats = {
	totalConnections: number
	idleConnections: number
	waitingCount: number
	maxConnections: number
}

const baseStats: PoolStats = {
	totalConnections: 10,
	idleConnections: 5,
	waitingCount: 0,
	maxConnections: 50,
}

let poolSequence = 0
function uniquePoolName(prefix: string): string {
	poolSequence += 1
	return `${prefix}-${poolSequence}`
}

describe("dbSaturation hysteresis", () => {
	it("activates saturation only after sustained pressure window", () => {
		const poolName = uniquePoolName("activate")
		const pressuredStats: PoolStats = {...baseStats, waitingCount: 5}

		const atStart = getPoolSaturationSnapshot(poolName, pressuredStats, 1_000)
		expect(atStart.isPressured).toBe(true)
		expect(atStart.isSaturated).toBe(false)

		const beforeThreshold = getPoolSaturationSnapshot(poolName, pressuredStats, 15_999)
		expect(beforeThreshold.isSaturated).toBe(false)

		const atThreshold = getPoolSaturationSnapshot(poolName, pressuredStats, 16_000)
		expect(atThreshold.isSaturated).toBe(true)
		expect(atThreshold.activeSince).not.toBeNull()
	})

	it("keeps saturated state until release window elapses", () => {
		const poolName = uniquePoolName("release")
		const pressuredStats: PoolStats = {...baseStats, waitingCount: 5}

		getPoolSaturationSnapshot(poolName, pressuredStats, 10_000)
		const saturated = getPoolSaturationSnapshot(poolName, pressuredStats, 25_000)
		expect(saturated.isSaturated).toBe(true)

		const normalStats: PoolStats = {...baseStats, waitingCount: 0}
		const beforeRelease = getPoolSaturationSnapshot(poolName, normalStats, 54_999)
		expect(beforeRelease.isSaturated).toBe(true)

		const atRelease = getPoolSaturationSnapshot(poolName, normalStats, 55_000)
		expect(atRelease.isSaturated).toBe(false)
		expect(atRelease.activeSince).toBeNull()
	})
})

describe("dbSaturation pressure reasons", () => {
	it("emits acquire_timeouts when timeout count passes threshold in rolling window", () => {
		const poolName = uniquePoolName("timeouts")

		recordPoolAcquireTimeout(poolName, 1_000)
		recordPoolAcquireTimeout(poolName, 2_000)

		const snapshot = getPoolSaturationSnapshot(poolName, baseStats, 2_500)
		expect(snapshot.reasons).toContain("acquire_timeouts")
		expect(snapshot.recentAcquireTimeouts).toBe(2)
	})

	it("prunes old timeout events outside rolling window", () => {
		const poolName = uniquePoolName("prune")

		recordPoolAcquireTimeout(poolName, 1_000)
		recordPoolAcquireTimeout(poolName, 2_000)
		recordPoolAcquireTimeout(poolName, 32_000)

		const snapshot = getPoolSaturationSnapshot(poolName, baseStats, 33_001)
		expect(snapshot.recentAcquireTimeouts).toBe(1)
		expect(snapshot.reasons).not.toContain("acquire_timeouts")
	})

	it("emits high_utilization when pool usage crosses threshold", () => {
		const poolName = uniquePoolName("utilization")
		const highUtilStats: PoolStats = {
			...baseStats,
			totalConnections: 45,
			idleConnections: 0,
			maxConnections: 50,
		}

		const snapshot = getPoolSaturationSnapshot(poolName, highUtilStats, 1_000)
		expect(snapshot.reasons).toContain("high_utilization")
		expect(snapshot.utilizationPercent).toBe(90)
	})
})

describe("dbSaturation load shedding", () => {
	it("does not shed on a momentary waiting client before activation", () => {
		const poolName = uniquePoolName("waiting")
		const snapshot = getPoolSaturationSnapshot(poolName, {...baseStats, waitingCount: 5}, 1_000)

		expect(snapshot.isPressured).toBe(true)
		expect(snapshot.isSaturated).toBe(false)
		expect(shouldShedPoolTraffic(snapshot)).toBe(false)
	})

	it("does not shed on recent acquire timeouts before activation", () => {
		const poolName = uniquePoolName("timeouts-only")
		recordPoolAcquireTimeout(poolName, 1_000)
		recordPoolAcquireTimeout(poolName, 1_500)

		const snapshot = getPoolSaturationSnapshot(poolName, baseStats, 2_000)

		expect(snapshot.reasons).toContain("acquire_timeouts")
		expect(snapshot.isSaturated).toBe(false)
		expect(shouldShedPoolTraffic(snapshot)).toBe(false)
	})

	it("sheds once saturation has activated", () => {
		const poolName = uniquePoolName("saturated")
		const pressuredStats: PoolStats = {...baseStats, waitingCount: 5}

		getPoolSaturationSnapshot(poolName, pressuredStats, 10_000)
		const snapshot = getPoolSaturationSnapshot(poolName, pressuredStats, 25_000)

		expect(snapshot.isSaturated).toBe(true)
		expect(shouldShedPoolTraffic(snapshot)).toBe(true)
	})

	it("continues shedding while saturated even after raw signals drop", () => {
		const poolName = uniquePoolName("sticky-saturated")
		const pressuredStats: PoolStats = {...baseStats, waitingCount: 5}

		getPoolSaturationSnapshot(poolName, pressuredStats, 10_000)
		const atSaturation = getPoolSaturationSnapshot(poolName, pressuredStats, 25_000)
		expect(atSaturation.isSaturated).toBe(true)

		// Signal drops, but release window has not elapsed.
		const normalStats: PoolStats = {...baseStats, waitingCount: 0}
		const stillSaturated = getPoolSaturationSnapshot(poolName, normalStats, 30_000)
		expect(stillSaturated.reasons).toEqual([])
		expect(stillSaturated.isSaturated).toBe(true)
		expect(shouldShedPoolTraffic(stillSaturated)).toBe(true)
	})

	it("does not shed on high utilization alone", () => {
		const poolName = uniquePoolName("utilization-only")
		const snapshot = getPoolSaturationSnapshot(
			poolName,
			{
				...baseStats,
				totalConnections: 45,
				idleConnections: 0,
				maxConnections: 50,
			},
			1_000,
		)

		expect(snapshot.reasons).toContain("high_utilization")
		expect(shouldShedPoolTraffic(snapshot)).toBe(false)
	})

	it("stops shedding after the release window elapses", () => {
		const poolName = uniquePoolName("release-shed")
		const pressuredStats: PoolStats = {...baseStats, waitingCount: 5}

		getPoolSaturationSnapshot(poolName, pressuredStats, 10_000)
		const saturated = getPoolSaturationSnapshot(poolName, pressuredStats, 25_000)
		expect(shouldShedPoolTraffic(saturated)).toBe(true)

		const normalStats: PoolStats = {...baseStats, waitingCount: 0}
		const afterRelease = getPoolSaturationSnapshot(poolName, normalStats, 55_001)
		expect(afterRelease.isSaturated).toBe(false)
		expect(shouldShedPoolTraffic(afterRelease)).toBe(false)
	})

	// Regression for the 2026-08-06 incident: the api stalled for hours while
	// every configured input read normal. These are the numbers observed on a
	// dying pod — a free pool, nothing queued, and acquires taking 500ms+
	// because the event loop could not run the callback.
	it("records slow acquires but does NOT raise a pressure reason", () => {
		const poolName = uniquePoolName("slow-acquire")
		const healthyStats: PoolStats = {
			totalConnections: 33,
			idleConnections: 19,
			waitingCount: 0,
			maxConnections: 50,
		}

		// Sanity: none of the pre-existing signals fire on these stats.
		const before = getPoolSaturationSnapshot(poolName, healthyStats, 1_000)
		expect(before.reasons).toEqual([])

		for (let i = 0; i < 5; i++) {
			recordPoolSlowAcquire(poolName, 1_000 + i)
		}

		const after = getPoolSaturationSnapshot(poolName, healthyStats, 1_100)
		// Counted and surfaced for observability...
		expect(after.recentSlowAcquires).toBe(5)
		// ...but deliberately not a pressure reason. 48% of populated 30s
		// windows in healthy production already contain >= 5 slow acquires, so
		// shedding on this would refuse ordinary traffic — and to a client that
		// retries 503s. See the threshold comment in dbSaturation.ts.
		expect(after.reasons).toEqual([])
		expect(after.isPressured).toBe(false)
		expect(after.utilizationPercent).toBe(66)
		expect(after.waitingCount).toBe(0)
	})

	it("counts slow acquires independently of the reported reasons", () => {
		const poolName = uniquePoolName("slow-acquire-under")
		recordPoolSlowAcquire(poolName, 1_000)
		recordPoolSlowAcquire(poolName, 1_001)

		const snapshot = getPoolSaturationSnapshot(poolName, baseStats, 1_100)
		expect(snapshot.recentSlowAcquires).toBe(2)
		expect(snapshot.reasons).toEqual([])
	})

	it("keeps slow acquires separate from acquire timeouts", () => {
		const poolName = uniquePoolName("slow-vs-timeout")
		for (let i = 0; i < 5; i++) recordPoolSlowAcquire(poolName, 1_000 + i)

		const snapshot = getPoolSaturationSnapshot(poolName, baseStats, 1_100)
		expect(snapshot.recentSlowAcquires).toBe(5)
		expect(snapshot.recentAcquireTimeouts).toBe(0)
	})

	it("never sheds on slow acquires alone, however many accumulate", () => {
		const poolName = uniquePoolName("slow-acquire-no-shed")
		const healthyStats: PoolStats = {
			totalConnections: 33,
			idleConnections: 19,
			waitingCount: 0,
			maxConnections: 50,
		}

		// Far more than the busiest window ever observed in production (21),
		// sustained well past the activation window. Must still not shed.
		for (let i = 0; i < 50; i++) recordPoolSlowAcquire(poolName, 10_000 + i)
		const first = getPoolSaturationSnapshot(poolName, healthyStats, 10_100)
		expect(shouldShedPoolTraffic(first)).toBe(false)

		for (let i = 0; i < 50; i++) recordPoolSlowAcquire(poolName, 25_000 + i)
		const later = getPoolSaturationSnapshot(poolName, healthyStats, 25_100)
		expect(later.recentSlowAcquires).toBeGreaterThan(21)
		expect(later.isPressured).toBe(false)
		expect(later.isSaturated).toBe(false)
		expect(shouldShedPoolTraffic(later)).toBe(false)
	})
})
