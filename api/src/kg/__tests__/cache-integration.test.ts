/**
 * Integration test for the GraphQL response cache.
 *
 * Spins up a real GraphQL Yoga server with the response cache plugin
 * backed by an in-memory mock cache, and verifies the full pipeline:
 * query → execute → onExecuteDone → cache.set → cache.get → serve from cache.
 *
 * This test catches version mismatches between graphql-yoga and the
 * response cache plugin (e.g. SET not firing due to peer dep mismatch).
 *
 * Uses bun:test (not vitest) to avoid duplicate graphql module conflicts
 * between @envelop/response-cache and the main graphql package.
 */
import {describe, expect, it} from "bun:test"
import {useResponseCache} from "@graphql-yoga/plugin-response-cache"
import type {ExecutionResult} from "graphql"
import {GraphQLInt, GraphQLObjectType, GraphQLSchema, GraphQLString} from "graphql"
import {createYoga} from "graphql-yoga"

type CacheEntry = {data: ExecutionResult; ttl: number; storedAt: number}

function createTestCache() {
	const store = new Map<string, CacheEntry>()
	let setCalls = 0
	let getCalls = 0

	const cache = {
		async set(
			id: string,
			data: ExecutionResult,
			_entities: Iterable<{typename: string; id?: string | number}>,
			ttl: number,
		) {
			setCalls++
			store.set(id, {data, ttl, storedAt: Date.now()})
		},
		async get(id: string): Promise<ExecutionResult | undefined> {
			getCalls++
			const entry = store.get(id)
			if (!entry) return undefined
			return entry.data
		},
		async invalidate() {},
	}

	return {
		cache,
		get setCalls() {
			return setCalls
		},
		get getCalls() {
			return getCalls
		},
		get storeSize() {
			return store.size
		},
		getEntries() {
			return store.entries()
		},
	}
}

let executionCount = 0

const schema = new GraphQLSchema({
	query: new GraphQLObjectType({
		name: "Query",
		fields: {
			hello: {
				type: GraphQLString,
				resolve: () => {
					executionCount++
					return "world"
				},
			},
			counter: {
				type: GraphQLInt,
				resolve: () => {
					executionCount++
					return executionCount
				},
			},
		},
	}),
})

async function fetchYoga(yoga: ReturnType<typeof createYoga>, query: string, headers?: Record<string, string>) {
	return yoga.fetch("http://localhost/graphql", {
		method: "POST",
		headers: {"content-type": "application/json", ...headers},
		body: JSON.stringify({query}),
	})
}

describe("Response cache integration", () => {
	it("SET: caches response after first query execution", async () => {
		const testCache = createTestCache()
		executionCount = 0

		const yoga = createYoga({
			schema,
			plugins: [useResponseCache({session: () => null, ttl: 10_000, cache: testCache.cache})],
		})

		const res1 = await fetchYoga(yoga, "{ hello }")
		expect(res1.status).toBe(200)
		const body1 = (await res1.json()) as {data: {hello: string}}
		expect(body1.data.hello).toBe("world")
		expect(executionCount).toBe(1)
		expect(testCache.getCalls).toBeGreaterThanOrEqual(1)
		expect(testCache.setCalls).toBe(1)
		expect(testCache.storeSize).toBe(1)
	})

	it("GET: serves from cache on second identical query (no re-execution)", async () => {
		const testCache = createTestCache()
		executionCount = 0

		const yoga = createYoga({
			schema,
			plugins: [useResponseCache({session: () => null, ttl: 10_000, cache: testCache.cache})],
		})

		await fetchYoga(yoga, "{ counter }")
		expect(executionCount).toBe(1)
		expect(testCache.setCalls).toBe(1)

		const res2 = await fetchYoga(yoga, "{ counter }")
		const body2 = (await res2.json()) as {data: {counter: number}}
		expect(body2.data.counter).toBe(1)
		expect(executionCount).toBe(1)
		expect(testCache.setCalls).toBe(1)
		expect(testCache.getCalls).toBeGreaterThanOrEqual(2)
	})

	it("different queries get different cache entries", async () => {
		const testCache = createTestCache()
		executionCount = 0

		const yoga = createYoga({
			schema,
			plugins: [useResponseCache({session: () => null, ttl: 10_000, cache: testCache.cache})],
		})

		await fetchYoga(yoga, "{ hello }")
		await fetchYoga(yoga, "{ counter }")

		expect(executionCount).toBe(2)
		expect(testCache.setCalls).toBe(2)
		expect(testCache.storeSize).toBe(2)
	})

	it("cache miss falls through to execution", async () => {
		const testCache = createTestCache()
		executionCount = 0

		const yoga = createYoga({
			schema,
			plugins: [useResponseCache({session: () => null, ttl: 10_000, cache: testCache.cache})],
		})

		const res = await fetchYoga(yoga, "{ hello }")
		const body = (await res.json()) as {data: {hello: string}}
		expect(body.data.hello).toBe("world")
		expect(executionCount).toBe(1)
	})

	it("session: () => null means shared cache across requests", async () => {
		const testCache = createTestCache()
		executionCount = 0

		const yoga = createYoga({
			schema,
			plugins: [useResponseCache({session: () => null, ttl: 10_000, cache: testCache.cache})],
		})

		await fetchYoga(yoga, "{ hello }", {"x-user": "alice"})
		await fetchYoga(yoga, "{ hello }", {"x-user": "bob"})

		expect(executionCount).toBe(1)
		expect(testCache.setCalls).toBe(1)
	})

	it("default TTL is passed to cache.set", async () => {
		const testCache = createTestCache()
		executionCount = 0

		const yoga = createYoga({
			schema,
			plugins: [useResponseCache({session: () => null, ttl: 10_000, cache: testCache.cache})],
		})

		await fetchYoga(yoga, "{ hello }")

		expect(testCache.setCalls).toBe(1)
		const entries = [...testCache.getEntries()]
		expect(entries.length).toBe(1)
		expect(entries[0]?.[1].ttl).toBe(10_000)
	})

	it("ttlPerSchemaCoordinate overrides default TTL for specific queries", async () => {
		const testCache = createTestCache()
		executionCount = 0

		const yoga = createYoga({
			schema,
			plugins: [
				useResponseCache({
					session: () => null,
					ttl: 10_000,
					ttlPerSchemaCoordinate: {"Query.hello": 60_000},
					cache: testCache.cache,
				}),
			],
		})

		await fetchYoga(yoga, "{ hello }")
		await fetchYoga(yoga, "{ counter }")

		expect(testCache.setCalls).toBe(2)
		const entries = [...testCache.getEntries()]
		const ttls = entries.map(([_, v]) => v.ttl).sort((a, b) => a - b)
		expect(ttls).toContain(10_000)
		expect(ttls).toContain(60_000)
	})
})
