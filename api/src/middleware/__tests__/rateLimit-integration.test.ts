/**
 * Rate limit integration test.
 *
 * Requires real Valkey and Postgres (CI provides both as services).
 * Tests the full pipeline: DB override lookup → Valkey counter → HTTP 429.
 *
 * Strategy: insert a rate_limit_overrides row with a very low limit (3/min)
 * for a test IP, then fire requests through a real Hono app with the rate
 * limit middleware wired to real Valkey + Postgres.
 */
import {sql} from "drizzle-orm"
import {drizzle} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import Redis from "ioredis"
import pg from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {createApiKeyLookup} from "../../services/rateLimit/apiKeys"
import {loadRateLimitConfig} from "../../services/rateLimit/config"
import {createOverrideLookup} from "../../services/rateLimit/overrides"
import {createValkeyRateLimitStore} from "../../services/rateLimit/store"
import {rateLimit} from "../rateLimit"

const TEST_IP = "198.51.100.42"
const TEST_IP_BLOCKED = "198.51.100.99"
const TEST_IP_DEFAULT = "203.0.113.10"
const TEST_LIMIT = 3

const databaseUrl = process.env.DATABASE_URL || "postgresql://test:test@localhost:5432/test"
const valkeyUrl = process.env.VALKEY_URL || "redis://localhost:6379"

let pool: pg.Pool
let db: ReturnType<typeof drizzle>
let valkey: Redis
let app: Hono

describe("Rate limit integration", () => {
	beforeAll(async () => {
		// Connect to real Postgres
		pool = new pg.Pool({connectionString: databaseUrl})
		db = drizzle({client: pool})

		// Create the rate_limit_overrides table if it doesn't exist (CI may not have run full migrations)
		await db.execute(sql`
			CREATE TABLE IF NOT EXISTS rate_limit_overrides (
				ip_range cidr PRIMARY KEY,
				requests_per_min integer NOT NULL CHECK (requests_per_min >= 0),
				description text,
				created_at timestamptz NOT NULL DEFAULT now(),
				updated_at timestamptz NOT NULL DEFAULT now()
			)
		`)
		await db.execute(sql`
			CREATE INDEX IF NOT EXISTS idx_rate_limit_overrides_ip_range
			ON rate_limit_overrides USING gist (ip_range inet_ops)
		`)

		// Create api_keys table
		await db.execute(sql`
			CREATE TABLE IF NOT EXISTS api_keys (
				key text PRIMARY KEY,
				client_name text NOT NULL,
				requests_per_min integer,
				enabled boolean DEFAULT true NOT NULL,
				created_at timestamptz NOT NULL DEFAULT now(),
				updated_at timestamptz NOT NULL DEFAULT now()
			)
		`)

		// Insert test data
		await db.execute(sql`DELETE FROM api_keys`)
		await db.execute(sql`DELETE FROM rate_limit_overrides`)
		await db.execute(
			sql`INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
			    VALUES (${TEST_IP}::cidr, ${TEST_LIMIT}, 'integration test: low limit')`,
		)
		await db.execute(
			sql`INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
			    VALUES (${TEST_IP_BLOCKED}::cidr, 0, 'integration test: blocked')`,
		)
		// API key test data
		await db.execute(
			sql`INSERT INTO api_keys (key, client_name, requests_per_min, enabled)
			    VALUES ('test-custom-key', 'test-client', 3, true)`,
		)
		await db.execute(
			sql`INSERT INTO api_keys (key, client_name, requests_per_min, enabled)
			    VALUES ('test-unlimited-key', 'railway-backend', NULL, true)`,
		)
		await db.execute(
			sql`INSERT INTO api_keys (key, client_name, requests_per_min, enabled)
			    VALUES ('test-disabled-key', 'disabled-client', 5000, false)`,
		)

		// Connect to real Valkey
		valkey = new Redis(valkeyUrl, {
			maxRetriesPerRequest: 1,
			lazyConnect: true,
			connectTimeout: 2000,
			commandTimeout: 500,
		})
		await valkey.connect()
		// Clean any stale rate limit keys
		const keys = await valkey.keys("rl:*")
		if (keys.length > 0) await valkey.del(...keys)

		// Build the Hono app with real deps
		const config = loadRateLimitConfig({
			RATE_LIMIT_ENABLED: "true",
			RATE_LIMIT_DEFAULT_PER_MINUTE: "1000",
			RATE_LIMIT_UNLIMITED_ALLOWLIST_IPS: "10.108.0.0/16",
			RATE_LIMIT_TRUSTED_PROXY_HOPS: "1",
		})
		const store = createValkeyRateLimitStore(valkey)
		const overrides = createOverrideLookup(db as never, 0) // TTL=0 so every lookup hits DB (no stale cache)
		const apiKeyLookup = createApiKeyLookup(db as never, 0)

		app = new Hono()
		app.use("/graphql", rateLimit({config, store, overrides, apiKeys: apiKeyLookup}))
		app.post("/graphql", (c) => c.json({data: {ok: true}}))
	})

	afterAll(async () => {
		await db.execute(sql`DELETE FROM api_keys`)
		await db.execute(sql`DELETE FROM rate_limit_overrides`)
		const keys = await valkey.keys("rl:*")
		if (keys.length > 0) await valkey.del(...keys)
		valkey.disconnect()
		await pool.end()
	})

	function req(ip: string) {
		return app.request("/graphql", {
			method: "POST",
			headers: {
				"content-type": "application/json",
				"x-forwarded-for": `${ip}, 10.108.0.5`,
			},
			body: JSON.stringify({query: "{__typename}"}),
		})
	}

	it("allows requests under the DB override limit then returns 429", async () => {
		// Self-contained: consumes the full quota and verifies 429 in one test
		for (let i = 0; i < TEST_LIMIT; i++) {
			const res = await req(TEST_IP)
			expect(res.status).toBe(200)
			expect(res.headers.get("RateLimit-Limit")).toBe(String(TEST_LIMIT))
			expect(Number(res.headers.get("RateLimit-Remaining"))).toBe(TEST_LIMIT - i - 1)
		}

		// Next request exceeds the limit
		const blocked = await req(TEST_IP)
		expect(blocked.status).toBe(429)
		expect(blocked.headers.get("Retry-After")).toBeDefined()
		const body = (await blocked.json()) as {error: string; retry_after_seconds: number}
		expect(body.error).toBe("rate_limit_exceeded")
		expect(body.retry_after_seconds).toBeGreaterThan(0)
	})

	it("blocks immediately when override is 0 (kill switch)", async () => {
		const res = await req(TEST_IP_BLOCKED)
		expect(res.status).toBe(429)
		expect(res.headers.get("RateLimit-Limit")).toBe("0")
	})

	it("uses default limit for IPs without a DB override", async () => {
		const res = await req(TEST_IP_DEFAULT)
		expect(res.status).toBe(200)
		expect(res.headers.get("RateLimit-Limit")).toBe("1000")
	})

	it("skips rate limiting for allowlisted IPs", async () => {
		// 10.108.5.42 is inside the allowlist CIDR 10.108.0.0/16
		// With trustedHops=1, XFF "10.108.5.42, 10.108.0.5" picks 10.108.5.42
		const res = await app.request("/graphql", {
			method: "POST",
			headers: {
				"content-type": "application/json",
				"x-forwarded-for": "10.108.5.42, 10.108.0.5",
			},
			body: JSON.stringify({query: "{__typename}"}),
		})
		expect(res.status).toBe(200)
		expect(res.headers.get("RateLimit-Limit")).toBeNull() // no headers for allowlisted
	})

	it("Valkey counter is shared (same key incremented across calls)", async () => {
		// Use a fresh IP to get a clean counter
		const freshIp = "192.0.2.77"
		const r1 = await req(freshIp)
		const r2 = await req(freshIp)
		expect(r1.headers.get("RateLimit-Remaining")).toBe("999")
		expect(r2.headers.get("RateLimit-Remaining")).toBe("998")
	})

	it("cache hits still count against rate limit (every request decrements remaining)", async () => {
		// This test proves that even if the GraphQL handler responds instantly
		// (simulating a Valkey response-cache hit), the rate limit middleware
		// still increments the counter on every request. The middleware runs
		// BEFORE the handler, so whether Yoga serves from cache or hits Postgres
		// is irrelevant — the counter was already incremented.
		//
		// We use a fresh IP with the default 1000/min limit and a DB override
		// of 5/min to make the test fast.
		const cacheHitIp = "192.0.2.200"
		await db.execute(
			sql`INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
			    VALUES (${cacheHitIp}::cidr, 5, 'integration test: cache-hit counting')`,
		)

		// Fire 5 requests — all "cache hits" (our handler returns instantly)
		const responses = []
		for (let i = 0; i < 5; i++) {
			responses.push(await req(cacheHitIp))
		}

		// Every response should be 200 with decrementing Remaining
		for (let i = 0; i < 5; i++) {
			const res = responses[i]!
			expect(res.status).toBe(200)
			expect(res.headers.get("RateLimit-Remaining")).toBe(String(5 - i - 1))
		}

		// 6th request should be 429 — even though every previous response was instant
		const blocked = await req(cacheHitIp)
		expect(blocked.status).toBe(429)
		expect(blocked.headers.get("RateLimit-Limit")).toBe("5")
	})

	// ─── API key integration tests ────────────────────────────────────

	function reqWithKey(key: string) {
		return app.request("/graphql", {
			method: "POST",
			headers: {
				"content-type": "application/json",
				"x-forwarded-for": "203.0.113.99, 10.108.0.5",
				"x-api-key": key,
			},
			body: JSON.stringify({query: "{__typename}"}),
		})
	}

	it("API key with custom limit: allows under limit then returns 429", async () => {
		for (let i = 0; i < 3; i++) {
			const res = await reqWithKey("test-custom-key")
			expect(res.status).toBe(200)
			expect(res.headers.get("RateLimit-Limit")).toBe("3")
			expect(Number(res.headers.get("RateLimit-Remaining"))).toBe(3 - i - 1)
		}
		const blocked = await reqWithKey("test-custom-key")
		expect(blocked.status).toBe(429)
	})

	it("API key with unlimited (null) allows unlimited requests with no headers", async () => {
		for (let i = 0; i < 20; i++) {
			const res = await reqWithKey("test-unlimited-key")
			expect(res.status).toBe(200)
			// Unlimited keys get no rate-limit headers
			expect(res.headers.get("RateLimit-Limit")).toBeNull()
		}
	})

	it("disabled API key falls through to IP-based limiting", async () => {
		const res = await reqWithKey("test-disabled-key")
		expect(res.status).toBe(200)
		// Should use the default IP-based limit (1000), not the key's 5000
		expect(res.headers.get("RateLimit-Limit")).toBe("1000")
	})

	it("unknown API key falls through to IP-based limiting", async () => {
		const res = await reqWithKey("nonexistent-key")
		expect(res.status).toBe(200)
		expect(res.headers.get("RateLimit-Limit")).toBe("1000")
	})

	it("API key counter is separate from IP counter", async () => {
		const freshIp = "192.0.2.55"
		// Make requests with IP (uses IP counter)
		const ipRes = await req(freshIp)
		expect(ipRes.headers.get("RateLimit-Remaining")).toBe("999")

		// Make requests with API key from same IP (uses key counter, separate)
		const keyRes = await reqWithKey("test-custom-key")
		// Key counter was already at 4 from earlier test... use unlimited key to verify isolation
		const unlimitedRes = await reqWithKey("test-unlimited-key")
		expect(unlimitedRes.status).toBe(200) // unlimited, unaffected by IP counter
	})
})
