import type Redis from "ioredis"
import {log} from "../telemetry"

/**
 * Thin Valkey-backed counter for fixed-window minute buckets.
 *
 * Why a fixed window: simplest sane semantics, atomic INCR, one round-trip per
 * request. The boundary effect (a client gets 2× their limit straddling the
 * window edge) is acceptable for a soft-protection rate limiter.
 *
 * Failure mode: every operation is wrapped to never throw. On Valkey error or
 * timeout we return `null` so the caller can fail open (allow the request, log
 * a warning). This matches the response cache's posture and keeps a Valkey
 * outage from taking the API down.
 */
export type RateLimitStore = {
	/**
	 * Atomically increment the counter for `(identifier, minute)` and return the
	 * new count + seconds-until-reset. The identifier is either an IP address or
	 * an API key prefixed with "key:" to keep namespaces separate.
	 * Returns `null` on Valkey failure.
	 */
	incrementAndGet(identifier: string, nowMs: number): Promise<{count: number; resetSeconds: number} | null>
}

const KEY_PREFIX = "rl:"
const WINDOW_SECONDS = 60
// EXPIRE on the bucket key. Slight TTL padding so Valkey never drops the key
// before the window legitimately ends, which would let counters reset early.
const EXPIRE_SECONDS = WINDOW_SECONDS * 2

export function createValkeyRateLimitStore(valkey: Redis): RateLimitStore {
	return {
		async incrementAndGet(identifier, nowMs) {
			try {
				const minute = Math.floor(nowMs / 1000 / WINDOW_SECONDS)
				const key = `${KEY_PREFIX}${identifier}:${minute}`

				// Pipeline INCR + EXPIRE in a single round-trip. We send EXPIRE
				// every time (cheap; idempotent) so the key's lifetime is always
				// refreshed during active windows.
				const pipeline = valkey.pipeline()
				pipeline.incr(key)
				pipeline.expire(key, EXPIRE_SECONDS)
				const results = await pipeline.exec()

				if (!results || results.length === 0) return null
				const incrResult = results[0]
				if (!incrResult || incrResult[0] !== null) return null

				const count = Number(incrResult[1])
				if (!Number.isFinite(count)) return null

				const windowStartMs = minute * WINDOW_SECONDS * 1000
				const elapsedMs = nowMs - windowStartMs
				const resetSeconds = Math.max(0, Math.ceil((WINDOW_SECONDS * 1000 - elapsedMs) / 1000))

				return {count, resetSeconds}
			} catch (err) {
				log.warn("rate limit: Valkey increment failed", {error: String(err), identifier})
				return null
			}
		},
	}
}
