import {log} from "../telemetry"
import {type Cidr, parseCidr} from "./cidr"

/**
 * Runtime configuration for the rate limiter, parsed once at startup from env.
 * All values have safe defaults so the API still boots if env is incomplete.
 */
export type RateLimitConfig = {
	enabled: boolean
	defaultPerMinute: number
	whitelist: Cidr[]
	overrideCacheTtlSeconds: number
	trustedProxyHops: number
}

export function loadRateLimitConfig(env: NodeJS.ProcessEnv = process.env): RateLimitConfig {
	const enabled = env.RATE_LIMIT_ENABLED !== "false" // default on

	const defaultPerMinute = parsePositiveInt(env.RATE_LIMIT_DEFAULT_PER_MINUTE, 1000)
	const overrideCacheTtlSeconds = parsePositiveInt(env.RATE_LIMIT_OVERRIDE_CACHE_TTL_SECONDS, 60)
	const trustedProxyHops = parsePositiveInt(env.RATE_LIMIT_TRUSTED_PROXY_HOPS, 1)

	const rawWhitelist = env.RATE_LIMIT_WHITELIST_IPS ?? ""
	const whitelist: Cidr[] = []
	for (const entry of rawWhitelist.split(",")) {
		const trimmed = entry.trim()
		if (trimmed === "") continue
		const parsed = parseCidr(trimmed)
		if (parsed === null) {
			log.warn("rate limit: ignoring unparseable whitelist entry", {entry: trimmed})
			continue
		}
		whitelist.push(parsed)
	}

	return {
		enabled,
		defaultPerMinute,
		whitelist,
		overrideCacheTtlSeconds,
		trustedProxyHops,
	}
}

function parsePositiveInt(value: string | undefined, fallback: number): number {
	if (!value) return fallback
	const n = Number(value)
	if (!Number.isFinite(n) || n < 0 || !Number.isInteger(n)) return fallback
	return n
}
