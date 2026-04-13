import {sql} from "drizzle-orm"
import type {db as Db} from "../storage/storage"
import {log} from "../telemetry"

/**
 * Lookup per-IP rate limit overrides from Postgres with a small in-process LRU
 * cache to keep the hot path off the DB.
 *
 * We use the cache for *both* hits (an explicit limit) and misses (no row
 * matched). A miss is cached as `null` so we don't repeatedly query for IPs
 * that fall through to the default limit.
 *
 * The cache is per-pod, so propagating a new override row takes up to
 * `ttlSeconds` to be picked up by all pods. That's the price of avoiding
 * a Postgres query on every API request, and acceptable for an admin-managed
 * configuration table.
 *
 * Memory bounding: the cache is a fixed-size LRU. When full, the
 * least-recently-used entry is evicted. This caps per-pod memory regardless
 * of unique-IP volume (e.g. a scraper from many IPs cannot grow it without
 * bound). Stale entries are also evicted lazily on read, so a long-idle pod
 * does not keep dead entries around past their TTL.
 */
export type OverrideLookup = {
	/** Returns the per-minute limit for `ip` if an override matches, otherwise `null`. */
	lookup(ip: string): Promise<number | null>
	/** Test/observability hook: current cache size. */
	size(): number
}

type CacheEntry = {limit: number | null; expiresAtMs: number}

export const DEFAULT_OVERRIDE_CACHE_MAX_ENTRIES = 100_000

export function createOverrideLookup(
	db: typeof Db,
	ttlSeconds: number,
	maxEntries: number = DEFAULT_OVERRIDE_CACHE_MAX_ENTRIES,
): OverrideLookup {
	// Map preserves insertion order in JS, which we exploit for LRU:
	// every read of a fresh entry deletes+re-inserts it, moving it to the tail.
	// On overflow, the head (oldest) is evicted via .keys().next().
	const cache = new Map<string, CacheEntry>()
	const ttlMs = ttlSeconds * 1000

	function touchAsRecent(ip: string, entry: CacheEntry): void {
		cache.delete(ip)
		cache.set(ip, entry)
	}

	function evictOldestIfFull(): void {
		while (cache.size >= maxEntries) {
			const oldest = cache.keys().next().value
			if (oldest === undefined) break
			cache.delete(oldest)
		}
	}

	return {
		async lookup(ip) {
			const now = Date.now()
			const cached = cache.get(ip)
			if (cached) {
				if (cached.expiresAtMs > now) {
					touchAsRecent(ip, cached)
					return cached.limit
				}
				// Expired: drop it now rather than waiting for LRU pressure.
				cache.delete(ip)
			}

			let limit: number | null = null
			try {
				// Most-specific containing prefix wins (e.g. /32 over /16).
				// `>>=` is "ip_range contains $1::inet".
				const result = await db.execute<{requests_per_min: number}>(sql`
					SELECT requests_per_min FROM rate_limit_overrides
					WHERE ip_range >>= ${ip}::inet
					ORDER BY masklen(ip_range) DESC
					LIMIT 1
				`)
				const row = result.rows[0]
				if (row) limit = Number(row.requests_per_min)
			} catch (err) {
				// On DB failure, treat as miss (default limit applies). Don't cache
				// the failure — retry on the next request in case it was transient.
				log.warn("rate limit: override lookup failed", {error: String(err), ip})
				return null
			}

			evictOldestIfFull()
			cache.set(ip, {limit, expiresAtMs: now + ttlMs})
			return limit
		},

		size() {
			return cache.size
		},
	}
}
