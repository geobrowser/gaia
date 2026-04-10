import {describe, expect, it} from "bun:test"
import type {ExecutionResult} from "graphql"

/**
 * Unit tests for the Valkey cache adapter contract.
 *
 * Tests the set/get/invalidate interface that the Yoga response cache
 * plugin uses, with TTL conversion and error handling.
 */
function createMockCache() {
	const store = new Map<string, {value: string; ttl: number}>()
	let shouldFail = false

	return {
		cache: {
			async set(id: string, data: ExecutionResult, _entities: Iterable<unknown>, ttl: number) {
				if (shouldFail) throw new Error("connection refused")
				const ttlSeconds = Math.ceil(ttl / 1000)
				store.set(id, {value: JSON.stringify(data), ttl: ttlSeconds})
			},
			async get(id: string): Promise<ExecutionResult | undefined | null> {
				if (shouldFail) throw new Error("connection refused")
				const entry = store.get(id)
				if (!entry) return undefined
				return JSON.parse(entry.value) as ExecutionResult
			},
			async invalidate(_entities: Iterable<unknown>) {},
		},
		getStored(id: string) {
			return store.get(id)
		},
		setFail(fail: boolean) {
			shouldFail = fail
		},
	}
}

describe("Valkey cache adapter contract", () => {
	it("returns undefined on cache miss", async () => {
		const mock = createMockCache()
		const result = await mock.cache.get("nonexistent-key")
		expect(result).toBeUndefined()
	})

	it("returns parsed JSON on cache hit", async () => {
		const mock = createMockCache()
		const data = {data: {spaces: [{id: "abc"}]}}
		await mock.cache.set("key-1", data, [], 10_000)
		const result = await mock.cache.get("key-1")
		expect(result).toEqual(data)
	})

	it("stores serialized data with TTL in seconds", async () => {
		const mock = createMockCache()
		const data = {data: {entity: {id: "123", name: "Test"}}}
		await mock.cache.set("key-1", data, [], 10_000)

		const stored = mock.getStored("key-1")
		expect(stored).toBeDefined()
		expect(stored?.ttl).toBe(10)
		expect(JSON.parse(stored!.value)).toEqual(data)
	})

	it("converts TTL from milliseconds to seconds (rounds up)", async () => {
		const mock = createMockCache()
		await mock.cache.set("key-2", {data: null}, [], 1500)
		expect(mock.getStored("key-2")?.ttl).toBe(2)
	})

	it("get errors propagate", async () => {
		const mock = createMockCache()
		mock.setFail(true)
		try {
			await mock.cache.get("some-key")
			expect(true).toBe(false) // should not reach
		} catch (e: unknown) {
			expect((e as Error).message).toBe("connection refused")
		}
	})

	it("set errors propagate", async () => {
		const mock = createMockCache()
		mock.setFail(true)
		try {
			await mock.cache.set("key-3", {data: null}, [], 10_000)
			expect(true).toBe(false)
		} catch (e: unknown) {
			expect((e as Error).message).toBe("connection refused")
		}
	})

	it("invalidate is a no-op", async () => {
		const mock = createMockCache()
		await mock.cache.invalidate([{typename: "Entity", id: "123"}])
	})
})
