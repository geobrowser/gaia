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

	it("client abort → 499 status, log.warn (not error), no Sentry issue", async () => {
		const app = setupApp(() => {
			throw makeAbortError()
		})

		const res = await app.request("/test")

		expect(res.status).toBe(499)
		// log.warn fires from the success-branch 499 case, never log.error
		// (which is what would create a Sentry issue).
		expect(log.warn).toHaveBeenCalledWith(
			"GET /test aborted by client",
			expect.objectContaining({method: "GET", path: "/test", status: 499}),
		)
		expect(log.error).not.toHaveBeenCalled()
	})

	it("Node-style AbortError also yields 499 + warn", async () => {
		const app = setupApp(() => {
			throw Object.assign(new Error("aborted"), {name: "AbortError", code: "ABORT_ERR"})
		})

		const res = await app.request("/test")

		expect(res.status).toBe(499)
		expect(log.warn).toHaveBeenCalledWith("GET /test aborted by client", expect.any(Object))
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
