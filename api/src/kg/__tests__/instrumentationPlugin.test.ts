import {GraphQLError} from "graphql"
import {describe, expect, it} from "vitest"
import {isClientError} from "../instrumentationPlugin"

describe("isClientError", () => {
	it("flags GraphQLError with BAD_USER_INPUT extension code", () => {
		const err = new GraphQLError("too big", {extensions: {code: "BAD_USER_INPUT"}})
		expect(isClientError(err)).toBe(true)
	})

	it("flags GraphQLError with GRAPHQL_PARSE_FAILED extension code", () => {
		const err = new GraphQLError("syntax bad", {extensions: {code: "GRAPHQL_PARSE_FAILED"}})
		expect(isClientError(err)).toBe(true)
	})

	it("flags GraphQLError with GRAPHQL_VALIDATION_FAILED extension code", () => {
		const err = new GraphQLError("invalid", {extensions: {code: "GRAPHQL_VALIDATION_FAILED"}})
		expect(isClientError(err)).toBe(true)
	})

	it("flags wrapped error whose originalError has BAD_USER_INPUT code", () => {
		const original = new GraphQLError("too big", {extensions: {code: "BAD_USER_INPUT"}})
		const wrapper = new GraphQLError("wrapped", {originalError: original})
		expect(isClientError(wrapper)).toBe(true)
	})

	it("flags the PostGraphile first+last error by message", () => {
		const original = new Error("We don't support setting both first and last")
		const wrapper = new GraphQLError("Unexpected error.", {originalError: original})
		expect(isClientError(wrapper)).toBe(true)
	})

	it("flags missing required variable error from graphql-js", () => {
		const err = new GraphQLError('Variable "$id" of required type "UUID!" was not provided.')
		expect(isClientError(err)).toBe(true)
	})

	it("flags non-null variable violation from graphql-js", () => {
		const err = new GraphQLError('Variable "$id" of non-null type "UUID!" must not be null.')
		expect(isClientError(err)).toBe(true)
	})

	it("flags invalid variable value from graphql-js", () => {
		const err = new GraphQLError('Variable "$spaceId" got invalid value "abc"; Expected type "UUID".')
		expect(isClientError(err)).toBe(true)
	})

	it("does not flag a server error", () => {
		const err = new GraphQLError("db exploded", {extensions: {code: "INTERNAL_SERVER_ERROR"}})
		expect(isClientError(err)).toBe(false)
	})

	it("does not flag a plain Error without client-error markers", () => {
		expect(isClientError(new Error("pool_pressure_shed"))).toBe(false)
	})

	it("does not flag null or undefined", () => {
		expect(isClientError(null)).toBe(false)
		expect(isClientError(undefined)).toBe(false)
	})
})
