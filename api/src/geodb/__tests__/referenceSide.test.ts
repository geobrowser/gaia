import {describe, expect, it} from "vitest"
import {GeoDbError} from "../client"
import {failureResult, ReferenceSideError, sqlStateOf, viaReference} from "../referenceSide"

describe("failureResult", () => {
	it("is failed for anything that did not come from the Postgres side", () => {
		expect(failureResult(new GeoDbError("timeout", "slow"))).toBe("failed")
		expect(failureResult(Object.assign(new Error("x"), {code: "57014"}))).toBe("failed")
	})
	it("matches the statement-timeout SQLSTATE, not the message", async () => {
		const timeout = Object.assign(new Error("something else entirely"), {code: "57014"})
		await expect(viaReference(Promise.reject(timeout))).rejects.toBeInstanceOf(ReferenceSideError)
		expect(failureResult(new ReferenceSideError(timeout))).toBe("reference_timeout")
		expect(failureResult(new ReferenceSideError(new Error("canceling statement due to statement timeout")))).toBe(
			"reference_failed",
		)
	})
	it("finds the code through a cause chain", () => {
		expect(sqlStateOf(new Error("w", {cause: Object.assign(new Error("i"), {code: "57014"})}))).toBe("57014")
		expect(sqlStateOf(Object.assign(new Error("x"), {code: "ECONNRESET"}))).toBeUndefined()
	})
})
