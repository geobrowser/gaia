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
 * Skip caching responses larger than this threshold. Serializing a large
 * response already allocates a second full copy of the data (JSON.stringify),
 * and handing that string to ioredis allocates a third in its send buffer.
 * For 30+ MB responses, this is the exact allocation spike that OOM'd pods
 * before the pagination cap landed. Skipping the SET entirely avoids the
 * ioredis buffer and the network send — the stringify still happens, but
 * the peak is bounded by one copy instead of three.
 *
 * 15 MB is chosen to cover the full range of expected cacheable responses:
 *   - the 12 MB Query.spaces response (near-static, high-value to cache)
 *   - 2–15 MB Query.entities* / entitiesOrderedByProperty responses
 *   - common 2–5 MB entity-list shapes
 *
 * Per-request memory impact stays bounded:
 *   - Single SET peak = JS string (15 MB) + ioredis send buffer (15 MB) ≈
 *     30 MB per concurrent write — 0.7% of the 4 Gi pod limit
 *   - 8-pod cache stampede peaks at ~240 MB aggregate; each pod stays
 *     under 30 MB incremental during the stampede
 *   - SET over in-cluster networking is typically <200 ms; the 3 s
 *     commandTimeout gives ~1.5× headroom for bad network days
 *
 * GET path on a cache hit parses the full JSON to rebuild the JS object
 * tree — ~100–250 ms CPU and up to ~90 MB transient memory per pod on
 * the biggest entries. For expensive queries this is still a net win over
 * hitting the DB; for cheap queries it can approach DB cost.
 *
 * Responses over 15 MB fall through to the DB on every request rather
 * than cache, bounding the worst-case memory allocation path.
 */
export const MAX_CACHEABLE_BYTES = 15_000_000

export function shouldSkipCacheSet(serialized: string): boolean {
	return serialized.length > MAX_CACHEABLE_BYTES
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
				if (shouldSkipCacheSet(serialized)) {
					log.debug("Valkey cache skip: response exceeds MAX_CACHEABLE_BYTES", {
						cacheKey: id.slice(0, 64),
						bytes: serialized.length,
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
