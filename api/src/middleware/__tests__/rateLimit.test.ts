import {Hono} from "hono"
import {beforeEach, describe, expect, it} from "vitest"
import {parseCidr} from "../../services/rateLimit/cidr"
import type {RateLimitConfig} from "../../services/rateLimit/config"
import type {OverrideLookup} from "../../services/rateLimit/overrides"
import type {RateLimitStore} from "../../services/rateLimit/store"
import {rateLimit} from "../rateLimit"

function fakeStore(): RateLimitStore & {calls: number; failNext: boolean} {
	const counters = new Map<string, number>()
	return {
		calls: 0,
		failNext: false,
		async incrementAndGet(ip) {
			this.calls++
			if (this.failNext) return null
			const next = (counters.get(ip) ?? 0) + 1
			counters.set(ip, next)
			return {count: next, resetSeconds: 30}
		},
	}
}

function fakeOverrides(map: Record<string, number | null>): OverrideLookup {
	return {
		async lookup(ip) {
			return map[ip] ?? null
		},
	}
}

function makeApp(config: RateLimitConfig, store: RateLimitStore, overrides: OverrideLookup) {
	const app = new Hono()
	app.use("/protected", rateLimit({config, store, overrides}))
	app.get("/protected", (c) => c.json({ok: true}))
	return app
}

const baseConfig: RateLimitConfig = {
	enabled: true,
	defaultPerMinute: 3,
	unlimitedAllowlist: [],
	overrideCacheTtlSeconds: 60,
	trustedProxyHops: 1,
}

const headers = {"x-forwarded-for": "203.0.113.42"}

describe("rateLimit middleware", () => {
	let store: ReturnType<typeof fakeStore>

	beforeEach(() => {
		store = fakeStore()
	})

	it("passes through when disabled", async () => {
		const app = makeApp({...baseConfig, enabled: false}, store, fakeOverrides({}))
		const res = await app.request("/protected", {headers})
		expect(res.status).toBe(200)
		expect(store.calls).toBe(0)
	})

	it("allows under default limit and sets headers", async () => {
		const app = makeApp(baseConfig, store, fakeOverrides({}))
		const res = await app.request("/protected", {headers})
		expect(res.status).toBe(200)
		expect(res.headers.get("RateLimit-Limit")).toBe("3")
		expect(res.headers.get("RateLimit-Remaining")).toBe("2")
		expect(res.headers.get("RateLimit-Reset")).toBe("30")
	})

	it("blocks with 429 when default limit exceeded", async () => {
		const app = makeApp(baseConfig, store, fakeOverrides({}))
		await app.request("/protected", {headers})
		await app.request("/protected", {headers})
		await app.request("/protected", {headers})
		const blocked = await app.request("/protected", {headers})
		expect(blocked.status).toBe(429)
		expect(blocked.headers.get("Retry-After")).toBe("30")
		const body = (await blocked.json()) as {error: string; retry_after_seconds: number}
		expect(body.error).toBe("rate_limit_exceeded")
		expect(body.retry_after_seconds).toBe(30)
	})

	it("does not consume counter for allowlisted IPs", async () => {
		const config = {...baseConfig, unlimitedAllowlist: [parseCidr("203.0.113.0/24")!]}
		const app = makeApp(config, store, fakeOverrides({}))
		// Do 10 requests — would normally exceed the limit of 3
		for (let i = 0; i < 10; i++) {
			const res = await app.request("/protected", {headers})
			expect(res.status).toBe(200)
		}
		expect(store.calls).toBe(0)
	})

	it("uses DB override when present", async () => {
		const overrides = fakeOverrides({"203.0.113.42": 1})
		const app = makeApp(baseConfig, store, overrides)
		const ok = await app.request("/protected", {headers})
		expect(ok.status).toBe(200)
		expect(ok.headers.get("RateLimit-Limit")).toBe("1")
		const blocked = await app.request("/protected", {headers})
		expect(blocked.status).toBe(429)
	})

	it("override of 0 blocks immediately as kill switch", async () => {
		const overrides = fakeOverrides({"203.0.113.42": 0})
		const app = makeApp(baseConfig, store, overrides)
		const res = await app.request("/protected", {headers})
		expect(res.status).toBe(429)
		expect(res.headers.get("RateLimit-Limit")).toBe("0")
		// Should not have consumed any Valkey call for the kill-switched IP
		expect(store.calls).toBe(0)
	})

	it("fails open when Valkey returns null", async () => {
		store.failNext = true
		const app = makeApp(baseConfig, store, fakeOverrides({}))
		const res = await app.request("/protected", {headers})
		expect(res.status).toBe(200)
		// No rate limit headers when fail-open
		expect(res.headers.get("RateLimit-Limit")).toBeNull()
	})

	it("allows request through when no client IP detected", async () => {
		const app = makeApp(baseConfig, store, fakeOverrides({}))
		const res = await app.request("/protected") // no headers
		expect(res.status).toBe(200)
		expect(store.calls).toBe(0)
	})

	it("counters are isolated per IP", async () => {
		const app = makeApp(baseConfig, store, fakeOverrides({}))
		// Spend the limit for IP A
		for (let i = 0; i < 3; i++) {
			await app.request("/protected", {headers: {"x-forwarded-for": "1.1.1.1"}})
		}
		const aBlocked = await app.request("/protected", {headers: {"x-forwarded-for": "1.1.1.1"}})
		expect(aBlocked.status).toBe(429)
		// IP B is unaffected
		const bOk = await app.request("/protected", {headers: {"x-forwarded-for": "2.2.2.2"}})
		expect(bOk.status).toBe(200)
	})
})
