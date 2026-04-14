import {Hono} from "hono"
import {beforeEach, describe, expect, it} from "vitest"
import type {ApiKeyLookup, ApiKeyResult} from "../../services/rateLimit/apiKeys"
import {parseCidr} from "../../services/rateLimit/cidr"
import type {RateLimitConfig} from "../../services/rateLimit/config"
import type {OverrideLookup} from "../../services/rateLimit/overrides"
import type {RateLimitStore} from "../../services/rateLimit/store"
import {rateLimit} from "../rateLimit"

function fakeStore(): RateLimitStore & {calls: number; failNext: boolean; lastId: string | null} {
	const counters = new Map<string, number>()
	return {
		calls: 0,
		failNext: false,
		lastId: null,
		async incrementAndGet(identifier) {
			this.calls++
			this.lastId = identifier
			if (this.failNext) return null
			const next = (counters.get(identifier) ?? 0) + 1
			counters.set(identifier, next)
			return {count: next, resetSeconds: 30}
		},
	}
}

function fakeOverrides(map: Record<string, number | null>): OverrideLookup {
	return {
		async lookup(ip) {
			return map[ip] ?? null
		},
		size() {
			return 0
		},
	}
}

function fakeApiKeys(map: Record<string, ApiKeyResult>): ApiKeyLookup {
	return {
		async lookup(key) {
			return map[key] ?? {found: false}
		},
		size() {
			return 0
		},
	}
}

function makeApp(
	config: RateLimitConfig,
	store: RateLimitStore,
	overrides: OverrideLookup,
	keys: ApiKeyLookup = fakeApiKeys({}),
) {
	const app = new Hono()
	app.use("/protected", rateLimit({config, store, overrides, apiKeys: keys}))
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
		expect(store.calls).toBe(0)
	})

	it("fails open when Valkey returns null", async () => {
		store.failNext = true
		const app = makeApp(baseConfig, store, fakeOverrides({}))
		const res = await app.request("/protected", {headers})
		expect(res.status).toBe(200)
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
		for (let i = 0; i < 3; i++) {
			await app.request("/protected", {headers: {"x-forwarded-for": "1.1.1.1"}})
		}
		const aBlocked = await app.request("/protected", {headers: {"x-forwarded-for": "1.1.1.1"}})
		expect(aBlocked.status).toBe(429)
		const bOk = await app.request("/protected", {headers: {"x-forwarded-for": "2.2.2.2"}})
		expect(bOk.status).toBe(200)
	})

	// ─── API key tests ───────────────────────────────────────────────

	it("API key with custom limit uses key-based counter", async () => {
		const keys = fakeApiKeys({
			"test-key-123": {found: true, requestsPerMin: 2, clientName: "test-client"},
		})
		const app = makeApp(baseConfig, store, fakeOverrides({}), keys)
		const h = {...headers, "x-api-key": "test-key-123"}

		const r1 = await app.request("/protected", {headers: h})
		expect(r1.status).toBe(200)
		expect(r1.headers.get("RateLimit-Limit")).toBe("2")
		expect(r1.headers.get("RateLimit-Remaining")).toBe("1")
		// Counter keyed by "key:<apikey>" not by IP
		expect(store.lastId).toBe("key:test-key-123")

		const r2 = await app.request("/protected", {headers: h})
		expect(r2.status).toBe(200)

		const r3 = await app.request("/protected", {headers: h})
		expect(r3.status).toBe(429)
	})

	it("API key with unlimited (null) skips counter entirely", async () => {
		const keys = fakeApiKeys({
			"unlimited-key": {found: true, requestsPerMin: null, clientName: "railway"},
		})
		const app = makeApp(baseConfig, store, fakeOverrides({}), keys)
		const h = {...headers, "x-api-key": "unlimited-key"}

		for (let i = 0; i < 10; i++) {
			const res = await app.request("/protected", {headers: h})
			expect(res.status).toBe(200)
		}
		// No Valkey calls — unlimited means no counter
		expect(store.calls).toBe(0)
	})

	it("API key with limit=0 blocks immediately", async () => {
		const keys = fakeApiKeys({
			"blocked-key": {found: true, requestsPerMin: 0, clientName: "blocked"},
		})
		const app = makeApp(baseConfig, store, fakeOverrides({}), keys)
		const res = await app.request("/protected", {headers: {...headers, "x-api-key": "blocked-key"}})
		expect(res.status).toBe(429)
		expect(res.headers.get("RateLimit-Limit")).toBe("0")
		expect(store.calls).toBe(0)
	})

	it("unknown API key falls through to IP-based limiting", async () => {
		const app = makeApp(baseConfig, store, fakeOverrides({}), fakeApiKeys({}))
		const res = await app.request("/protected", {headers: {...headers, "x-api-key": "bad-key"}})
		expect(res.status).toBe(200)
		// Should have used IP-based counter, not key-based
		expect(store.lastId).toBe("203.0.113.42")
	})

	it("disabled API key falls through to IP-based limiting", async () => {
		const keys = fakeApiKeys({
			"disabled-key": {found: false}, // disabled keys return found:false
		})
		const app = makeApp(baseConfig, store, fakeOverrides({}), keys)
		const res = await app.request("/protected", {headers: {...headers, "x-api-key": "disabled-key"}})
		expect(res.status).toBe(200)
		expect(store.lastId).toBe("203.0.113.42")
	})

	it("API key counter is separate from IP counter", async () => {
		const keys = fakeApiKeys({
			"my-key": {found: true, requestsPerMin: 3, clientName: "test"},
		})
		const app = makeApp(baseConfig, store, fakeOverrides({}), keys)

		// Exhaust IP limit
		for (let i = 0; i < 3; i++) {
			await app.request("/protected", {headers})
		}
		const ipBlocked = await app.request("/protected", {headers})
		expect(ipBlocked.status).toBe(429)

		// Same IP but with API key → uses key counter (fresh), should still work
		const keyOk = await app.request("/protected", {headers: {...headers, "x-api-key": "my-key"}})
		expect(keyOk.status).toBe(200)
		expect(keyOk.headers.get("RateLimit-Remaining")).toBe("2")
	})
})
