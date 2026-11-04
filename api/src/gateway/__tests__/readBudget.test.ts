import {describe, expect, it} from "vitest"
import {createReadBudget} from "../readBudget"

describe("read budget", () => {
	it("takes n reads all or none, and refills with time up to the burst", () => {
		let now = 0
		const budget = createReadBudget({perSecond: 2, burst: 3, now: () => now})
		expect(budget.tryAcquire(2)).toBe(true)
		expect(budget.tryAcquire(2)).toBe(false) // 1 left: none taken
		expect(budget.tryAcquire(1)).toBe(true)
		expect(budget.tryAcquire(1)).toBe(false)
		now += 500
		expect(budget.tryAcquire(1)).toBe(true)
		now += 60_000
		expect(budget.tryAcquire(4)).toBe(false) // never more than the burst
		expect(budget.tryAcquire(3)).toBe(true)
	})
})
