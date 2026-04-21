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
		// Previously under-counted — now a nested un-paginated query scales
		// with the MAX default at each level. Math: innermost valuesList{2 scalars}
		// would be MAX × 2 + 1 = 2001, then entity { id valuesList } → MAX × 2002 + 1,
		// etc. Crosses the 1B cap at 3 levels of implicit defaults, so the walker
		// clamps to MAX_COST_CALC_LIMIT rather than drifting toward Infinity.
		const result = cost(`{
			entities {
				id
				relationsList {
					toEntity { valuesList { propertyId text } }
				}
			}
		}`)
		expect(result).toBe(1_000_000_000)
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

// --------------------------------------------------------------------------
// Safety invariants — the plugin must never throw into yoga (shadow mode
// must not break the request it was observing), and the cost math must
// never produce Infinity / NaN regardless of input.
// --------------------------------------------------------------------------

const MAX_COST_CALC_LIMIT = Number.parseInt(process.env.GRAPHQL_COST_CALC_LIMIT ?? "1000000000", 10)

describe("computeQueryCost — cap & overflow protection", () => {
	it("caps at MAX_COST_CALC_LIMIT (1B default) on an otherwise-astronomical query", () => {
		// 6 nested levels of first:1000 would mathematically be 1000^6 = 1e18
		// (way past Number.MAX_SAFE_INTEGER). With the cap in place the walker
		// bails as soon as the running total crosses 1B.
		const query = `
			{
				entities(first: 1000) {
					a: entities(first: 1000) {
						b: entities(first: 1000) {
							c: entities(first: 1000) {
								d: entities(first: 1000) {
									e: entities(first: 1000) { id }
								}
							}
						}
					}
				}
			}
		`
		expect(cost(query)).toBe(MAX_COST_CALC_LIMIT)
	})

	it("stops accumulating once a sibling pushes past the cap", () => {
		// The first branch alone exceeds the cap; the second branch shouldn't
		// be walked at all. Observable effect: result is exactly the cap.
		const query = `
			{
				first_branch: entities(first: 1000) {
					a: entities(first: 1000) {
						b: entities(first: 1000) {
							c: entities(first: 1000) { id }
						}
					}
				}
				second_branch: entities(first: 1000) { id }
			}
		`
		expect(cost(query)).toBe(MAX_COST_CALC_LIMIT)
	})

	it("never returns Infinity or NaN, even on pathologically deep inputs", () => {
		// Build a synthetic query 50 levels deep, each with first:1000.
		let q = "{ id }"
		for (let i = 0; i < 50; i++) {
			q = `{ entities(first: 1000) ${q} }`
		}
		const c = cost(q)
		expect(Number.isFinite(c)).toBe(true)
		expect(c).toBeLessThanOrEqual(MAX_COST_CALC_LIMIT)
	})

	it("handles a deeply-nested legit query without stack overflow", () => {
		// 30-level nesting (realistic upper bound for client queries). Should
		// return cleanly regardless of whether it hits the cap.
		let q = "{ id }"
		for (let i = 0; i < 30; i++) {
			q = `{ entity(id: "x") ${q} }`
		}
		expect(() => cost(q)).not.toThrow()
	})
})

describe("computeQueryCost — fragment cycles", () => {
	it("returns cleanly on a fragment-spread cycle (A → B → A)", () => {
		// NoFragmentCycles validation would normally reject this before
		// execute, but we defend against an upstream plugin skipping validation.
		const doc = `
			query Q { ...A }
			fragment A on Query { ...B }
			fragment B on Query { ...A }
		`
		expect(() => cost(doc)).not.toThrow()
		// Cost is 0 — the cycle is cut on second visit and neither fragment
		// selects any concrete field. The important thing is that it terminates.
		expect(cost(doc)).toBe(0)
	})

	it("allows a fragment to be used multiple times at sibling positions (not a cycle)", () => {
		// Visited fragments are popped after each recursion, so the same
		// fragment legitimately included twice as siblings does not get cut.
		const doc = `
			query Q { ...F ...F }
			fragment F on Query { a: __typename b: __typename }
		`
		// two sibling spreads × (a + b = 2 scalars) = 4
		expect(cost(doc)).toBe(4)
	})
})

describe("useCostLogger — shadow-mode safety", () => {
	it("does not throw when the cost walker throws", async () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		// Hand the plugin a malformed document so computeQueryCost throws on
		// the definitions.find lookup. The plugin should swallow, not rethrow.
		expect(() =>
			onExecute({
				args: {
					// biome-ignore lint/suspicious/noExplicitAny: deliberately malformed
					document: null as any,
					variableValues: {},
					operationName: null,
				},
			}),
		).not.toThrow()
	})

	it("does not throw when the high-cost log path throws", async () => {
		// Fake a variable that JSON.stringify can't serialize (circular ref).
		// The log call inside the plugin's high-cost branch would normally
		// blow up; the outer try/catch must keep the request flowing.
		const circular: Record<string, unknown> = {}
		circular.self = circular

		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		// A query guaranteed to cross the default 1M log threshold.
		expect(() =>
			onExecute({
				args: {
					document: parse(`{ entities(first: 1000) { relationsList { id } } }`),
					variableValues: {circular},
					operationName: null,
				},
			}),
		).not.toThrow()
	})
})
