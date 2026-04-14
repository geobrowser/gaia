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

		// Insert test overrides
		await db.execute(sql`DELETE FROM rate_limit_overrides`)
		await db.execute(
			sql`INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
			    VALUES (${TEST_IP}::cidr, ${TEST_LIMIT}, 'integration test: low limit')`,
		)
		await db.execute(
			sql`INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
			    VALUES (${TEST_IP_BLOCKED}::cidr, 0, 'integration test: blocked')`,
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

		app = new Hono()
		app.use("/graphql", rateLimit({config, store, overrides}))
		app.post("/graphql", (c) => c.json({data: {ok: true}}))
	})

	afterAll(async () => {
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

	it("allows requests under the DB override limit", async () => {
		for (let i = 0; i < TEST_LIMIT; i++) {
			const res = await req(TEST_IP)
			expect(res.status).toBe(200)
			expect(res.headers.get("RateLimit-Limit")).toBe(String(TEST_LIMIT))
			expect(Number(res.headers.get("RateLimit-Remaining"))).toBe(TEST_LIMIT - i - 1)
		}
	})

	it("returns 429 after exceeding the DB override limit", async () => {
		// The previous test already consumed TEST_LIMIT requests
		const res = await req(TEST_IP)
		expect(res.status).toBe(429)
		expect(res.headers.get("Retry-After")).toBeDefined()
		const body = (await res.json()) as {error: string; retry_after_seconds: number}
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
})
