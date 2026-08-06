import type {ExecutionResult} from "graphql"
import Redis from "ioredis"
import {log} from "../services/telemetry"

/**
 * Cache interface expected by @graphql-yoga/plugin-response-cache.
 * We only implement set/get — invalidate is a no-op since we rely on TTL
 * expiry rather than entity-based invalidation.
 */
type CacheEntityRecord = {typename: string; id?: number | string}

export type Cache = {
	set(id: string, data: ExecutionResult, entities: Iterable<CacheEntityRecord>, ttl: number): Promise<void> | void
	get(id: string): Promise<ExecutionResult | undefined | null> | ExecutionResult | undefined | null
	invalidate(entities: Iterable<CacheEntityRecord>): Promise<void> | void
}

/**
 * Skip caching responses larger than this threshold.
 *
 * The previous value was 15 MB, sized so that the biggest responses — the
 * ~12 MB `Query.spaces` list and the 2–15 MB `Query.entities*` lists — stayed
 * cacheable, on the reasoning that they are expensive and near-static so
 * caching them is where the win is. Two things were wrong with that.
 *
 * First, the arithmetic assumed a 4 Gi pod limit ("30 MB per concurrent
 * write — 0.7%"). `api/k8s/v2/api.yaml` and `api/k8s/production/api.yaml`
 * both set `limits.memory: 2Gi`. The headroom was half what the sizing
 * claimed.
 *
 * Second, and the reason this is being lowered: the write path is the cheap
 * half. On a hit the GET path parses the full JSON back into a JS object
 * tree, which for the biggest entries costs ~100–250 ms CPU and up to ~90 MB
 * transient per pod. Valkey is shared across all API pods, so one writer
 * produces N readers, and `ttlPerSchemaCoordinate` in postgraphile.ts gives
 * these exact queries the *longest* TTL (60 s) — the combination maximises
 * hits per large entry, which is precisely the expensive direction.
 *
 * Measured on the v2 cluster 2026-08-06, with the API OOMKilling on a ~2 h
 * cycle (9 pods, 10–21 restarts each) against a production cluster on the
 * same 2Gi limit that had run 9 days clean with the cache disabled:
 *
 *   DBSIZE 91, used_memory 15.08 MB, distributed as
 *     8.39 MB   1 key
 *     5.24 MB   1 key
 *     1.05 MB   3 keys
 *     ≤918 KB   86 keys (most ~17 KB)
 *
 * Two entries held 13.6 of 15.08 MB and both sailed under the 15 MB cap. A
 * 1 MB cap keeps 89 of the 91 keys — ~2% of entries, ~90% of bytes — so the
 * hit rate, which follows repeated small queries rather than one-off large
 * ones, is largely preserved while the two amplifying entries stop being
 * written and, more importantly, stop being re-parsed on every hit.
 *
 * This bounds the damage; it does not fix the cause. The responses are large
 * because paginationCapPlugin.ts caps `first` *per argument* (max 1000,
 * default 100) while nested lists multiply — `entities { valuesList(first:
 * 1000) relationsList(first: 1000) }` is legal at every argument and still
 * hundreds of thousands of rows. A per-response node budget is the real fix.
 *
 * Note the check below still runs *after* JSON.stringify, so lowering this
 * avoids the ioredis send buffer, the network send, and every subsequent GET
 * parse — but not the serialization itself. Skipping that too means bounding
 * on row count in `shouldCacheResult` (postgraphile.ts), before the response
 * is ever serialized.
 */
export const MAX_CACHEABLE_BYTES = 1_000_000

/**
 * Decide whether to skip caching a serialized response based on its
 * UTF-8 byte length, not `serialized.length` (which is UTF-16 code units).
 * ASCII chars are 1:1, but a single CJK char is 3 UTF-8 bytes and an
 * emoji is 4 — a code-unit check would under-count and let oversized
 * payloads through. ioredis ultimately encodes the string to UTF-8 for
 * its send buffer and the Valkey wire, which is the property we want
 * to bound.
 */
export function shouldSkipCacheSet(serialized: string): boolean {
	return Buffer.byteLength(serialized, "utf8") > MAX_CACHEABLE_BYTES
}

/**
 * Valkey-backed response cache for GraphQL Yoga.
 * Uses ioredis client (Valkey is protocol-compatible with Redis).
 *
 * - Shared across all API pods via a single Valkey instance
 * - TTL-based expiry (no entity-based invalidation)
 * - Valkey maxmemory + allkeys-lru eviction handles memory limits
 * - Graceful degradation: cache misses on Valkey errors or timeouts (no request failures)
 * - No per-request retries + 3s command timeout: if Valkey is slow or
 *   unhealthy, a single in-flight command times out cleanly and the request
 *   falls through to the DB. 3s gives enough headroom for the biggest
 *   cacheable responses (up to MAX_CACHEABLE_BYTES) to complete a SET
 *   even when Valkey is under moderate load, while still bounding the
 *   per-request tail wait. Retries are disabled — they doubled tail
 *   latency when Valkey was degraded without improving hit rate.
 * - Skips cache SET for responses over MAX_CACHEABLE_BYTES to bound
 *   per-request memory spikes.
 */
export function createValkeyCache(valkeyUrl: string): Cache {
	const valkey = new Redis(valkeyUrl, {
		// No per-request retries. When Valkey is slow or down, a retry just
		// adds to tail latency and doesn't improve hit rate — better to fail
		// fast and fall through to the DB.
		maxRetriesPerRequest: 0,
		lazyConnect: true,
		connectTimeout: 2000,
		// 3s per command covers the worst case of a ~15 MB SET under
		// moderate load (~1-2s on a degraded network) while still
		// bounding how long a single request will wait on an unhealthy
		// cache before falling through to the DB.
		commandTimeout: 3000,
		retryStrategy(times) {
			// Reconnect with exponential backoff, max 5s
			return Math.min(times * 500, 5000)
		},
	})

	valkey.on("error", (err) => {
		log.warn("Valkey cache error", {error: String(err)})
	})

	valkey.on("connect", () => {
		log.info("Valkey cache connected")
	})

	valkey.connect().catch((err) => {
		log.warn("Valkey cache initial connection failed, will retry", {error: String(err)})
	})

	return {
		async set(id, data, _entities, ttl) {
			try {
				const serialized = JSON.stringify(data)
				const bytes = Buffer.byteLength(serialized, "utf8")
				if (bytes > MAX_CACHEABLE_BYTES) {
					log.debug("Valkey cache skip: response exceeds MAX_CACHEABLE_BYTES", {
						cacheKey: id.slice(0, 64),
						bytes,
						limit: MAX_CACHEABLE_BYTES,
					})
					return
				}
				// TTL is in milliseconds from the plugin, EX expects seconds
				const ttlSeconds = Math.ceil(ttl / 1000)
				await valkey.set(id, serialized, "EX", ttlSeconds)
			} catch (err) {
				log.warn("Valkey cache set failed", {error: String(err), cacheKey: id.slice(0, 64)})
			}
		},

		async get(id) {
			try {
				const cached = await valkey.get(id)
				if (cached === null) return undefined
				return JSON.parse(cached) as ExecutionResult
			} catch (err) {
				log.warn("Valkey cache get failed", {error: String(err), cacheKey: id.slice(0, 64)})
				return undefined
			}
		},

		async invalidate(_entities) {
			// No-op: we rely on TTL expiry, not entity-based invalidation.
			// Valkey maxmemory + allkeys-lru handles eviction when memory is full.
		},
	}
}
