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
		// Conservative model: each un-paginated structured field defaults to
		// MAX_PAGINATION_LIMIT (1000). The nested shape below multiplies out to
		// valuesList{2 scalars} = 2001, toEntity{valuesList} = 2_001_001,
		// relationsList{toEntity} = 2_001_001_001, entities{id, relationsList} =
		// 2_001_001_002_001 (~2T). Under the 500T cap this is the exact cost —
		// useful as a regression fixture for the multiplier math itself.
		const result = cost(`{
			entities {
				id
				relationsList {
					toEntity { valuesList { propertyId text } }
				}
			}
		}`)
		expect(result).toBe(2_001_001_002_001)
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

		// Cost 1001 → hits le=100_000, ..., le=+Inf (all buckets ≥ 1001)
		runQuery(`{ entities { id } }`)
		// Cost 100_001 (first:100_000 × id) is rejected by PaginationCapPlugin in
		// the server path, but computeQueryCost is purely schema-free and happily
		// evaluates AST-derived multipliers — pick a construction that produces
		// a cost between 100k and 1M: entities(first:1000){id name} = 2001,
		// that's still ≤ 100_000 so it only adds to the smaller bucket. Use a
		// deliberately heavier query to straddle the 100k/1M edges.
		runQuery(`{ entities(first: 500) { id name values(first: 5) { propertyId text } } }`) // 500*(1+1+5*(1+1)+1) + 1 = 500*13 + 1 = 6501

		const out = renderQueryCostHistogram()
		expect(out).toContain("gaia_api_graphql_query_cost_count 2")
		expect(out).toContain(`gaia_api_graphql_query_cost_sum ${1001 + 6501}`)
		// le=100000: both observations qualify (1001 and 6501 both ≤ 100_000)
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="100000"} 2$/m)
		// le=1000000: still both
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="1000000"} 2$/m)
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

const MAX_COST_CALC_LIMIT = Number.parseInt(process.env.GRAPHQL_COST_CALC_LIMIT ?? "9000000000000000", 10)

describe("computeQueryCost — cap & overflow protection", () => {
	it("MAX_COST_CALC_LIMIT stays within Number.MAX_SAFE_INTEGER", () => {
		// Regression guard: the cap must be representable exactly as a JS
		// Number. Above 2^53 - 1 (~9.007e15), integer arithmetic starts rounding,
		// which would break the `Math.min(child * limit + 1, cap)` clamp and the
		// `child >= cap` short-circuit — both rely on exact equality/comparison
		// at the cap magnitude. If someone bumps the cap past this point they
		// need BigInt, not just a bigger Number literal.
		expect(MAX_COST_CALC_LIMIT).toBeLessThanOrEqual(Number.MAX_SAFE_INTEGER)
		// And we still have unit precision at this magnitude (cap+1 > cap,
		// cap-1 < cap). If we drift past MAX_SAFE_INTEGER these fail silently.
		expect(MAX_COST_CALC_LIMIT + 1).toBeGreaterThan(MAX_COST_CALC_LIMIT)
		expect(MAX_COST_CALC_LIMIT - 1).toBeLessThan(MAX_COST_CALC_LIMIT)
	})

	it("caps at MAX_COST_CALC_LIMIT (9P default) on an otherwise-astronomical query", () => {
		// 6 nested levels of first:1000 would mathematically be 1000^6 = 1e18
		// (way past Number.MAX_SAFE_INTEGER). With the 9P cap in place the
		// walker bails as soon as the running total crosses it.
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
		// Needs 6 `first:1000` levels to clear the 500T cap: 1000^6 = 10^18 > 500T.
		const query = `
			{
				first_branch: entities(first: 1000) {
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

	it("caps on the production `entitiesConnection` query that produced a ~45 MB response", () => {
		// Real query captured from prod stdout during the OOM investigation
		// (fingerprint gql:1424e9e7). first:1000 at the root plus three levels
		// of unbounded nested `{ nodes { ... } }` connections — the effective
		// multiplier chain is ~1000^7 times leaf-scalar counts, so the
		// unclamped cost is on the order of 10^22. The walker crosses the
		// 9P cap almost immediately and short-circuits. Kept as a regression
		// fixture so future changes to the cost model don't quietly start
		// returning a finite value here.
		const query = `
			query GetEntities($type: [UUID!], $first: Int!, $after: Cursor) {
				entitiesConnection(
					first: $first
					after: $after
					filter: {relations: {some: {typeId: {is: "8f151ba4de204e3c9cb499ddf96f48f1"}, toEntityId: {in: $type}}}}
				) {
					pageInfo { hasNextPage endCursor }
					nodes {
						id
						name
						values {
							nodes {
								spaceId propertyId text language date datetime time
								integer float decimal unit boolean point
							}
						}
						relations {
							nodes {
								id spaceId fromEntityId toEntityId typeId verified
								position toSpaceId entityId
								entity {
									id
									name
									values {
										nodes {
											spaceId propertyId text language date datetime time
											integer float decimal unit boolean point
										}
									}
									relations {
										nodes {
											id spaceId fromEntityId toEntityId typeId
											position toSpaceId entityId
										}
									}
								}
							}
						}
					}
				}
			}
		`
		const result = cost(query, {
			type: ["7ed45f2bc48b419e8e4664d5ff680b0d"],
			first: 1000,
			after: null,
		})
		expect(result).toBe(MAX_COST_CALC_LIMIT)
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
