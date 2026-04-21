import {type ASTNode, GraphQLError, Kind} from "graphql"
import {describe, expect, it} from "vitest"
import {isClientError} from "../instrumentationPlugin"

// Minimal AST-node factory — isClientError only reads `kind`, so the rest of
// the shape doesn't matter. Cast through `unknown` to avoid TS insisting on a
// complete node object.
function node(kind: (typeof Kind)[keyof typeof Kind]): ASTNode {
	return {kind} as unknown as ASTNode
}

describe("isClientError", () => {
	// ------------------------------------------------------------------
	// Structured extension codes
	// ------------------------------------------------------------------

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

	// ------------------------------------------------------------------
	// AST-node-based detection (variable / validation / coercion errors)
	// ------------------------------------------------------------------

	it("flags missing required variable via VariableDefinition node", () => {
		const err = new GraphQLError('Variable "$id" of required type "UUID!" was not provided.', {
			nodes: [node(Kind.VARIABLE_DEFINITION)],
		})
		expect(isClientError(err)).toBe(true)
	})

	it("flags non-null variable violation via VariableDefinition node", () => {
		const err = new GraphQLError('Variable "$id" of non-null type "UUID!" must not be null.', {
			nodes: [node(Kind.VARIABLE_DEFINITION)],
		})
		expect(isClientError(err)).toBe(true)
	})

	it("flags invalid variable value via VariableDefinition node", () => {
		const err = new GraphQLError('Variable "$spaceId" got invalid value "abc"; Expected type "UUID".', {
			nodes: [node(Kind.VARIABLE_DEFINITION)],
		})
		expect(isClientError(err)).toBe(true)
	})

	it("flags validation error pointing at a Field node (unknown field)", () => {
		const err = new GraphQLError('Cannot query field "foo" on type "Query".', {
			nodes: [node(Kind.FIELD)],
		})
		expect(isClientError(err)).toBe(true)
	})

	it("flags validation error pointing at an Argument node", () => {
		const err = new GraphQLError('Unknown argument "foo" on field "Query.bar".', {
			nodes: [node(Kind.ARGUMENT)],
		})
		expect(isClientError(err)).toBe(true)
	})

	it("flags input-coercion error pointing at an ObjectField node", () => {
		const err = new GraphQLError('Field "foo" is not defined by type "BarInput".', {
			nodes: [node(Kind.OBJECT_FIELD)],
		})
		expect(isClientError(err)).toBe(true)
	})

	it("flags directive error pointing at a Directive node", () => {
		const err = new GraphQLError('Unknown directive "@foo".', {
			nodes: [node(Kind.DIRECTIVE)],
		})
		expect(isClientError(err)).toBe(true)
	})

	// ------------------------------------------------------------------
	// Non-client errors (must not be flagged)
	// ------------------------------------------------------------------

	it("does not flag a GraphQLError with INTERNAL_SERVER_ERROR code", () => {
		const err = new GraphQLError("db exploded", {extensions: {code: "INTERNAL_SERVER_ERROR"}})
		expect(isClientError(err)).toBe(false)
	})

	it("does not flag a plain Error without client-error markers", () => {
		expect(isClientError(new Error("pool_pressure_shed"))).toBe(false)
	})

	it("does not flag a resolver throw wrapped in a GraphQLError", () => {
		// graphql-js wraps resolver exceptions into a GraphQLError with the
		// Field node attached, but the underlying originalError is a plain
		// Error — that's our cue that this was thrown from a resolver, not a
		// client-caused structural error.
		const original = new Error("pool_pressure_shed")
		const wrapper = new GraphQLError("pool_pressure_shed", {
			nodes: [node(Kind.FIELD)],
			originalError: original,
		})
		expect(isClientError(wrapper)).toBe(false)
	})

	it("does not flag a resolver-thrown GraphQLError that has an execution path", () => {
		// If a resolver does `throw new GraphQLError("db timed out")` directly,
		// there is no code on extensions and no plain-Error originalError to
		// signal a resolver origin. The discriminator is `path`: graphql-js
		// only attaches it during execution, so parse/validate/coerce errors
		// lack it while any resolver-surfaced error has it.
		const err = new GraphQLError("db timed out", {
			nodes: [node(Kind.FIELD)],
			path: ["entities", 0, "name"],
		})
		expect(isClientError(err)).toBe(false)
	})

	it("does not flag a GraphQLError whose nodes are only type-system definitions", () => {
		// Contrived — wouldn't normally appear at request time — but proves
		// schema-build errors are excluded from the client-error classification.
		const err = new GraphQLError("schema problem", {nodes: [node(Kind.OBJECT_TYPE_DEFINITION)]})
		expect(isClientError(err)).toBe(false)
	})

	it("does not flag null or undefined", () => {
		expect(isClientError(null)).toBe(false)
		expect(isClientError(undefined)).toBe(false)
	})
})
