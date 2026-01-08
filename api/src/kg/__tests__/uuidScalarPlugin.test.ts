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

	it("accepts dashed and undashed UUID literals and normalizes to undashed lowercase", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(
			scalar.parseLiteral({ kind: Kind.STRING, value: "550E8400-E29B-41D4-A716-446655440000" }),
		).toBe("550e8400e29b41d4a716446655440000")
		expect(
			scalar.parseLiteral({ kind: Kind.STRING, value: "550e8400e29b41d4a716446655440000" }),
		).toBe("550e8400e29b41d4a716446655440000")
	})

	describe("invalid UUID input validation", () => {
		it("rejects partially dashed UUIDs via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseValue("550e8400-e29b41d4a716446655440000")).toThrow(
				/Invalid UUID/i,
			)
		})

		it("rejects partially dashed UUIDs via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() =>
				scalar.parseLiteral({ kind: Kind.STRING, value: "550e8400-e29b41d4a716446655440000" }),
			).toThrow(/Invalid UUID/i)
		})

		it("rejects empty strings via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseValue("")).toThrow(/Invalid UUID/i)
		})

		it("rejects empty strings via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseLiteral({ kind: Kind.STRING, value: "" })).toThrow(
				/Invalid UUID/i,
			)
		})

		it("rejects UUIDs with incorrect character counts via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			// Too short (31 characters)
			expect(() => scalar.parseValue("550e8400e29b41d4a71644665544000")).toThrow(
				/Invalid UUID/i,
			)
			// Too long (33 characters)
			expect(() => scalar.parseValue("550e8400e29b41d4a7164466554400000")).toThrow(
				/Invalid UUID/i,
			)
			// Dashed but wrong format (too short)
			expect(() => scalar.parseValue("550e8400-e29b-41d4-a716-44665544000")).toThrow(
				/Invalid UUID/i,
			)
		})

		it("rejects UUIDs with incorrect character counts via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() =>
				scalar.parseLiteral({ kind: Kind.STRING, value: "550e8400e29b41d4a71644665544000" }),
			).toThrow(/Invalid UUID/i)
		})

		it("rejects UUIDs with invalid characters via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			// Invalid character 'g' in undashed format
			expect(() => scalar.parseValue("550e8400e29b41d4a71644665544000g")).toThrow(
				/Invalid UUID/i,
			)
			// Invalid character 'g' in dashed format
			expect(() => scalar.parseValue("550e8400-e29b-41d4-a716-44665544000g")).toThrow(
				/Invalid UUID/i,
			)
			// Invalid character 'x' in undashed format
			expect(() => scalar.parseValue("550e8400e29b41d4a71644665544000x")).toThrow(
				/Invalid UUID/i,
			)
		})

		it("rejects UUIDs with invalid characters via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() =>
				scalar.parseLiteral({ kind: Kind.STRING, value: "550e8400-e29b-41d4-a716-44665544000g" }),
			).toThrow(/Invalid UUID/i)
		})

		it("rejects invalid UUIDs via serialize", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.serialize("invalid-uuid")).toThrow(/Invalid UUID/i)
			expect(() => scalar.serialize("")).toThrow(/Invalid UUID/i)
			expect(() => scalar.serialize("550e8400-e29b-41d4-a716-44665544000g")).toThrow(
				/Invalid UUID/i,
			)
		})
	})
})


