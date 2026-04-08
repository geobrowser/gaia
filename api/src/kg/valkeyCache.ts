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
 * Valkey-backed response cache for GraphQL Yoga.
 * Uses ioredis client (Valkey is protocol-compatible with Redis).
 *
 * - Shared across all API pods via a single Valkey instance
 * - TTL-based expiry (no entity-based invalidation)
 * - Valkey maxmemory + allkeys-lru eviction handles memory limits
 * - Graceful degradation: cache misses on Valkey errors or timeouts (no request failures)
 * - 500ms command timeout via ioredis commandTimeout: if Valkey is slow/hanging, requests fall through to DB
 */
export function createValkeyCache(valkeyUrl: string): Cache {
	const valkey = new Redis(valkeyUrl, {
		maxRetriesPerRequest: 1,
		lazyConnect: true,
		connectTimeout: 2000,
		commandTimeout: 500,
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
