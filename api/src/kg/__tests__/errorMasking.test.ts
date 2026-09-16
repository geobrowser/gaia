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
	it("unmasks a code nested several wrappers deep", () => {
		// graphql-js wraps once per level it propagates through, so an error
		// thrown while building a NESTED field's SQL arrives deeper than the one
		// level the old implementation unwrapped. This is the pagination-cap
		// error on a nested collection.
		const original = new GraphQLError('Pagination argument "first" cannot exceed 1000; received 1001', {
			extensions: {code: "BAD_USER_INPUT"},
		})
		let wrapped: GraphQLError = original
		for (let i = 0; i < 4; i++) {
			wrapped = new GraphQLError("wrapped", {originalError: wrapped})
		}
		expect(shouldUnmaskError(wrapped)).toBe(true)
	})

	it("unmasks an error from a different copy of the graphql package", () => {
		// The api's tree carries several copies of `graphql` (its own, plus ones
		// nested under postgraphile and graphile-utils), and a class from one
		// copy fails `instanceof` against another's. An error that crosses that
		// boundary must still be judged on its `extensions.code`, which is the
		// actual contract — otherwise it is masked to "Unexpected error." and
		// the caller loses the message telling them what to fix.
		class ForeignGraphQLError extends Error {
			extensions: Record<string, unknown>
			originalError?: Error
			constructor(message: string, extensions: Record<string, unknown>) {
				super(message)
				this.extensions = extensions
			}
		}
		const foreign = new ForeignGraphQLError("bad input", {code: "BAD_USER_INPUT"})
		expect(foreign instanceof GraphQLError).toBe(false)
		expect(shouldUnmaskError(foreign)).toBe(true)
	})

	it("unmasks a foreign error wrapped by a native one", () => {
		class ForeignGraphQLError extends Error {
			extensions: Record<string, unknown> = {code: "BAD_USER_INPUT"}
		}
		const wrapper = new GraphQLError("wrapped", {originalError: new ForeignGraphQLError("bad")})
		expect(shouldUnmaskError(wrapper)).toBe(true)
	})

	it("terminates on a cyclic originalError chain", () => {
		// The chain's length follows caller-controlled query nesting, so the walk
		// is bounded; a cycle must not hang error handling.
		const a = new Error("a") as Error & {originalError?: Error}
		const b = new Error("b") as Error & {originalError?: Error}
		a.originalError = b
		b.originalError = a
		expect(shouldUnmaskError(a)).toBe(false)
	})

	it("still does not unmask a deep chain with no unmaskable code", () => {
		let wrapped: GraphQLError = new GraphQLError("root", {extensions: {code: "INTERNAL_SERVER_ERROR"}})
		for (let i = 0; i < 6; i++) {
			wrapped = new GraphQLError("wrapped", {originalError: wrapped})
		}
		expect(shouldUnmaskError(wrapped)).toBe(false)
	})
})
