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
 * Redis-backed response cache for GraphQL Yoga.
 *
 * - Shared across all API pods via a single Redis instance
 * - TTL-based expiry (no entity-based invalidation)
 * - Redis maxmemory + allkeys-lru eviction handles memory limits
 * - Graceful degradation: cache misses on Redis errors (no request failures)
 */
export function createRedisCache(redisUrl: string): Cache {
	const redis = new Redis(redisUrl, {
		maxRetriesPerRequest: 1,
		lazyConnect: true,
		retryStrategy(times) {
			// Reconnect with exponential backoff, max 5s
			return Math.min(times * 500, 5000)
		},
	})

	redis.on("error", (err) => {
		log.warn("Redis cache error", {error: String(err)})
	})

	redis.on("connect", () => {
		log.info("Redis cache connected")
	})

	redis.connect().catch((err) => {
		log.warn("Redis cache initial connection failed, will retry", {error: String(err)})
	})

	return {
		async set(id, data, _entities, ttl) {
			try {
				const serialized = JSON.stringify(data)
				// TTL is in milliseconds from the plugin, Redis EX expects seconds
				const ttlSeconds = Math.ceil(ttl / 1000)
				await redis.set(id, serialized, "EX", ttlSeconds)
			} catch (err) {
				log.warn("Redis cache set failed", {error: String(err), cacheKey: id.slice(0, 64)})
			}
		},

		async get(id) {
			try {
				const cached = await redis.get(id)
				if (cached === null) return undefined
				return JSON.parse(cached) as ExecutionResult
			} catch (err) {
				log.warn("Redis cache get failed", {error: String(err), cacheKey: id.slice(0, 64)})
				return undefined
			}
		},

		async invalidate(_entities) {
			// No-op: we rely on TTL expiry, not entity-based invalidation.
			// Redis maxmemory + allkeys-lru handles eviction when memory is full.
		},
	}
}
