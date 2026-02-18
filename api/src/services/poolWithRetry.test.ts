import type {Pool, PoolClient} from "pg"
import {describe, expect, it, vi} from "vitest"

import {createPoolWithRetryConnect} from "./poolWithRetry"

describe("createPoolWithRetryConnect", () => {
	it("retries callback-style connect path", async () => {
		let attempts = 0
		const client = {release: vi.fn()} as unknown as PoolClient
		const pool = {
			connect: vi.fn(async () => {
				attempts += 1
				if (attempts < 2) {
					throw new Error("timeout exceeded when trying to connect")
				}
				return client
			}),
		} as unknown as Pool

		const onRetry = vi.fn()
		const onGiveUp = vi.fn()
		const wrapped = createPoolWithRetryConnect({
			pool,
			operationName: "drizzle.pool.connect",
			onRetry,
			onGiveUp,
		})

		await new Promise<void>((resolve, reject) => {
			wrapped.connect((error, connectedClient, done) => {
				if (error) {
					reject(error)
					return
				}

				expect(connectedClient).toBe(client)
				done()
				resolve()
			})
		})

		expect(pool.connect).toHaveBeenCalledTimes(2)
		expect(onRetry).toHaveBeenCalledTimes(1)
		expect(onGiveUp).not.toHaveBeenCalled()
		expect(client.release).toHaveBeenCalledTimes(1)
	})

	it("returns promise-style connect path with retries", async () => {
		let attempts = 0
		const client = {release: vi.fn()} as unknown as PoolClient
		const pool = {
			connect: vi.fn(async () => {
				attempts += 1
				if (attempts < 2) {
					throw new Error("timeout exceeded when trying to connect")
				}
				return client
			}),
		} as unknown as Pool

		const wrapped = createPoolWithRetryConnect({
			pool,
			operationName: "drizzle.pool.connect",
			onRetry: vi.fn(),
			onGiveUp: vi.fn(),
		})

		const connectedClient = await wrapped.connect()
		expect(connectedClient).toBe(client)
		expect(pool.connect).toHaveBeenCalledTimes(2)
	})
})
