import {log} from "../telemetry"
import {type Cidr, parseCidr} from "./cidr"

/**
 * Runtime configuration for the rate limiter, parsed once at startup from env.
 * All values have safe defaults so the API still boots if env is incomplete.
 */
export type RateLimitConfig = {
	enabled: boolean
	defaultPerMinute: number
	unlimitedAllowlist: Cidr[]
	overrideCacheTtlSeconds: number
	trustedProxyHops: number
}

export function loadRateLimitConfig(env: NodeJS.ProcessEnv = process.env): RateLimitConfig {
	const enabled = env.RATE_LIMIT_ENABLED !== "false" // default on

	const defaultPerMinute = parsePositiveInt(env.RATE_LIMIT_DEFAULT_PER_MINUTE, 1000)
	const overrideCacheTtlSeconds = parsePositiveInt(env.RATE_LIMIT_OVERRIDE_CACHE_TTL_SECONDS, 60)
	const trustedProxyHops = parsePositiveInt(env.RATE_LIMIT_TRUSTED_PROXY_HOPS, 1)

	const rawAllowlist = env.RATE_LIMIT_UNLIMITED_ALLOWLIST_IPS ?? ""
	const unlimitedAllowlist: Cidr[] = []
	for (const entry of rawAllowlist.split(",")) {
		const trimmed = entry.trim()
		if (trimmed === "") continue
		const parsed = parseCidr(trimmed)
		if (parsed === null) {
			log.warn("rate limit: ignoring unparseable allowlist entry", {entry: trimmed})
			continue
		}
		unlimitedAllowlist.push(parsed)
	}

	return {
		enabled,
		defaultPerMinute,
		unlimitedAllowlist,
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
