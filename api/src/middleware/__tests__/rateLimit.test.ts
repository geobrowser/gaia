import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"

// Mock the structured logger so we can assert on log levels without noise.
vi.mock("../../services/telemetry", () => ({
	log: {
		debug: vi.fn(),
		info: vi.fn(),
		warn: vi.fn(),
		error: vi.fn(),
	},
}))

import {log} from "../../services/telemetry"
import {createRateLimitMiddleware, type RateLimitClient} from "../rateLimit"

/**
 * Fake Valkey client mirroring exactly what the production INCR_WITH_TTL_SCRIPT
 * does (increment, arm expiry only on the first hit), so the middleware's
 * calling contract is exercised without a real Redis/Valkey connection —
 * same philosophy as ../../kg/__tests__/valkeyCache.test.ts's hand-rolled fake.
 */
function createFakeClient(): RateLimitClient & {shouldFail: boolean} {
	const counts = new Map<string, number>()
	return {
		shouldFail: false,
		async eval(...args: unknown[]) {
			if (this.shouldFail) throw new Error("connection refused")
			// args: [script, numKeys, key, arg] — see createRateLimitMiddleware's call.
			const key = args[2] as string
			const next = (counts.get(key) ?? 0) + 1
			counts.set(key, next)
			return next
		},
	}
}

function setupApp(client: RateLimitClient, options?: {windowMs?: number; maxRequests?: number}) {
	const app = new Hono<{Variables: {requestId: string}}>()
	app.use("*", async (c, next) => {
		c.set("requestId", "test-request-id")
		await next()
	})
	app.use("*", createRateLimitMiddleware(client, options))
	app.all("/health", (c) => c.text("ok"))
	app.all("/test", (c) => c.text("ok"))
	return app
}

// biome-ignore lint/suspicious/noExplicitAny: test helper accepts any Hono app env
function requestFrom(app: Hono<any>, path: string, ip: string) {
	return app.request(path, {headers: {"x-real-ip": ip}})
}

describe("createRateLimitMiddleware", () => {
	beforeEach(() => {
		vi.clearAllMocks()
	})

	it("allows requests under the limit", async () => {
		const client = createFakeClient()
		const app = setupApp(client, {maxRequests: 3, windowMs: 60_000})

		for (let i = 0; i < 3; i++) {
			const res = await requestFrom(app, "/test", "1.2.3.4")
			expect(res.status).toBe(200)
		}
	})

	it("blocks the request that exceeds the limit with 429 and Retry-After", async () => {
		const client = createFakeClient()
		const app = setupApp(client, {maxRequests: 2, windowMs: 30_000})

		expect((await requestFrom(app, "/test", "5.6.7.8")).status).toBe(200)
		expect((await requestFrom(app, "/test", "5.6.7.8")).status).toBe(200)

		const blocked = await requestFrom(app, "/test", "5.6.7.8")
		expect(blocked.status).toBe(429)
		expect(blocked.headers.get("retry-after")).toBe("30")
		const body = await blocked.json()
		expect(body).toEqual({error: "Too many requests", requestId: "test-request-id"})
	})

	it("tracks separate IPs independently", async () => {
		const client = createFakeClient()
		const app = setupApp(client, {maxRequests: 1, windowMs: 60_000})

		expect((await requestFrom(app, "/test", "1.1.1.1")).status).toBe(200)
		expect((await requestFrom(app, "/test", "2.2.2.2")).status).toBe(200)
		// Second hit from the first IP is now over its own limit.
		expect((await requestFrom(app, "/test", "1.1.1.1")).status).toBe(429)
	})

	it("fails open when the Valkey client errors", async () => {
		const client = createFakeClient()
		client.shouldFail = true
		const app = setupApp(client, {maxRequests: 1, windowMs: 60_000})

		const res = await requestFrom(app, "/test", "9.9.9.9")
		expect(res.status).toBe(200)
		expect(log.warn).toHaveBeenCalledWith(
			"Rate limiter check failed, allowing request",
			expect.objectContaining({clientIp: "9.9.9.9"}),
		)
	})

	it("fails open when no trustworthy client IP is present", async () => {
		const client = createFakeClient()
		const app = setupApp(client, {maxRequests: 1, windowMs: 60_000})

		// No x-real-ip / x-forwarded-for header at all.
		const first = await app.request("/test")
		const second = await app.request("/test")
		expect(first.status).toBe(200)
		expect(second.status).toBe(200)
	})

	it("skips rate limiting for /health regardless of volume", async () => {
		const client = createFakeClient()
		const app = setupApp(client, {maxRequests: 1, windowMs: 60_000})

		for (let i = 0; i < 5; i++) {
			const res = await requestFrom(app, "/health", "3.3.3.3")
			expect(res.status).toBe(200)
		}
	})

	it("uses the rightmost X-Forwarded-For entry when X-Real-IP is absent", async () => {
		const client = createFakeClient()
		const app = new Hono<{Variables: {requestId: string}}>()
		app.use("*", async (c, next) => {
			c.set("requestId", "test-request-id")
			await next()
		})
		app.use("*", createRateLimitMiddleware(client, {maxRequests: 1, windowMs: 60_000}))
		app.all("/test", (c) => c.text("ok"))

		const headers = {"x-forwarded-for": "client-controlled, 10.0.0.5"}
		expect((await app.request("/test", {headers})).status).toBe(200)
		// Same trusted (rightmost) IP again -> blocked, even though the
		// client-controlled leftmost entry is absent this time.
		const blocked = await app.request("/test", {headers: {"x-forwarded-for": "10.0.0.5"}})
		expect(blocked.status).toBe(429)
	})
})
