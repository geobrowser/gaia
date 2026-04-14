import {sql} from "drizzle-orm"
import type {db as Db} from "../storage/storage"
import {log} from "../telemetry"

/**
 * API key lookup for the rate-limit middleware. When a request includes an
 * `X-Api-Key` header, we check this table before falling through to IP-based
 * limiting.
 *
 * Returns:
 *   - `{found: true, requestsPerMin: N}` — key exists and is enabled, use N as limit
 *   - `{found: true, requestsPerMin: null}` — key exists and is unlimited
 *   - `{found: false}` — key not found or disabled → fall through to IP-based
 *
 * Results are cached per-pod in a bounded LRU (same pattern as IP overrides).
 */
export type ApiKeyResult = {found: true; requestsPerMin: number | null; clientName: string} | {found: false}

export type ApiKeyLookup = {
	lookup(key: string): Promise<ApiKeyResult>
	size(): number
}

type CacheEntry = {result: ApiKeyResult; expiresAtMs: number}

const DEFAULT_MAX_ENTRIES = 10_000

export function createApiKeyLookup(
	db: typeof Db,
	ttlSeconds: number,
	maxEntries: number = DEFAULT_MAX_ENTRIES,
): ApiKeyLookup {
	const cache = new Map<string, CacheEntry>()
	const ttlMs = ttlSeconds * 1000

	function touchAsRecent(key: string, entry: CacheEntry): void {
		cache.delete(key)
		cache.set(key, entry)
	}

	function evictOldestIfFull(): void {
		while (cache.size >= maxEntries) {
			const oldest = cache.keys().next().value
			if (oldest === undefined) break
			cache.delete(oldest)
		}
	}

	return {
		async lookup(apiKey) {
			const now = Date.now()
			const cached = cache.get(apiKey)
			if (cached) {
				if (cached.expiresAtMs > now) {
					touchAsRecent(apiKey, cached)
					return cached.result
				}
				cache.delete(apiKey)
			}

			let result: ApiKeyResult = {found: false}
			try {
				const rows = await db.execute<{
					requests_per_min: number | null
					client_name: string
					enabled: boolean
				}>(sql`
					SELECT requests_per_min, client_name, enabled FROM api_keys
					WHERE key = ${apiKey}
					LIMIT 1
				`)
				const row = rows.rows[0]
				if (row && row.enabled) {
					result = {
						found: true,
						requestsPerMin: row.requests_per_min,
						clientName: row.client_name,
					}
				}
			} catch (err) {
				log.warn("rate limit: API key lookup failed", {error: String(err)})
				return {found: false}
			}

			evictOldestIfFull()
			cache.set(apiKey, {result, expiresAtMs: now + ttlMs})
			return result
		},

		size() {
			return cache.size
		},
	}
}
