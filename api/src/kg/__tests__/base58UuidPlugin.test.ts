import {GraphQLScalarType} from "graphql"
import {Kind} from "graphql/language"
import {describe, expect, it} from "vitest"
import {uuidToBase58} from "../../utils/uuid"
import {patchUuidScalar} from "../base58UuidPlugin"

describe("Base58UuidPlugin scalar patch", () => {
	it("accepts Base58 input and normalizes to dashed lowercase UUID", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		const base58 = uuidToBase58("550e8400-e29b-41d4-a716-446655440000")
		expect(scalar.parseValue(base58)).toBe("550e8400-e29b-41d4-a716-446655440000")
	})

	it("rejects dashed hex UUID via parseValue", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(() => scalar.parseValue("550e8400-e29b-41d4-a716-446655440000")).toThrow(/Invalid Base58/i)
	})

	it("rejects dashless hex UUID via parseValue", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(() => scalar.parseValue("550e8400e29b41d4a716446655440000")).toThrow(/Invalid Base58/i)
	})

	it("serializes UUID outputs as Base58", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		// serialize should produce the exact Base58 encoding
		const result = scalar.serialize("550e8400-e29b-41d4-a716-446655440000")
		expect(result).toBe("BWBeN28Vb7cMEx7Ym8AUzs")
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

		expect(() => scalar.parseLiteral({kind: Kind.INT, value: "123"} as any)).toThrow(/parse string/i)
	})

	it("accepts Base58 literals and normalizes to dashed lowercase UUID", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		const base58 = uuidToBase58("550e8400-e29b-41d4-a716-446655440000")
		expect(scalar.parseLiteral({kind: Kind.STRING, value: base58})).toBe("550e8400-e29b-41d4-a716-446655440000")
	})

	it("rejects dashed hex UUID via parseLiteral", () => {
		const scalar = new GraphQLScalarType({
			name: "UUID",
			serialize: (v) => String(v),
			parseValue: (v) => String(v),
			parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
		})

		patchUuidScalar(scalar)

		expect(() => scalar.parseLiteral({kind: Kind.STRING, value: "550e8400-e29b-41d4-a716-446655440000"})).toThrow(
			/Invalid Base58/i,
		)
	})

	describe("invalid input validation", () => {
		it("rejects partially dashed UUIDs via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseValue("550e8400-e29b41d4a716446655440000")).toThrow(/Invalid Base58/i)
		})

		it("rejects partially dashed UUIDs via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseLiteral({kind: Kind.STRING, value: "550e8400-e29b41d4a716446655440000"})).toThrow(
				/Invalid Base58/i,
			)
		})

		it("rejects empty strings via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseValue("")).toThrow(/Invalid Base58/i)
		})

		it("rejects empty strings via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseLiteral({kind: Kind.STRING, value: ""})).toThrow(/Invalid Base58/i)
		})

		it("rejects hex strings with incorrect character counts via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			// Dashless hex (31 characters) — not valid Base58 due to '0'
			expect(() => scalar.parseValue("550e8400e29b41d4a71644665544000")).toThrow(/Invalid Base58/i)
			// Dashless hex (33 characters) — not valid Base58 due to '0'
			expect(() => scalar.parseValue("550e8400e29b41d4a7164466554400000")).toThrow(/Invalid Base58/i)
			// Dashed hex (wrong format) — not valid Base58 due to '-'
			expect(() => scalar.parseValue("550e8400-e29b-41d4-a716-44665544000")).toThrow(/Invalid Base58/i)
		})

		it("rejects hex strings with incorrect character counts via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.parseLiteral({kind: Kind.STRING, value: "550e8400e29b41d4a71644665544000"})).toThrow(
				/Invalid Base58/i,
			)
		})

		it("rejects strings with invalid characters via parseValue", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			// Invalid character 'g' in undashed format — not valid Base58 due to '0'
			expect(() => scalar.parseValue("550e8400e29b41d4a71644665544000g")).toThrow(/Invalid Base58/i)
			// Invalid character 'g' in dashed format — not valid Base58 due to '-'
			expect(() => scalar.parseValue("550e8400-e29b-41d4-a716-44665544000g")).toThrow(/Invalid Base58/i)
			// Invalid character 'x' in undashed format — not valid Base58 due to '0'
			expect(() => scalar.parseValue("550e8400e29b41d4a71644665544000x")).toThrow(/Invalid Base58/i)
		})

		it("rejects strings with invalid characters via parseLiteral", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() =>
				scalar.parseLiteral({kind: Kind.STRING, value: "550e8400-e29b-41d4-a716-44665544000g"}),
			).toThrow(/Invalid Base58/i)
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
			expect(() => scalar.serialize("550e8400-e29b-41d4-a716-44665544000g")).toThrow(/Invalid UUID/i)
		})

		it("rejects non-string values via serialize", () => {
			const scalar = new GraphQLScalarType({
				name: "UUID",
				serialize: (v) => String(v),
				parseValue: (v) => String(v),
				parseLiteral: (ast) => (ast.kind === Kind.STRING ? ast.value : null),
			})

			patchUuidScalar(scalar)

			expect(() => scalar.serialize(123 as any)).toThrow(/non-string/)
			expect(() => scalar.serialize(null as any)).toThrow(/non-string/)
			expect(() => scalar.serialize(undefined as any)).toThrow(/non-string/)
			expect(() => scalar.serialize({} as any)).toThrow(/non-string/)
		})
	})
})
