import type {PoolClient} from "pg"
import {describe, expect, it, vi} from "vitest"

import {connectPgClientWithRetry} from "./pgClient"

describe("connectPgClientWithRetry", () => {
	it("retries transient connect failures and returns client", async () => {
		const client = {release: vi.fn()} as unknown as PoolClient
		let attempts = 0

		const pool = {
			connect: vi.fn(async () => {
				attempts += 1
				if (attempts < 3) {
					throw new Error("timeout exceeded when trying to connect")
				}
				return client
			}),
		}

		const logger = {warn: vi.fn(), error: vi.fn()}
		const result = await connectPgClientWithRetry({
			pool,
			operationName: "postgraphile.pool.connect",
			getPoolStats: () => ({totalConnections: 10, idleConnections: 2, waitingCount: 1, maxConnections: 50}),
			logger,
		})

		expect(result).toBe(client)
		expect(pool.connect).toHaveBeenCalledTimes(3)
		expect(logger.warn).toHaveBeenCalledTimes(2)
		expect(logger.error).not.toHaveBeenCalled()
	})

	it("does not retry non-transient failures", async () => {
		const pool = {
			connect: vi.fn(async () => {
				throw new Error("relation does not exist")
			}),
		}
		const logger = {warn: vi.fn(), error: vi.fn()}

		await expect(
			connectPgClientWithRetry({
				pool,
				operationName: "postgraphile.pool.connect",
				getPoolStats: () => ({totalConnections: 0, idleConnections: 0, waitingCount: 0, maxConnections: 50}),
				logger,
			}),
		).rejects.toThrow("relation does not exist")

		expect(pool.connect).toHaveBeenCalledTimes(1)
		expect(logger.warn).not.toHaveBeenCalled()
		expect(logger.error).toHaveBeenCalledTimes(1)
	})
})
