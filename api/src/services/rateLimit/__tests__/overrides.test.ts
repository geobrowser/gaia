import {beforeEach, describe, expect, it, vi} from "vitest"
import {createOverrideLookup} from "../overrides"

/**
 * The override lookup wraps a Postgres query in a per-pod LRU cache.
 * These tests exercise the cache layer with a stubbed db.execute,
 * verifying that:
 *   - a hit is served from cache (no DB call)
 *   - a miss is also cached (so we don't re-query for default-limit IPs)
 *   - expired entries trigger a re-query
 *   - the LRU evicts oldest entries when capacity is reached
 *   - DB errors are not cached (transient retry behavior)
 */
function fakeDb(rowsByIp: Record<string, number | undefined>, throwOn?: Set<string>) {
	const calls: string[] = []
	const db = {
		async execute<_T>(query: {queryChunks: unknown[]}): Promise<{rows: Array<{requests_per_min: number}>}> {
			// drizzle's sql template wraps params; for the test we just inspect the
			// stringified query to extract the IP that was bound.
			const serialized = JSON.stringify(query)
			const ipMatch = serialized.match(/\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}/)
			const ip = ipMatch?.[0] ?? ""
			calls.push(ip)
			if (throwOn?.has(ip)) throw new Error("db error")
			const limit = rowsByIp[ip]
			return {rows: limit !== undefined ? [{requests_per_min: limit}] : []}
		},
	}
	return {db: db as never, calls}
}

describe("createOverrideLookup", () => {
	beforeEach(() => {
		vi.useRealTimers()
	})

	it("returns the limit for a matching IP", async () => {
		const {db} = fakeDb({"203.0.113.42": 5000})
		const lookup = createOverrideLookup(db, 60)
		expect(await lookup.lookup("203.0.113.42")).toBe(5000)
	})

	it("returns null when no override matches", async () => {
		const {db} = fakeDb({})
		const lookup = createOverrideLookup(db, 60)
		expect(await lookup.lookup("203.0.113.42")).toBeNull()
	})

	it("serves cache hit without re-querying DB", async () => {
		const {db, calls} = fakeDb({"203.0.113.42": 5000})
		const lookup = createOverrideLookup(db, 60)
		await lookup.lookup("203.0.113.42")
		await lookup.lookup("203.0.113.42")
		await lookup.lookup("203.0.113.42")
		expect(calls).toHaveLength(1)
	})

	it("caches misses so default-limit IPs do not re-query", async () => {
		const {db, calls} = fakeDb({})
		const lookup = createOverrideLookup(db, 60)
		await lookup.lookup("203.0.113.42")
		await lookup.lookup("203.0.113.42")
		expect(calls).toHaveLength(1)
	})

	it("re-queries after the TTL expires", async () => {
		vi.useFakeTimers()
		const {db, calls} = fakeDb({"203.0.113.42": 5000})
		const lookup = createOverrideLookup(db, 60)
		await lookup.lookup("203.0.113.42")
		vi.advanceTimersByTime(61_000)
		await lookup.lookup("203.0.113.42")
		expect(calls).toHaveLength(2)
	})

	it("does not cache DB errors so transient failures are retried", async () => {
		const {db, calls} = fakeDb({}, new Set(["203.0.113.42"]))
		const lookup = createOverrideLookup(db, 60)
		expect(await lookup.lookup("203.0.113.42")).toBeNull()
		expect(await lookup.lookup("203.0.113.42")).toBeNull()
		expect(calls).toHaveLength(2)
	})

	it("evicts the oldest entry when LRU capacity is reached", async () => {
		const {db, calls} = fakeDb({})
		const lookup = createOverrideLookup(db, 60, 3) // tiny capacity for the test

		await lookup.lookup("1.1.1.1")
		await lookup.lookup("2.2.2.2")
		await lookup.lookup("3.3.3.3")
		expect(lookup.size()).toBe(3)

		// Adding a 4th should evict 1.1.1.1 (oldest)
		await lookup.lookup("4.4.4.4")
		expect(lookup.size()).toBe(3)

		// 1.1.1.1 was evicted → re-querying it should hit DB again
		const callsBefore = calls.length
		await lookup.lookup("1.1.1.1")
		expect(calls.length).toBe(callsBefore + 1)

		// 4.4.4.4 was the most recent insert and survives all evictions → cache hit
		const callsBefore2 = calls.length
		await lookup.lookup("4.4.4.4")
		expect(calls.length).toBe(callsBefore2)
	})

	it("touches LRU recency on read", async () => {
		const {db, calls} = fakeDb({})
		const lookup = createOverrideLookup(db, 60, 3)

		await lookup.lookup("1.1.1.1")
		await lookup.lookup("2.2.2.2")
		await lookup.lookup("3.3.3.3")
		// Touch 1.1.1.1 → moves it to most-recent position
		await lookup.lookup("1.1.1.1")
		// Insert 4.4.4.4 → evicts 2.2.2.2 (now oldest) instead of 1.1.1.1
		await lookup.lookup("4.4.4.4")

		const callsBefore = calls.length
		await lookup.lookup("1.1.1.1") // still cached
		expect(calls.length).toBe(callsBefore)
		await lookup.lookup("2.2.2.2") // evicted, re-queries
		expect(calls.length).toBe(callsBefore + 1)
	})

	it("evicts stale entries proactively on read", async () => {
		vi.useFakeTimers()
		const {db, calls} = fakeDb({"203.0.113.42": 1000})
		const lookup = createOverrideLookup(db, 60)
		await lookup.lookup("203.0.113.42")
		expect(lookup.size()).toBe(1)

		vi.advanceTimersByTime(61_000)
		await lookup.lookup("203.0.113.42")
		// Should have been deleted then re-inserted (still size 1, and DB called twice)
		expect(lookup.size()).toBe(1)
		expect(calls).toHaveLength(2)
	})
})
