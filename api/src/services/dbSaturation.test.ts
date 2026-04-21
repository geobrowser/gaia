import {describe, expect, it} from "vitest"

import {getPoolSaturationSnapshot, recordPoolAcquireTimeout, shouldShedPoolTraffic} from "./dbSaturation"

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
})
