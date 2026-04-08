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

/** Max time to wait for a Valkey operation before treating it as a miss. */
const VALKEY_COMMAND_TIMEOUT_MS = 500

/**
 * Race a promise against a timeout. Returns undefined if the timeout fires first.
 */
function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T | undefined> {
	return Promise.race([promise, new Promise<undefined>((resolve) => setTimeout(resolve, ms))])
}

/**
 * Valkey-backed response cache for GraphQL Yoga.
 * Uses ioredis client (Valkey is protocol-compatible with Redis).
 *
 * - Shared across all API pods via a single Valkey instance
 * - TTL-based expiry (no entity-based invalidation)
 * - Valkey maxmemory + allkeys-lru eviction handles memory limits
 * - Graceful degradation: cache misses on Valkey errors or timeouts (no request failures)
 * - 500ms command timeout: if Valkey is slow/hanging, requests fall through to DB
 */
export function createValkeyCache(valkeyUrl: string): Cache {
	const valkey = new Redis(valkeyUrl, {
		maxRetriesPerRequest: 1,
		lazyConnect: true,
		connectTimeout: 2000,
		commandTimeout: VALKEY_COMMAND_TIMEOUT_MS,
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
				await withTimeout(valkey.set(id, serialized, "EX", ttlSeconds), VALKEY_COMMAND_TIMEOUT_MS)
			} catch (err) {
				log.warn("Valkey cache set failed", {error: String(err), cacheKey: id.slice(0, 64)})
			}
		},

		async get(id) {
			try {
				const cached = await withTimeout(valkey.get(id), VALKEY_COMMAND_TIMEOUT_MS)
				if (cached === null || cached === undefined) return undefined
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
