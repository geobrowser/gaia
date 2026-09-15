import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"

// Mock the structured logger so we can assert on log levels.
vi.mock("../../services/telemetry", () => ({
	log: {
		debug: vi.fn(),
		info: vi.fn(),
		warn: vi.fn(),
		error: vi.fn(),
	},
}))

// Capture span status / attribute calls so we can assert them. The middleware
// calls trace.getTracer(...).startSpan(...) at request time.
const spanSetStatus = vi.fn()
const spanSetAttribute = vi.fn()
const spanEnd = vi.fn()
vi.mock("@opentelemetry/api", async () => {
	const actual = await vi.importActual<typeof import("@opentelemetry/api")>("@opentelemetry/api")
	return {
		...actual,
		trace: {
			...actual.trace,
			getTracer: () => ({
				startSpan: () => ({
					setStatus: spanSetStatus,
					setAttribute: spanSetAttribute,
					end: spanEnd,
					spanContext: () => ({traceId: "0".repeat(32), spanId: "0".repeat(16), traceFlags: 0}),
				}),
			}),
		},
	}
})

import {SpanStatusCode} from "@opentelemetry/api"
import {log} from "../../services/telemetry"
import {canonicalRequestLogging, isClientAbortError, requestId} from "../requestLogging"

/**
 * Mirror the production app wiring: requestId + canonicalRequestLogging,
 * plus an `app.onError` that converts AbortErrors to 499 (matching main.ts).
 * The default Hono error handler turns everything else into 500.
 */
function setupApp(handler: () => Promise<Response> | Response) {
	const app = new Hono()
	app.onError((err, c) => {
		if (isClientAbortError(err)) {
			return new Response(null, {status: 499})
		}
		return c.text("Internal Server Error", 500)
	})
	app.use("*", requestId())
	app.use("*", canonicalRequestLogging())
	app.all("/test", handler)
	return app
}

/**
 * Mirror the production AbortError shape: graphql-yoga / @whatwg-node/server
 * throw a `DOMException` (which extends Error in modern Node/Bun) with
 * `name === "AbortError"`, `code === 20`, empty stack. We can't construct a
 * DOMException directly in vitest's environment, so we forge an Error subclass
 * with the same surface — Hono's compose only routes `instanceof Error`
 * throws through `app.onError`.
 */
function makeAbortError(): Error {
	const err = new Error("The connection was closed.")
	Object.defineProperty(err, "name", {value: "AbortError"})
	Object.defineProperty(err, "code", {value: 20})
	err.stack = ""
	return err
}
const ABORT_ERROR_LIKE = {
	name: "AbortError",
	code: 20,
	message: "The connection was closed.",
}

describe("isClientAbortError", () => {
	it("recognizes DOMException-shape AbortError (code 20)", () => {
		expect(isClientAbortError(ABORT_ERROR_LIKE)).toBe(true)
	})

	it("recognizes Node-style AbortError (code 'ABORT_ERR')", () => {
		expect(isClientAbortError({name: "AbortError", code: "ABORT_ERR"})).toBe(true)
	})

	it("recognizes by name alone (defensive)", () => {
		expect(isClientAbortError({name: "AbortError"})).toBe(true)
	})

	it("does not match arbitrary errors", () => {
		expect(isClientAbortError(new Error("boom"))).toBe(false)
		expect(isClientAbortError({code: 20})).toBe(true) // code-only is enough — DOMException pattern
		expect(isClientAbortError({name: "TypeError"})).toBe(false)
		expect(isClientAbortError(null)).toBe(false)
		expect(isClientAbortError(undefined)).toBe(false)
		expect(isClientAbortError("string error")).toBe(false)
	})
})

describe("canonicalRequestLogging — client abort handling", () => {
	beforeEach(() => {
		vi.clearAllMocks()
	})

	it("client abort → 499 status, info-level log, no error/warn, no Sentry issue", async () => {
		const app = setupApp(() => {
			throw makeAbortError()
		})

		const res = await app.request("/test")

		expect(res.status).toBe(499)
		// 499 falls through to the default "completed" path at info level —
		// no warn/error means no Sentry issue and minimal log noise.
		expect(log.info).toHaveBeenCalledWith(
			"GET /test completed",
			expect.objectContaining({method: "GET", path: "/test", status: 499}),
		)
		expect(log.warn).not.toHaveBeenCalled()
		expect(log.error).not.toHaveBeenCalled()
	})

	it("Node-style AbortError also yields 499 with no error/warn", async () => {
		const app = setupApp(() => {
			throw Object.assign(new Error("aborted"), {name: "AbortError", code: "ABORT_ERR"})
		})

		const res = await app.request("/test")

		expect(res.status).toBe(499)
		expect(log.warn).not.toHaveBeenCalled()
		expect(log.error).not.toHaveBeenCalled()
	})

	it("non-abort errors still surface as 500 with log.error", async () => {
		const app = setupApp(() => {
			throw new Error("database exploded")
		})

		const res = await app.request("/test")

		expect(res.status).toBe(500)
		expect(log.error).toHaveBeenCalledWith(
			"GET /test returned 500",
			expect.objectContaining({method: "GET", path: "/test", status: 500}),
		)
	})

	it("successful requests log info as before", async () => {
		const app = setupApp(() => new Response("ok", {status: 200}))

		const res = await app.request("/test")

		expect(res.status).toBe(200)
		expect(log.info).toHaveBeenCalledWith("GET /test completed", expect.objectContaining({status: 200}))
		expect(log.warn).not.toHaveBeenCalled()
		expect(log.error).not.toHaveBeenCalled()
	})
})

describe("canonicalRequestLogging — span status", () => {
	beforeEach(() => {
		vi.clearAllMocks()
	})

	it("does NOT mark span as ERROR for 499 (client abort)", async () => {
		const app = setupApp(() => {
			throw makeAbortError()
		})
		await app.request("/test")

		// Critical: SentrySpanProcessor turns ERROR spans into failed
		// transactions in the Sentry Performance dashboard. Client aborts
		// must not pollute that signal.
		expect(spanSetStatus).not.toHaveBeenCalled()
		expect(spanSetAttribute).toHaveBeenCalledWith("http.status_code", 499)
	})

	it("DOES mark span as ERROR for genuine 5xx", async () => {
		const app = setupApp(() => {
			throw new Error("database exploded")
		})
		await app.request("/test")

		expect(spanSetStatus).toHaveBeenCalledWith({code: SpanStatusCode.ERROR, message: "HTTP 500"})
	})

	it("DOES mark span as ERROR for 4xx (non-499)", async () => {
		const app = setupApp(() => new Response("bad", {status: 400}))
		await app.request("/test")

		expect(spanSetStatus).toHaveBeenCalledWith({code: SpanStatusCode.ERROR, message: "HTTP 400"})
	})

	it("does NOT mark span as ERROR for 2xx", async () => {
		const app = setupApp(() => new Response("ok", {status: 200}))
		await app.request("/test")

		expect(spanSetStatus).not.toHaveBeenCalled()
	})
})

// Load shedding is not a fault. `shouldShedPoolTraffic` answers 503 with Retry-After
// when the database pool is saturated, which is the system protecting itself — it is
// already counted as `graphql.pool_shed` and logged once per episode.
//
// Reporting each one at ERROR routes to Sentry.captureMessage and creates an issue
// event per request. One shedding episode produced 210,732 of them in 30 days, 84% of
// the org's error volume, which exhausted the plan quota and left Sentry dropping
// everything with `error_usage_exceeded` from 2026-09-11. These assert the level,
// because the level is the whole fix.
describe("canonicalRequestLogging — load shedding vs genuine 5xx", () => {
	beforeEach(() => {
		vi.clearAllMocks()
	})

	it("503 WITH Retry-After is a shed: warn, never error", async () => {
		const app = setupApp(() => new Response("shed", {status: 503, headers: {"Retry-After": "2"}}))

		const res = await app.request("/test")

		expect(res.status).toBe(503)
		expect(log.warn).toHaveBeenCalledWith(
			"GET /test shed (503 + Retry-After)",
			expect.objectContaining({status: 503}),
		)
		// The message must not name a mechanism. Two different ones answer this way —
		// database pool shedding and admission control — and this branch cannot tell
		// them apart, because all it sees is the status and the header. Naming pool
		// pressure here sent a GEO-2881 investigation to look at Postgres while every
		// one of the 108 sheds in that window was admission control.
		expect(log.warn).not.toHaveBeenCalledWith(expect.stringContaining("pool pressure"), expect.anything())
		// The load-bearing assertion: error creates a Sentry issue, warn does not.
		expect(log.error).not.toHaveBeenCalled()
	})

	it("503 WITHOUT Retry-After is a genuine fault: still error", async () => {
		// What the health probes raise when the database is actually gone.
		const app = setupApp(() => new Response("db down", {status: 503}))

		const res = await app.request("/test")

		expect(res.status).toBe(503)
		expect(log.error).toHaveBeenCalledWith("GET /test returned 503", expect.objectContaining({status: 503}))
		expect(log.warn).not.toHaveBeenCalled()
	})

	it("500 is unaffected even if something set Retry-After", async () => {
		// Retry-After only downgrades a 503. A 500 is a fault whatever headers ride along.
		const app = setupApp(() => new Response("boom", {status: 500, headers: {"Retry-After": "2"}}))

		await app.request("/test")

		expect(log.error).toHaveBeenCalledWith("GET /test returned 500", expect.objectContaining({status: 500}))
		expect(log.warn).not.toHaveBeenCalled()
	})

	it("502 and 504 still error", async () => {
		for (const status of [502, 504]) {
			vi.clearAllMocks()
			const app = setupApp(() => new Response("upstream", {status}))
			await app.request("/test")
			expect(log.error).toHaveBeenCalledWith(`GET /test returned ${status}`, expect.objectContaining({status}))
		}
	})

	it("a shed still logs, so it stays visible as a breadcrumb", async () => {
		const app = setupApp(() => new Response("shed", {status: 503, headers: {"Retry-After": "2"}}))

		await app.request("/test")

		// Downgraded, not silenced — warn is a Sentry breadcrumb, so a shed still
		// shows up as context on whatever error follows it.
		expect(log.warn).toHaveBeenCalledTimes(1)
		expect(log.info).not.toHaveBeenCalledWith("GET /test completed", expect.anything())
	})
})
