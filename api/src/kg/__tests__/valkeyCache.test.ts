import {describe, expect, it, mock, beforeEach} from "bun:test"

// Mock ioredis before importing valkeyCache
const mockSet = mock(() => Promise.resolve("OK"))
const mockGet = mock(() => Promise.resolve(null))
const mockOn = mock(() => {})
const mockConnect = mock(() => Promise.resolve())

mock.module("ioredis", () => ({
	default: class MockRedis {
		set = mockSet
		get = mockGet
		on = mockOn
		connect = mockConnect
		constructor() {}
	},
}))

// Import after mocking
const {createValkeyCache} = await import("../valkeyCache")

describe("createValkeyCache", () => {
	beforeEach(() => {
		mockSet.mockClear()
		mockGet.mockClear()
	})

	it("returns undefined on cache miss", async () => {
		mockGet.mockResolvedValueOnce(null)
		const cache = createValkeyCache("redis://localhost:6379")
		const result = await cache.get("nonexistent-key")
		expect(result).toBeUndefined()
	})

	it("returns parsed JSON on cache hit", async () => {
		const cached = {data: {spaces: [{id: "abc"}]}}
		mockGet.mockResolvedValueOnce(JSON.stringify(cached))
		const cache = createValkeyCache("redis://localhost:6379")
		const result = await cache.get("some-key")
		expect(result).toEqual(cached)
	})

	it("calls set with serialized data and TTL in seconds", async () => {
		const cache = createValkeyCache("redis://localhost:6379")
		const data = {data: {entity: {id: "123", name: "Test"}}}
		await cache.set("key-1", data, [], 10000) // 10s in ms

		expect(mockSet).toHaveBeenCalledWith(
			"key-1",
			JSON.stringify(data),
			"EX",
			10,
		)
	})

	it("converts TTL from milliseconds to seconds (rounds up)", async () => {
		const cache = createValkeyCache("redis://localhost:6379")
		await cache.set("key-2", {data: null}, [], 1500) // 1.5s

		expect(mockSet).toHaveBeenCalledWith(
			"key-2",
			expect.any(String),
			"EX",
			2, // Math.ceil(1.5)
		)
	})

	it("returns undefined on get error (graceful degradation)", async () => {
		mockGet.mockRejectedValueOnce(new Error("connection refused"))
		const cache = createValkeyCache("redis://localhost:6379")
		const result = await cache.get("some-key")
		expect(result).toBeUndefined()
	})

	it("does not throw on set error (graceful degradation)", async () => {
		mockSet.mockRejectedValueOnce(new Error("connection refused"))
		const cache = createValkeyCache("redis://localhost:6379")
		// Should not throw
		await cache.set("key-3", {data: null}, [], 10000)
	})

	it("invalidate is a no-op", async () => {
		const cache = createValkeyCache("redis://localhost:6379")
		// Should not throw
		await cache.invalidate([{typename: "Entity", id: "123"}])
	})
})
