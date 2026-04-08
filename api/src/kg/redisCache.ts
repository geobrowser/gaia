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

/** Max time to wait for a Redis operation before treating it as a miss. */
const REDIS_COMMAND_TIMEOUT_MS = 500

/**
 * Race a promise against a timeout. Returns undefined if the timeout fires first.
 */
function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T | undefined> {
	return Promise.race([
		promise,
		new Promise<undefined>((resolve) => setTimeout(resolve, ms)),
	])
}

/**
 * Redis-backed response cache for GraphQL Yoga.
 *
 * - Shared across all API pods via a single Redis instance
 * - TTL-based expiry (no entity-based invalidation)
 * - Redis maxmemory + allkeys-lru eviction handles memory limits
 * - Graceful degradation: cache misses on Redis errors or timeouts (no request failures)
 * - 500ms command timeout: if Redis is slow/hanging, requests fall through to DB
 */
export function createRedisCache(redisUrl: string): Cache {
	const redis = new Redis(redisUrl, {
		maxRetriesPerRequest: 1,
		lazyConnect: true,
		connectTimeout: 2000,
		commandTimeout: REDIS_COMMAND_TIMEOUT_MS,
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
				await withTimeout(redis.set(id, serialized, "EX", ttlSeconds), REDIS_COMMAND_TIMEOUT_MS)
			} catch (err) {
				log.warn("Redis cache set failed", {error: String(err), cacheKey: id.slice(0, 64)})
			}
		},

		async get(id) {
			try {
				const cached = await withTimeout(redis.get(id), REDIS_COMMAND_TIMEOUT_MS)
				if (cached === null || cached === undefined) return undefined
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
