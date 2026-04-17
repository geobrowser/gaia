import {GraphQLError} from "graphql"
import {describe, expect, it} from "vitest"
import {isClientError, isExpectedError} from "../instrumentationPlugin"

describe("isExpectedError", () => {
	it("flags GraphQLError with BAD_USER_INPUT extension code", () => {
		const err = new GraphQLError("too big", {extensions: {code: "BAD_USER_INPUT"}})
		expect(isExpectedError(err)).toBe(true)
	})

	it("flags GraphQLError with GRAPHQL_PARSE_FAILED extension code", () => {
		const err = new GraphQLError("syntax bad", {extensions: {code: "GRAPHQL_PARSE_FAILED"}})
		expect(isExpectedError(err)).toBe(true)
	})

	it("flags GraphQLError with GRAPHQL_VALIDATION_FAILED extension code", () => {
		const err = new GraphQLError("invalid", {extensions: {code: "GRAPHQL_VALIDATION_FAILED"}})
		expect(isExpectedError(err)).toBe(true)
	})

	it("flags GraphQLError with SERVICE_UNAVAILABLE extension code (pool shed)", () => {
		const err = new GraphQLError("retry later", {extensions: {code: "SERVICE_UNAVAILABLE"}})
		expect(isExpectedError(err)).toBe(true)
	})

	it("flags wrapped error whose originalError has BAD_USER_INPUT code", () => {
		const original = new GraphQLError("too big", {extensions: {code: "BAD_USER_INPUT"}})
		const wrapper = new GraphQLError("wrapped", {originalError: original})
		expect(isExpectedError(wrapper)).toBe(true)
	})

	it("flags wrapped error whose originalError has SERVICE_UNAVAILABLE code", () => {
		const original = new GraphQLError("retry", {extensions: {code: "SERVICE_UNAVAILABLE"}})
		const wrapper = new GraphQLError("wrapped", {originalError: original})
		expect(isExpectedError(wrapper)).toBe(true)
	})

	it("flags the PostGraphile first+last error by message", () => {
		const original = new Error("We don't support setting both first and last")
		const wrapper = new GraphQLError("Unexpected error.", {originalError: original})
		expect(isExpectedError(wrapper)).toBe(true)
	})

	it("does not flag a server error", () => {
		const err = new GraphQLError("db exploded", {extensions: {code: "INTERNAL_SERVER_ERROR"}})
		expect(isExpectedError(err)).toBe(false)
	})

	it("does not flag a plain Error without expected-error markers", () => {
		expect(isExpectedError(new Error("pool_pressure_shed"))).toBe(false)
	})

	it("does not flag null or undefined", () => {
		expect(isExpectedError(null)).toBe(false)
		expect(isExpectedError(undefined)).toBe(false)
	})

	it("isClientError is a back-compat alias for isExpectedError", () => {
		expect(isClientError).toBe(isExpectedError)
	})
})
