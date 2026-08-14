/**
 * Per-IP request rate limiting.
 *
 * Shares the same Valkey instance as the GraphQL response cache (see
 * ../kg/valkeyCache.ts) rather than opening a second connection pool for a
 * lightweight INCR/PEXPIRE workload — Valkey maxmemory + allkeys-lru already
 * bounds memory for both uses.
 *
 * Disabled gracefully if VALKEY_URL is not set, and fails open on any
 * Valkey error or timeout: this middleware runs on every request, so a rate
 * limiter that can take the whole API down during a Valkey blip is worse
 * than no rate limiter at all. Mirrors the failure-handling philosophy in
 * valkeyCache.ts.
 *
 * Per-IP is a blunt instrument — a shared NAT/VPN/office egress IP means
 * many real users can share one bucket — so the default is deliberately
 * generous (10 req/s sustained). This is meant to stop a single scraper
 * hammering the API from one address, not to rate-limit legitimate
 * interactive use.
 */
import type {Context, Next} from "hono"
import Redis from "ioredis"
import {log} from "../services/telemetry"
import {extractClientIp} from "../utils/clientIp"

const DEFAULT_WINDOW_MS = 60_000
const DEFAULT_MAX_REQUESTS = 600

const HEALTH_PATH_PREFIX = "/health"

/**
 * Atomically increments the per-key counter and arms its expiry only on the
 * window's first hit, so a crash between INCR and PEXPIRE can never leave a
 * counter that lives forever. Returns the post-increment count.
 */
const INCR_WITH_TTL_SCRIPT = `
local count = redis.call("INCR", KEYS[1])
if count == 1 then
	redis.call("PEXPIRE", KEYS[1], ARGV[1])
end
return count
`

/**
 * Minimal surface this middleware needs from a Redis/Valkey client —
 * loose enough to accept a real ioredis instance or a test fake without
 * fighting ioredis's own overloaded `eval` typings.
 */
export type RateLimitClient = {
	eval: (...args: unknown[]) => Promise<unknown>
}

export type RateLimitOptions = {
	windowMs?: number
	maxRequests?: number
}

/**
 * Fixed-window per-IP rate limiter middleware, built against an
 * already-connected client. Exported separately from createRateLimiter so
 * the limiting logic is testable against a fake client instead of a real
 * ioredis connection.
 */
export function createRateLimitMiddleware(client: RateLimitClient, options: RateLimitOptions = {}) {
	const windowMs = options.windowMs ?? DEFAULT_WINDOW_MS
	const maxRequests = options.maxRequests ?? DEFAULT_MAX_REQUESTS

	return async (c: Context, next: Next) => {
		// High-frequency, low-value traffic — already excluded from tracing/
		// logging for the same reason (see main.ts).
		if (c.req.path.startsWith(HEALTH_PATH_PREFIX)) {
			await next()
			return
		}

		const clientIp = extractClientIp(c.req.raw.headers)
		// No trustworthy IP — fail open rather than lump every such request
		// into one shared "unknown" bucket, which would rate-limit everyone
		// behind a misconfigured/unexpected hop together.
		if (!clientIp) {
			await next()
			return
		}

		const key = `ratelimit:${clientIp}`

		let count: number
		try {
			count = Number(await client.eval(INCR_WITH_TTL_SCRIPT, 1, key, windowMs))
		} catch (err) {
			log.warn("Rate limiter check failed, allowing request", {error: String(err), clientIp})
			await next()
			return
		}

		if (count > maxRequests) {
			const requestId = (c.get("requestId") as string | undefined) || "unknown"
			const retryAfterSeconds = Math.ceil(windowMs / 1000)
			log.warn("Rate limit exceeded", {clientIp, count, maxRequests, requestId})
			return new Response(JSON.stringify({error: "Too many requests", requestId}), {
				status: 429,
				headers: new Headers({
					"content-type": "application/json",
					"retry-after": String(retryAfterSeconds),
					"x-request-id": requestId,
				}),
			})
		}

		await next()
	}
}

/**
 * Builds the production rate limiter against the shared Valkey instance.
 * Disabled gracefully if VALKEY_URL is not set, matching ../kg/valkeyCache.ts
 * so an unconfigured cache doesn't also silently disable rate limiting (or
 * vice versa) — this stays independently controllable via the same var.
 */
export function createRateLimiter(valkeyUrl: string | undefined, options: RateLimitOptions = {}) {
	if (!valkeyUrl) {
		log.info("Rate limiting disabled (VALKEY_URL not set)")
		return null
	}

	const valkey = new Redis(valkeyUrl, {
		// No per-request retries. On a slow/unhealthy Valkey, a retry just adds
		// tail latency to every request without improving anything — better to
		// fail fast and let the request through.
		maxRetriesPerRequest: 0,
		lazyConnect: true,
		connectTimeout: 2000,
		// Short timeout: unlike the response cache (occasional large SETs),
		// this runs on every single request, so a slow Valkey must fail fast
		// rather than add tail latency across the whole API.
		commandTimeout: 1000,
		retryStrategy(times) {
			return Math.min(times * 500, 5000)
		},
	})

	valkey.on("error", (err) => {
		log.warn("Rate limiter Valkey error", {error: String(err)})
	})

	valkey.on("connect", () => {
		log.info("Rate limiter Valkey connected")
	})

	valkey.connect().catch((err) => {
		log.warn("Rate limiter Valkey initial connection failed, will retry", {error: String(err)})
	})

	log.info("Rate limiting enabled", {
		windowMs: options.windowMs ?? DEFAULT_WINDOW_MS,
		maxRequests: options.maxRequests ?? DEFAULT_MAX_REQUESTS,
	})

	// Bridges ioredis's real (overloaded) `eval` signature to the minimal
	// RateLimitClient contract at a single, explicit call site, rather than
	// trying to make RateLimitClient structurally match ioredis's typings.
	const client: RateLimitClient = {
		eval: (script, numKeys, key, arg) =>
			valkey.eval(script as string, numKeys as number, key as string, arg as string | number),
	}

	return createRateLimitMiddleware(client, options)
}
