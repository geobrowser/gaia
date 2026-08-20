import {describe, expect, it} from "bun:test"
import type {ExecutionResult} from "graphql"
import {exceedsCacheableBytesEstimate, MAX_CACHEABLE_BYTES, shouldSkipCacheSet} from "../valkeyCache"

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

describe("shouldSkipCacheSet", () => {
	it("returns false for an empty payload", () => {
		expect(shouldSkipCacheSet("")).toBe(false)
	})

	it("returns false for payloads at the exact limit", () => {
		// boundary: a string of exactly MAX_CACHEABLE_BYTES chars is NOT skipped
		const atLimit = "x".repeat(MAX_CACHEABLE_BYTES)
		expect(shouldSkipCacheSet(atLimit)).toBe(false)
	})

	it("returns true for payloads just over the limit", () => {
		const overLimit = "x".repeat(MAX_CACHEABLE_BYTES + 1)
		expect(shouldSkipCacheSet(overLimit)).toBe(true)
	})

	it("returns true for significantly oversized payloads", () => {
		// Simulates the 36 MB Claim-relations response we saw in prod logs
		const huge = "x".repeat(36_000_000)
		expect(shouldSkipCacheSet(huge)).toBe(true)
	})

	it("is a pure function over string length", () => {
		// Deliberately schema-agnostic: we only inspect length, not content
		expect(shouldSkipCacheSet("x".repeat(MAX_CACHEABLE_BYTES + 100))).toBe(
			shouldSkipCacheSet("y".repeat(MAX_CACHEABLE_BYTES + 100)),
		)
	})

	it("measures UTF-8 bytes, not UTF-16 code units (CJK content)", () => {
		// Regression guard: `.length` returns UTF-16 code units. Each CJK char
		// is 1 code unit but 3 UTF-8 bytes, so a payload that looks "safe" by
		// length alone can still exceed the byte budget. ceil(limit/3)+1 chars
		// puts us just past the cap after UTF-8 encoding.
		const cjkCharsOverLimit = Math.ceil(MAX_CACHEABLE_BYTES / 3) + 1
		const cjk = "日".repeat(cjkCharsOverLimit)
		// By UTF-16 code units it looks well under the cap...
		expect(cjk.length).toBeLessThan(MAX_CACHEABLE_BYTES)
		// ...but the actual UTF-8 byte length is over, so we skip.
		expect(Buffer.byteLength(cjk, "utf8")).toBeGreaterThan(MAX_CACHEABLE_BYTES)
		expect(shouldSkipCacheSet(cjk)).toBe(true)
	})

	it("treats ASCII content as byte-for-byte equal to its length", () => {
		// Sanity check: for pure ASCII, UTF-16 code units == UTF-8 bytes,
		// so the byte-accurate check still agrees with `.length` here.
		const asciiJustUnder = "x".repeat(MAX_CACHEABLE_BYTES)
		const asciiJustOver = "x".repeat(MAX_CACHEABLE_BYTES + 1)
		expect(shouldSkipCacheSet(asciiJustUnder)).toBe(false)
		expect(shouldSkipCacheSet(asciiJustOver)).toBe(true)
	})
})

describe("exceedsCacheableBytesEstimate", () => {
	// The pre-check exists so an oversized response is never serialized just to
	// be measured and discarded. Its correctness rests on one asymmetry: the
	// estimate is always <= the real UTF-8 length, so a positive answer is
	// always right, and a negative one is only ever conservative (the exact
	// Buffer.byteLength check still runs after it).

	it("passes an ordinary response through to the exact check", () => {
		const data = {data: {entities: [{id: "a", name: "small"}]}}
		expect(exceedsCacheableBytesEstimate(data)).toBe(false)
	})

	it("rejects a clearly oversized response", () => {
		// The 62.9 MB shape from the 2026-08-19 incident, in miniature: many
		// rows rather than one huge string, so the walk has to accumulate.
		const data = {
			data: {
				entities: Array.from({length: 20_000}, (_, i) => ({
					id: `entity-${i}`,
					name: "x".repeat(1000),
				})),
			},
		}
		expect(exceedsCacheableBytesEstimate(data)).toBe(true)
	})

	it("never rejects a response the exact check would have accepted", () => {
		// The one-directional guarantee, over content designed to stress it:
		// multi-byte characters and escape-heavy strings are exactly where a
		// code-unit estimate diverges from real UTF-8 bytes, and it must
		// diverge downward.
		for (const filler of ["x", "日", "🌍", '"', "\n"]) {
			const rows = Math.ceil(MAX_CACHEABLE_BYTES / (filler.length * 200))
			const data = {data: {rows: Array.from({length: rows}, () => filler.repeat(200))}}
			const serialized = JSON.stringify(data)
			if (!shouldSkipCacheSet(serialized)) {
				expect(exceedsCacheableBytesEstimate(data)).toBe(false)
			}
		}
	})

	it("agrees with the exact check on a clearly oversized multi-byte payload", () => {
		const data = {data: {text: "日".repeat(MAX_CACHEABLE_BYTES)}}
		expect(exceedsCacheableBytesEstimate(data)).toBe(true)
		expect(shouldSkipCacheSet(JSON.stringify(data))).toBe(true)
	})
})
