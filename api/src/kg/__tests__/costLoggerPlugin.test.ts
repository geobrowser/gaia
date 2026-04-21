import {parse} from "graphql"
import {beforeEach, describe, expect, it} from "vitest"
import {
	__resetQueryCostHistogramForTests,
	computeQueryCost,
	renderQueryCostHistogram,
	useCostLogger,
} from "../costLoggerPlugin"
import {MAX_PAGINATION_LIMIT} from "../paginationCapPlugin"

const cost = (query: string, variables: Record<string, unknown> = {}) => computeQueryCost(parse(query), variables)

describe("computeQueryCost — basic shapes", () => {
	it("scalar-only operation → 1", () => {
		expect(cost(`{ __typename }`)).toBe(1)
	})

	it("structured field without first/last defaults to MAX (conservative)", () => {
		// Single-entity lookups get over-counted — intentionally. We never know
		// from the AST alone whether a field is a list or a single object; the
		// SQL layer injects first=MAX on every collection field anyway, so
		// over-counting single-entity lookups is the price of never missing an
		// unbounded collection.
		// entity { id name } → MAX × (1 + 1) + 1 = 2001
		expect(cost(`{ entity(id: "...") { id name } }`)).toBe(MAX_PAGINATION_LIMIT * 2 + 1)
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
		// The real shape we saw timing out, computed with the conservative model:
		//   valuesList(first:1){propertyId}        = 1 × 1 + 1                  = 2
		//   entity { valuesList }                  = MAX × 2 + 1                = 2001
		//   relations(first:50) { id, entity }     = 50 × (1 + 2001) + 1        = 100_101
		//   5 × 100_101 + 1 (id on entity)         = 500_506
		//   entities(first:1000) { ... }           = 1000 × 500_506 + 1         = 500_506_001
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
		expect(result).toBe(500_506_001)
		expect(result).toBeGreaterThan(100_000_000) // trivially exceeds any threshold
	})

	it("catches implicit-default heavy queries that omit `first`", () => {
		// Previously under-counted — now a nested un-paginated query correctly
		// scales with the MAX default at each level.
		// innermost: valuesList { propertyId text }  → MAX × 2 + 1 = 2001
		// entity { id valuesList }                    → MAX × (1 + 2001) + 1 = 2_002_001
		// entities { id entity { ... } }              → MAX × (1 + 2_002_001) + 1 = 2_002_002_001
		const result = cost(`{
			entities {
				id
				relationsList {
					toEntity { valuesList { propertyId text } }
				}
			}
		}`)
		expect(result).toBeGreaterThan(1_000_000_000) // billions — real SQL fan-out is huge
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

// --------------------------------------------------------------------------
// Prometheus histogram exposed via /health/metrics
// --------------------------------------------------------------------------

describe("query cost histogram", () => {
	beforeEach(() => {
		__resetQueryCostHistogramForTests()
	})

	it("empty state emits zero-counts for all buckets", () => {
		const out = renderQueryCostHistogram()
		expect(out).toMatch(/# TYPE gaia_api_graphql_query_cost histogram/)
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="\+Inf"} 0/)
		expect(out).toMatch(/gaia_api_graphql_query_cost_count 0/)
		expect(out).toMatch(/gaia_api_graphql_query_cost_sum 0/)
	})

	it("each recorded query increments the right buckets cumulatively", async () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		const runQuery = (query: string, variables: Record<string, unknown> = {}) => {
			onExecute({
				args: {document: parse(query), variableValues: variables, operationName: null},
			})
		}

		// Cost 51 → should hit le=100, le=1000, ..., le=+Inf (all ≥ 51)
		runQuery(`{ entities(first: 50) { id } }`)
		// Cost 1001 → hits le=10_000, le=100_000, ..., le=+Inf (buckets ≥ 1001)
		runQuery(`{ entities { id } }`)

		const out = renderQueryCostHistogram()
		expect(out).toContain("gaia_api_graphql_query_cost_count 2")
		expect(out).toContain(`gaia_api_graphql_query_cost_sum ${51 + 1001}`)
		// le=100: only the cost=51 observation qualifies
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="100"} 1$/m)
		// le=1000: still only cost=51 (1001 > 1000)
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="1000"} 1$/m)
		// le=10000: both observations
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="10000"} 2$/m)
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="\+Inf"} 2$/m)
	})

	it("skips introspection queries", async () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		onExecute({
			args: {
				document: parse(`{ __typename }`),
				variableValues: {},
				operationName: "IntrospectionQuery",
			},
		})
		expect(renderQueryCostHistogram()).toContain("gaia_api_graphql_query_cost_count 0")
	})
})
