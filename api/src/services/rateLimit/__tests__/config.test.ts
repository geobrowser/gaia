import {describe, expect, it} from "vitest"
import {loadRateLimitConfig} from "../config"

describe("loadRateLimitConfig", () => {
	it("uses defaults when env is empty", () => {
		const config = loadRateLimitConfig({})
		expect(config.enabled).toBe(true)
		expect(config.defaultPerMinute).toBe(1000)
		expect(config.overrideCacheTtlSeconds).toBe(60)
		expect(config.trustedProxyHops).toBe(1)
		expect(config.unlimitedAllowlist).toEqual([])
	})

	it("honors RATE_LIMIT_ENABLED=false", () => {
		const config = loadRateLimitConfig({RATE_LIMIT_ENABLED: "false"})
		expect(config.enabled).toBe(false)
	})

	it("treats any other value as enabled", () => {
		expect(loadRateLimitConfig({RATE_LIMIT_ENABLED: "true"}).enabled).toBe(true)
		expect(loadRateLimitConfig({RATE_LIMIT_ENABLED: "yes"}).enabled).toBe(true)
		expect(loadRateLimitConfig({}).enabled).toBe(true)
	})

	it("parses default per-minute from env", () => {
		expect(loadRateLimitConfig({RATE_LIMIT_DEFAULT_PER_MINUTE: "500"}).defaultPerMinute).toBe(500)
	})

	it("falls back when default per-minute is invalid", () => {
		expect(loadRateLimitConfig({RATE_LIMIT_DEFAULT_PER_MINUTE: "abc"}).defaultPerMinute).toBe(1000)
		expect(loadRateLimitConfig({RATE_LIMIT_DEFAULT_PER_MINUTE: "-5"}).defaultPerMinute).toBe(1000)
		expect(loadRateLimitConfig({RATE_LIMIT_DEFAULT_PER_MINUTE: "1.5"}).defaultPerMinute).toBe(1000)
	})

	it("parses comma-separated allowlist", () => {
		const config = loadRateLimitConfig({
			RATE_LIMIT_UNLIMITED_ALLOWLIST_IPS: "10.0.0.1, 192.168.0.0/24 , 203.0.113.42",
		})
		expect(config.unlimitedAllowlist).toHaveLength(3)
		expect(config.unlimitedAllowlist.map((c) => c.source)).toEqual(["10.0.0.1", "192.168.0.0/24", "203.0.113.42"])
	})

	it("ignores unparseable allowlist entries", () => {
		const config = loadRateLimitConfig({
			RATE_LIMIT_UNLIMITED_ALLOWLIST_IPS: "10.0.0.1,bad-entry,192.168.0.0/24,256.0.0.0",
		})
		expect(config.unlimitedAllowlist).toHaveLength(2)
	})

	it("treats empty allowlist string as no entries", () => {
		expect(loadRateLimitConfig({RATE_LIMIT_UNLIMITED_ALLOWLIST_IPS: ""}).unlimitedAllowlist).toEqual([])
	})
})
