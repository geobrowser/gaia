import {describe, expect, it} from "vitest"

import {classifyDbFailure, detectDbFailureClass, isRetryableDbFailure, isRetryableDbFailureClass} from "./dbFailures"

describe("dbFailures", () => {
	it("detects pool connect timeout", () => {
		expect(detectDbFailureClass(new Error("timeout exceeded when trying to connect"))).toBe("pool_connect_timeout")
	})

	it("detects connection reset by code", () => {
		expect(classifyDbFailure({code: "ECONNRESET", message: "socket hang up"})).toBe("connection_reset")
	})

	it("detects too many connection errors", () => {
		expect(classifyDbFailure(new Error("sorry, too many clients already"))).toBe("too_many_connections")
	})

	it("marks retryable classes correctly", () => {
		expect(isRetryableDbFailureClass("pool_connect_timeout")).toBe(true)
		expect(isRetryableDbFailureClass("statement_timeout")).toBe(false)
		expect(isRetryableDbFailureClass("unknown_db_failure")).toBe(false)
	})

	it("marks retryable errors correctly", () => {
		expect(isRetryableDbFailure(new Error("timeout exceeded when trying to connect"))).toBe(true)
		expect(isRetryableDbFailure(new Error("canceling statement due to statement timeout"))).toBe(false)
	})
})
