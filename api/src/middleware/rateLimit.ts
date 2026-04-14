import type {MiddlewareHandler} from "hono"
import type {ApiKeyLookup} from "../services/rateLimit/apiKeys"
import {ipInAnyCidr} from "../services/rateLimit/cidr"
import type {RateLimitConfig} from "../services/rateLimit/config"
import type {OverrideLookup} from "../services/rateLimit/overrides"
import type {RateLimitStore} from "../services/rateLimit/store"
import {log} from "../services/telemetry"
import {extractClientIp} from "./clientIp"

/**
 * Per-IP / per-API-key rate limit middleware for the GraphQL HTTP endpoint.
 *
 * Tier resolution (first match wins):
 *   0. X-Api-Key header      → DB lookup: unlimited (null) or custom limit, counter keyed by "key:<key>"
 *   1. CIDR unlimited-allowlist from env → unlimited (counter not incremented, no headers)
 *   2. DB IP override         → use the row's `requests_per_min`, counter keyed by IP
 *   3. Default                → `defaultPerMinute` from env, counter keyed by IP
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
	apiKeys: ApiKeyLookup
}

const STATUS_RATE_LIMITED = 429

export function rateLimit(deps: RateLimitDeps): MiddlewareHandler {
	const {config, store, overrides, apiKeys} = deps

	return async (c, next) => {
		if (!config.enabled) return next()

		// Tier 0: API key — takes precedence over everything else.
		const apiKey = c.req.header("x-api-key")
		if (apiKey) {
			const keyResult = await apiKeys.lookup(apiKey)
			if (keyResult.found) {
				// Unlimited key (requestsPerMin is null) → skip counter entirely
				if (keyResult.requestsPerMin === null) return next()

				const keyLimit = keyResult.requestsPerMin
				if (keyLimit === 0) {
					c.header("RateLimit-Limit", "0")
					c.header("RateLimit-Remaining", "0")
					c.header("Retry-After", "60")
					return c.json({error: "rate_limit_exceeded", retry_after_seconds: 60}, STATUS_RATE_LIMITED)
				}

				// Counter keyed by API key, not IP
				const result = await store.incrementAndGet(`key:${apiKey}`, Date.now())
				if (result === null) return next()

				const remaining = Math.max(0, keyLimit - result.count)
				c.header("RateLimit-Limit", String(keyLimit))
				c.header("RateLimit-Remaining", String(remaining))
				c.header("RateLimit-Reset", String(result.resetSeconds))

				if (result.count > keyLimit) {
					c.header("Retry-After", String(result.resetSeconds))
					log.warn("rate limit: blocked request (API key)", {
						clientName: keyResult.clientName,
						limit: keyLimit,
						count: result.count,
						path: c.req.path,
					})
					return c.json(
						{error: "rate_limit_exceeded", retry_after_seconds: result.resetSeconds},
						STATUS_RATE_LIMITED,
					)
				}

				return next()
			}
			// Key not found or disabled → fall through to IP-based limiting
		}

		const ip = extractClientIp(c, config.trustedProxyHops)
		if (!ip) {
			log.warn("rate limit: no client IP found, allowing request", {
				path: c.req.path,
				method: c.req.method,
			})
			return next()
		}

		// Tier 1: unlimited allowlist (cluster-internal IPs, our own backend services).
		if (ipInAnyCidr(ip, config.unlimitedAllowlist)) return next()

		// Tier 2 / 3: DB IP override or default.
		const overrideLimit = await overrides.lookup(ip)
		const limit = overrideLimit ?? config.defaultPerMinute

		if (limit === 0) {
			c.header("RateLimit-Limit", "0")
			c.header("RateLimit-Remaining", "0")
			c.header("Retry-After", "60")
			return c.json({error: "rate_limit_exceeded", retry_after_seconds: 60}, STATUS_RATE_LIMITED)
		}

		const result = await store.incrementAndGet(ip, Date.now())
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
