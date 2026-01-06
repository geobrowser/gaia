import { GraphQLScalarType } from "graphql"
import { Kind } from "graphql/language"
import { describe, expect, it } from "vitest"
import { patchUuidScalar } from "../uuidScalarPlugin"

describe("UndashedUuidPlugin scalar patch", () => {
	it("accepts dashed and undashed UUID inputs and normalizes to undashed lowercase", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(scalar.parseValue("550E8400-E29B-41D4-A716-446655440000")).toBe(
			"550e8400e29b41d4a716446655440000",
		)
		expect(scalar.parseValue("550e8400e29b41d4a716446655440000")).toBe(
			"550e8400e29b41d4a716446655440000",
		)
	})

	it("serializes UUID outputs without dashes (undashed lowercase)", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(scalar.serialize("550e8400-e29b-41d4-a716-446655440000")).toBe(
			"550e8400e29b41d4a716446655440000",
		)
	})

	it("rejects non-string input values", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(() => scalar.parseValue(123 as any)).toThrow(/non-string/)
	})

	it("rejects non-string literals", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(() => scalar.parseLiteral({ kind: Kind.INT, value: "123" } as any)).toThrow(
			/parse string/i,
		)
	})
})


