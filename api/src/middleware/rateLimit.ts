import type {MiddlewareHandler} from "hono"
import {ipInAnyCidr} from "../services/rateLimit/cidr"
import type {RateLimitConfig} from "../services/rateLimit/config"
import type {OverrideLookup} from "../services/rateLimit/overrides"
import type {RateLimitStore} from "../services/rateLimit/store"
import {log} from "../services/telemetry"
import {extractClientIp} from "./clientIp"

/**
 * Per-IP rate limit middleware for the GraphQL HTTP endpoint.
 *
 * Tier resolution (first match wins):
 *   1. CIDR whitelist from env  → unlimited (counter not incremented, no headers)
 *   2. DB override for this IP  → use the row's `requests_per_min`
 *   3. Default                  → `defaultPerMinute` from env
 *
 * Counter store is shared across all API pods via Valkey. On Valkey failure
 * we **fail open** (allow the request, log warn) since rate limiting is a
 * soft guard, not a security boundary.
 *
 * Sets RFC-style response headers when limited:
 *   RateLimit-Limit, RateLimit-Remaining, RateLimit-Reset, Retry-After (on 429).
 */
export type RateLimitDeps = {
	config: RateLimitConfig
	store: RateLimitStore
	overrides: OverrideLookup
}

const STATUS_RATE_LIMITED = 429

export function rateLimit(deps: RateLimitDeps): MiddlewareHandler {
	const {config, store, overrides} = deps

	return async (c, next) => {
		if (!config.enabled) return next()

		const ip = extractClientIp(c, config.trustedProxyHops)
		if (!ip) {
			// No IP we can identify the request by — let it through but log so we
			// can detect misconfigured ingress. This is rare; ingress-nginx always
			// sets X-Forwarded-For.
			log.warn("rate limit: no client IP found, allowing request", {
				path: c.req.path,
				method: c.req.method,
			})
			return next()
		}

		// Tier 1: whitelist (cluster-internal IPs, our own backend services).
		if (ipInAnyCidr(ip, config.whitelist)) return next()

		// Tier 2 / 3: DB override or default.
		const overrideLimit = await overrides.lookup(ip)
		const limit = overrideLimit ?? config.defaultPerMinute

		// Limit of 0 = block entirely (admin kill switch).
		if (limit === 0) {
			c.header("RateLimit-Limit", "0")
			c.header("RateLimit-Remaining", "0")
			c.header("Retry-After", "60")
			return c.json({error: "rate_limit_exceeded", retry_after_seconds: 60}, STATUS_RATE_LIMITED)
		}

		const result = await store.incrementAndGet(ip, Date.now())
		// Valkey failure → fail open. The store has already logged the warning.
		if (result === null) return next()

		const remaining = Math.max(0, limit - result.count)

		c.header("RateLimit-Limit", String(limit))
		c.header("RateLimit-Remaining", String(remaining))
		c.header("RateLimit-Reset", String(result.resetSeconds))

		if (result.count > limit) {
			c.header("Retry-After", String(result.resetSeconds))
			log.warn("rate limit: blocked request", {
				ip,
				limit,
				count: result.count,
				path: c.req.path,
			})
			return c.json({error: "rate_limit_exceeded", retry_after_seconds: result.resetSeconds}, STATUS_RATE_LIMITED)
		}

		return next()
	}
}
