import {describe, expect, it, vi} from "vitest"

import {calculateRetryDelayMs, withDbRetry} from "./dbRetry"

describe("dbRetry", () => {
	it("retries transient failures and succeeds", async () => {
		let attempts = 0
		let nowMs = 0
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
				random: () => 0,
				now: () => nowMs,
				sleep: async (delayMs) => {
					nowMs += delayMs
				},
				onRetry: ({attempt, delayMs}) => {
					retryEvents.push({attempt, delayMs})
				},
			},
		)

		expect(result).toBe("ok")
		expect(attempts).toBe(3)
		expect(retryEvents).toEqual([
			{attempt: 1, delayMs: 50},
			{attempt: 2, delayMs: 100},
		])
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
		let nowMs = 0
		const sleep = vi.fn(async (delayMs: number) => {
			nowMs += delayMs
		})

		await expect(
			withDbRetry(
				async () => {
					attempts += 1
					throw new Error("timeout exceeded when trying to connect")
				},
				{
					maxElapsedMs: 120,
					baseDelayMs: 100,
					maxDelayMs: 800,
					random: () => 1,
					now: () => nowMs,
					sleep,
				},
			),
		).rejects.toThrow("timeout exceeded when trying to connect")

		expect(attempts).toBe(1)
		expect(sleep).not.toHaveBeenCalled()
	})

	it("calculates bounded jittered backoff", () => {
		const delay = calculateRetryDelayMs(4, {maxElapsedMs: 3000, baseDelayMs: 100, maxDelayMs: 300}, () => 1)

		expect(delay).toBe(300)
	})
})
