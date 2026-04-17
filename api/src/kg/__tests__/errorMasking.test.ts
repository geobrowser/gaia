import {GraphQLError} from "graphql"
import {describe, expect, it} from "vitest"
import {shouldUnmaskError} from "../errorMasking"

describe("shouldUnmaskError", () => {
	it("unmasks BAD_USER_INPUT errors", () => {
		const err = new GraphQLError("too big", {extensions: {code: "BAD_USER_INPUT"}})
		expect(shouldUnmaskError(err)).toBe(true)
	})

	it("unmasks SERVICE_UNAVAILABLE errors", () => {
		const err = new GraphQLError("retry later", {extensions: {code: "SERVICE_UNAVAILABLE"}})
		expect(shouldUnmaskError(err)).toBe(true)
	})

	it("unmasks wrapped BAD_USER_INPUT via originalError", () => {
		const original = new GraphQLError("too big", {extensions: {code: "BAD_USER_INPUT"}})
		const wrapper = new GraphQLError("wrapped", {originalError: original})
		expect(shouldUnmaskError(wrapper)).toBe(true)
	})

	it("unmasks wrapped SERVICE_UNAVAILABLE via originalError", () => {
		const original = new GraphQLError("retry", {extensions: {code: "SERVICE_UNAVAILABLE"}})
		const wrapper = new GraphQLError("wrapped", {originalError: original})
		expect(shouldUnmaskError(wrapper)).toBe(true)
	})

	it("does not unmask INTERNAL_SERVER_ERROR", () => {
		const err = new GraphQLError("db exploded", {extensions: {code: "INTERNAL_SERVER_ERROR"}})
		expect(shouldUnmaskError(err)).toBe(false)
	})

	it("does not unmask GraphQLError with no code", () => {
		expect(shouldUnmaskError(new GraphQLError("untagged"))).toBe(false)
	})

	it("does not unmask plain Error instances", () => {
		expect(shouldUnmaskError(new Error("boom"))).toBe(false)
	})

	it("does not unmask non-Error values", () => {
		expect(shouldUnmaskError(null)).toBe(false)
		expect(shouldUnmaskError(undefined)).toBe(false)
		expect(shouldUnmaskError("string")).toBe(false)
	})
})
