import {parse} from "graphql"
import {describe, expect, it} from "vitest"
import {computeQueryCost} from "../costLoggerPlugin"
import {MAX_PAGINATION_LIMIT} from "../paginationCapPlugin"

const cost = (query: string, variables: Record<string, unknown> = {}) => computeQueryCost(parse(query), variables)

describe("computeQueryCost — basic shapes", () => {
	it("scalar-only operation → 1", () => {
		expect(cost(`{ __typename }`)).toBe(1)
	})

	it("object field with no pagination args → sum of children + 1", () => {
		// Single-entity lookup (no first/last): not multiplied.
		// entity { id name } → (1 + 1) + 1 = 3
		expect(cost(`{ entity(id: "...") { id name } }`)).toBe(3)
	})
})

describe("computeQueryCost — pagination", () => {
	it("first:50 selecting id → 51", () => {
		expect(cost(`{ entities(first: 50) { id } }`)).toBe(51)
	})

	it("first:100 selecting id + name → 201", () => {
		expect(cost(`{ entities(first: 100) { id name } }`)).toBe(201)
	})

	it("last on root collection honored", () => {
		expect(cost(`{ entities(last: 25) { id } }`)).toBe(26)
	})

	it("nested list multiplies correctly", () => {
		// inner relations(first:10){id} = 11
		// outer entities(first:20){id + inner} = 20 × (1 + 11) + 1 = 241
		expect(
			cost(`{
				entities(first: 20) {
					id
					relations(first: 10) { id }
				}
			}`),
		).toBe(241)
	})

	it("models the slow-query pattern that triggered prod statement_timeout", () => {
		// 1000-entity outer × 5 relation sub-collections × 50 each × 2 nested entity fields
		// matches the real shape we saw timing out.
		// inner per-sub-collection: 50 × (1 + (1 + 1)) + 1 ... we'll just compute raw:
		// { id, entity { valuesList(first:1){propertyId} } } = 1 + (1*1+1 + 1) = 4
		// sub-collection(first:50){...} = 50 × 4 + 1 = 201
		// 5 of those + id = 5 × 201 + 1 = 1006 per outer entity
		// entities(first:1000){...} = 1000 × 1006 + 1 = 1_006_001
		const query = `
			{
				entities(first: 1000) {
					id
					a: relations(first: 50) { id entity { valuesList(first: 1) { propertyId } } }
					b: relations(first: 50) { id entity { valuesList(first: 1) { propertyId } } }
					c: relations(first: 50) { id entity { valuesList(first: 1) { propertyId } } }
					d: relations(first: 50) { id entity { valuesList(first: 1) { propertyId } } }
					e: relations(first: 50) { id entity { valuesList(first: 1) { propertyId } } }
				}
			}
		`
		const result = cost(query)
		expect(result).toBe(1_006_001)
		expect(result).toBeGreaterThan(100_000) // would trivially exceed any sane threshold
	})
})

describe("computeQueryCost — defensive handling of bogus inputs", () => {
	it("first:0 uses MAX_PAGINATION_LIMIT (defensive)", () => {
		// Otherwise an attacker could hide expensive nested work behind first:0
		// (which at SQL time returns nothing, but is atypical in legit traffic).
		expect(cost(`{ entities(first: 0) { id } }`)).toBe(MAX_PAGINATION_LIMIT * 1 + 1)
	})

	it("negative first uses MAX_PAGINATION_LIMIT", () => {
		expect(cost(`{ entities(first: -5) { id } }`)).toBe(MAX_PAGINATION_LIMIT + 1)
	})

	it("negative last uses MAX_PAGINATION_LIMIT", () => {
		expect(cost(`{ entities(last: -100) { id } }`)).toBe(MAX_PAGINATION_LIMIT + 1)
	})

	it("negative outer first still charges for valid nested first", () => {
		// Attack shape: user tries to hide expensive inner behind first:-5 outer.
		// Inner relations(first:50){id} = 51.
		// Outer entities(first:-5) → MAX × (1 + 51) + 1 = 52_001
		expect(
			cost(`{
				entities(first: -5) {
					id
					relations(first: 50) { id }
				}
			}`),
		).toBe(MAX_PAGINATION_LIMIT * 52 + 1)
	})
})

describe("computeQueryCost — variables", () => {
	it("resolves pagination args supplied via variables", () => {
		expect(cost(`query Q($first: Int!) { entities(first: $first) { id } }`, {first: 100})).toBe(101)
	})

	it("treats missing variable as no valid pagination → MAX", () => {
		expect(
			cost(
				`query Q($first: Int) { entities(first: $first) { id } }`,
				{}, // $first unresolved
			),
		).toBe(MAX_PAGINATION_LIMIT + 1)
	})
})

describe("computeQueryCost — fragments", () => {
	it("inline fragments contribute to parent's childComplexity", () => {
		// entities(first:10) { ... on Entity { id name } }
		// inline frag selections = 2 → outer = 10 × 2 + 1 = 21
		expect(
			cost(`{
				entities(first: 10) {
					... on Entity { id name }
				}
			}`),
		).toBe(21)
	})

	it("fragment spreads follow the definition", () => {
		// fragment F on Entity { id name }
		// entities(first:10) { ...F } → 10 × 2 + 1 = 21
		expect(
			cost(`
				query { entities(first: 10) { ...F } }
				fragment F on Entity { id name }
			`),
		).toBe(21)
	})
})

describe("computeQueryCost — empty / edge", () => {
	it("returns 0 for a document with no operation", () => {
		expect(cost(`fragment Unused on Query { __typename }`)).toBe(0)
	})
})
