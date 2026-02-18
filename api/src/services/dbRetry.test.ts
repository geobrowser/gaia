import {describe, expect, it, vi} from "vitest"

import {calculateRetryDelayMs, withDbRetry} from "./dbRetry"

describe("dbRetry", () => {
	it("retries transient failures and succeeds", async () => {
		let attempts = 0
		const retryEvents: Array<{attempt: number; delayMs: number}> = []

		const result = await withDbRetry(
			async () => {
				attempts += 1
				if (attempts < 3) {
					throw new Error("timeout exceeded when trying to connect")
				}
				return "ok"
			},
			{
				maxElapsedMs: 3000,
				baseDelayMs: 100,
				maxDelayMs: 800,
				maxAttempts: 3,
				random: () => 0,
				onRetry: ({attempt, delayMs}) => {
					retryEvents.push({attempt, delayMs})
				},
			},
		)

		expect(result).toBe("ok")
		expect(attempts).toBe(3)
		expect(retryEvents.length).toBe(2)
		expect(retryEvents[0]?.attempt).toBe(1)
		expect(retryEvents[1]?.attempt).toBe(2)
		expect(retryEvents[0]?.delayMs).toBeGreaterThan(0)
		expect(retryEvents[1]?.delayMs).toBeGreaterThan(0)
	})

	it("does not retry non-transient failures", async () => {
		let attempts = 0

		await expect(
			withDbRetry(async () => {
				attempts += 1
				throw new Error("relation does not exist")
			}),
		).rejects.toThrow("relation does not exist")

		expect(attempts).toBe(1)
	})

	it("stops retrying when elapsed budget would be exceeded", async () => {
		let attempts = 0
		const onGiveUp = vi.fn()

		await expect(
			withDbRetry(
				async () => {
					attempts += 1
					throw new Error("timeout exceeded when trying to connect")
				},
				{
					maxElapsedMs: 1,
					baseDelayMs: 100,
					maxDelayMs: 800,
					maxAttempts: 3,
					random: () => 0,
					onGiveUp,
				},
			),
		).rejects.toThrow("timeout exceeded when trying to connect")

		expect(attempts).toBeLessThanOrEqual(2)
		expect(onGiveUp).toHaveBeenCalledWith(
			expect.objectContaining({
				reason: "budget_exceeded",
			}),
		)
	})

	it("stops retrying when max attempts is reached", async () => {
		let attempts = 0
		const onGiveUp = vi.fn()

		await expect(
			withDbRetry(
				async () => {
					attempts += 1
					throw new Error("timeout exceeded when trying to connect")
				},
				{
					maxElapsedMs: 3000,
					baseDelayMs: 100,
					maxDelayMs: 800,
					maxAttempts: 2,
					onGiveUp,
				},
			),
		).rejects.toThrow("timeout exceeded when trying to connect")

		expect(attempts).toBe(2)
		expect(onGiveUp).toHaveBeenCalledWith(
			expect.objectContaining({
				reason: "max_attempts",
				attempts: 2,
			}),
		)
	})

	it("calculates bounded jittered backoff", () => {
		const delay = calculateRetryDelayMs(
			4,
			{maxElapsedMs: 3000, baseDelayMs: 100, maxDelayMs: 300, maxAttempts: 3},
			() => 1,
		)

		expect(delay).toBe(300)
	})
})
