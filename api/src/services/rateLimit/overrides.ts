import {sql} from "drizzle-orm"
import type {db as Db} from "../storage/storage"
import {log} from "../telemetry"

/**
 * Lookup per-IP rate limit overrides from Postgres with a small in-process
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
 */
export type OverrideLookup = {
	/** Returns the per-minute limit for `ip` if an override matches, otherwise `null`. */
	lookup(ip: string): Promise<number | null>
}

type CacheEntry = {limit: number | null; expiresAtMs: number}

export function createOverrideLookup(db: typeof Db, ttlSeconds: number): OverrideLookup {
	const cache = new Map<string, CacheEntry>()
	const ttlMs = ttlSeconds * 1000

	return {
		async lookup(ip) {
			const now = Date.now()
			const cached = cache.get(ip)
			if (cached && cached.expiresAtMs > now) return cached.limit

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

			cache.set(ip, {limit, expiresAtMs: now + ttlMs})
			return limit
		},
	}
}
