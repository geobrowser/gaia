import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"

// Mock Sentry so we can assert on the new metric emission without a real
// Sentry init. Order matters: this mock must precede the import of the
// module under test.
vi.mock("@sentry/node", () => ({
	metrics: {
		distribution: vi.fn(),
	},
}))

import * as Sentry from "@sentry/node"
import {
	__resetQueryCostHistogramForTests,
	compressCost,
	computeQueryCost,
	computeQueryCostRaw,
	GRAPHQL_QUERY_COST_CONTEXT_KEY,
	renderQueryCostHistogram,
	useCostLogger,
} from "../costLoggerPlugin"
import {MAX_PAGINATION_LIMIT} from "../paginationCapPlugin"

/** Compressed-score helper (what gets recorded and logged). */
const cost = (query: string, variables: Record<string, unknown> = {}) => computeQueryCost(parse(query), variables)

/** Raw BigInt-cost helper — for tests that care about the exact multiplier math. */
const rawCost = (query: string, variables: Record<string, unknown> = {}) => computeQueryCostRaw(parse(query), variables)

// --------------------------------------------------------------------------
// compressCost — log₁₀×10 compression primitive
// --------------------------------------------------------------------------

describe("compressCost", () => {
	it("returns 0 for n ≤ 1 (trivial / empty)", () => {
		expect(compressCost(0n)).toBe(0)
		expect(compressCost(1n)).toBe(0)
	})

	it("round(log₁₀(n) × 10) — each 10-point step = one order of magnitude", () => {
		expect(compressCost(10n)).toBe(10) // log10(10) = 1.000
		expect(compressCost(100n)).toBe(20) // log10(100) = 2.000
		expect(compressCost(1_000n)).toBe(30)
		expect(compressCost(1_000_000n)).toBe(60)
		expect(compressCost(10n ** 15n)).toBe(150)
		expect(compressCost(10n ** 25n)).toBe(250)
	})

	it("rounds correctly between decades", () => {
		// log10(201) ≈ 2.303 → round(23.03) = 23
		expect(compressCost(201n)).toBe(23)
		// log10(51) ≈ 1.708 → round(17.08) = 17
		expect(compressCost(51n)).toBe(17)
	})

	it("handles BigInts far past Number.MAX_SAFE_INTEGER without precision loss", () => {
		// 27-digit number (past 2^53) — the kind of value cap-hitter
		// entity queries produce with D=1000. log₁₀(1.4 × 10²⁶) ≈ 26.146.
		const huge = 140_000_000_000_000_000_000_000_000n // 1.4 × 10²⁶
		expect(compressCost(huge)).toBe(261)
		// 300-digit number — adversarial-depth probe. log₁₀(10²⁹⁹) = 299.
		const astronomical = 10n ** 299n
		expect(compressCost(astronomical)).toBe(2990)
	})
})

// --------------------------------------------------------------------------
// Compressed-cost scores for realistic shapes
// --------------------------------------------------------------------------

describe("computeQueryCost — compressed scores", () => {
	it("scalar-only operation → 0", () => {
		// raw = 1 → compressed = 0
		expect(cost(`{ __typename }`)).toBe(0)
	})

	it("simple single-entity lookup → ~30", () => {
		// raw = 1000 × 2 + 1 = 2001, log10 ≈ 3.301 → 33
		expect(cost(`{ entity(id: "...") { id name } }`)).toBe(33)
	})

	it("first:50 single scalar → 17", () => {
		// raw = 51, log10(51) ≈ 1.708 → 17
		expect(cost(`{ entities(first: 50) { id } }`)).toBe(17)
	})

	it("last:25 single scalar → 14", () => {
		// raw = 26, log10 ≈ 1.415 → 14
		expect(cost(`{ entities(last: 25) { id } }`)).toBe(14)
	})

	it("nested-list with explicit pagination → 24", () => {
		// inner relations(first:10){id} = 11
		// outer entities(first:20){id + inner} = 20 × (1 + 11) + 1 = 241
		// log10(241) ≈ 2.382 → 24
		expect(
			cost(`{
				entities(first: 20) {
					id
					relations(first: 10) { id }
				}
			}`),
		).toBe(24)
	})
})

// --------------------------------------------------------------------------
// Exact-raw-cost regression tests (multiplier math)
// --------------------------------------------------------------------------

describe("computeQueryCostRaw — multiplier math", () => {
	it("explicit-pagination nested: `entities(first:20) { id, relations(first:10){id} }` → 241n", () => {
		expect(
			rawCost(`{
				entities(first: 20) {
					id
					relations(first: 10) { id }
				}
			}`),
		).toBe(241n)
	})

	it("implicit-default heavy query: three unpaginated levels compound at 1000×", () => {
		// valuesList { 2 scalars }             = 1000 × 2 + 1   = 2001
		// toEntity { valuesList }              = 1000 × 2001 + 1 = 2_001_001
		// relationsList { toEntity }           = 1000 × 2_001_001 + 1 = 2_001_001_001
		// entities { id, relationsList }       = 1000 × 2_001_001_002 + 1 = 2_001_001_002_001
		expect(
			rawCost(`{
				entities {
					id
					relationsList {
						toEntity { valuesList { propertyId text } }
					}
				}
			}`),
		).toBe(2_001_001_002_001n)
	})

	it("real prod shape: `entitiesConnection` with 3-level nested relations", () => {
		// Captured from prod logs (fingerprint gql:1424e9e7). Explicit
		// first:1000 at the root + 3 levels of unbounded `{ nodes {...} }`
		// nested connections produces a raw cost deep into 22-digit territory.
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
		const raw = rawCost(query, {type: ["7ed45f2bc48b419e8e4664d5ff680b0d"], first: 1000, after: null})
		// Raw is a 22-digit BigInt. Exact value is stable across runs since
		// the walker is deterministic.
		expect(raw.toString().length).toBeGreaterThanOrEqual(22)
		expect(raw.toString().length).toBeLessThanOrEqual(23)
		// Compressed score of the prod cap-hitter: ~220 (≈10²² raw cost).
		expect(compressCost(raw)).toBeGreaterThanOrEqual(215)
		expect(compressCost(raw)).toBeLessThanOrEqual(225)
	})
})

// --------------------------------------------------------------------------
// Adversarial / pathological inputs — must not crash, must produce bounded
// (if large) scores.
// --------------------------------------------------------------------------

describe("computeQueryCost — adversarial inputs", () => {
	it("50-level deeply nested `entities(first:1000)` → score ~1500, no throw", () => {
		// raw = 1000^50 ≈ 10^150 → compressed = 1500.
		let q = "{ id }"
		for (let i = 0; i < 50; i++) q = `{ entities(first: 1000) ${q} }`
		const c = cost(q)
		expect(Number.isFinite(c)).toBe(true)
		expect(c).toBeGreaterThanOrEqual(1500)
		expect(c).toBeLessThanOrEqual(1510)
	})

	it("100-level nesting — BigInt walker handles it, score scales linearly with depth", () => {
		// raw = 1000^100 = 10^300 → compressed = 3000.
		let q = "{ id }"
		for (let i = 0; i < 100; i++) q = `{ entities(first: 1000) ${q} }`
		const c = cost(q)
		expect(c).toBeGreaterThanOrEqual(3000)
		expect(c).toBeLessThanOrEqual(3010)
	})

	it("wide fan-out: 100 sibling aliased collections → modest score, linear in width", () => {
		// Each sibling: entities(first:1000){id} = 1001 → 100 siblings sum = ~100_100.
		// log10(100_100) ≈ 5.0 → compressed ≈ 50.
		const aliases = Array.from({length: 100}, (_, i) => `a${i}: entities(first: 1000) { id }`).join(" ")
		const c = cost(`{ ${aliases} }`)
		expect(c).toBeGreaterThanOrEqual(45)
		expect(c).toBeLessThanOrEqual(55)
	})

	it("attacker attempts `first: 0` to sneak nested work past the estimator", () => {
		// first:0 is non-positive → fallback to MAX_PAGINATION_LIMIT. Nested
		// relations(first:50){id} still charged.
		// inner = 51, outer = 1000 × (1 + 51) + 1 = 52001 → compressed ~47.
		const raw = rawCost(`{
			entities(first: 0) {
				id
				relations(first: 50) { id }
			}
		}`)
		expect(raw).toBe(BigInt(MAX_PAGINATION_LIMIT) * 52n + 1n)
		expect(compressCost(raw)).toBe(47)
	})

	it("attacker attempts negative `first`", () => {
		expect(rawCost(`{ entities(first: -100) { id } }`)).toBe(BigInt(MAX_PAGINATION_LIMIT) + 1n)
	})

	it("unresolved variable in `first:` falls back to MAX_PAGINATION_LIMIT", () => {
		expect(rawCost(`query Q($first: Int) { entities(first: $first) { id } }`, {})).toBe(
			BigInt(MAX_PAGINATION_LIMIT) + 1n,
		)
	})

	it("fragment cycle terminates cleanly (defence-in-depth vs skipped validation)", () => {
		const doc = `
			query Q { ...A }
			fragment A on Query { ...B }
			fragment B on Query { ...A }
		`
		expect(() => cost(doc)).not.toThrow()
		expect(cost(doc)).toBe(0)
	})

	it("malformed input (null document) does not escape the plugin", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
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
})

// --------------------------------------------------------------------------
// Pagination arg resolution + variables
// --------------------------------------------------------------------------

describe("computeQueryCost — pagination + variables", () => {
	it("resolves pagination args supplied via variables", () => {
		expect(rawCost(`query Q($first: Int!) { entities(first: $first) { id } }`, {first: 100})).toBe(101n)
	})

	it("sibling with negative outer still charges for valid nested first", () => {
		// Inner relations(first:50){id} = 51.
		// Outer entities(first:-5) → MAX_PAGINATION_LIMIT × (1 + 51) + 1 = 52_001
		expect(
			rawCost(`{
				entities(first: -5) {
					id
					relations(first: 50) { id }
				}
			}`),
		).toBe(BigInt(MAX_PAGINATION_LIMIT) * 52n + 1n)
	})
})

// --------------------------------------------------------------------------
// Fragments
// --------------------------------------------------------------------------

describe("computeQueryCost — fragments", () => {
	it("inline fragments contribute to parent's childComplexity", () => {
		// entities(first:10) { ... on Entity { id name } }
		// inline frag = 2 → 10 × 2 + 1 = 21
		expect(rawCost(`{ entities(first: 10) { ... on Entity { id name } } }`)).toBe(21n)
	})

	it("fragment spreads follow the definition", () => {
		expect(
			rawCost(`
				query { entities(first: 10) { ...F } }
				fragment F on Entity { id name }
			`),
		).toBe(21n)
	})

	it("allows a fragment to be used multiple times at sibling positions (not a cycle)", () => {
		// Visited fragments are popped after each recursion.
		expect(
			rawCost(`
				query Q { ...F ...F }
				fragment F on Query { a: __typename b: __typename }
			`),
		).toBe(4n)
	})
})

// --------------------------------------------------------------------------
// Empty / edge
// --------------------------------------------------------------------------

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

	it("records compressed scores into the right buckets", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		const runQuery = (query: string, variables: Record<string, unknown> = {}) => {
			onExecute({args: {document: parse(query), variableValues: variables, operationName: null}})
		}

		// Score 0 — trivial.
		runQuery(`{ __typename }`)
		// Score 33 — simple entity.
		runQuery(`{ entity(id: "...") { id name } }`)

		const out = renderQueryCostHistogram()
		expect(out).toContain("gaia_api_graphql_query_cost_count 2")
		expect(out).toContain(`gaia_api_graphql_query_cost_sum ${0 + 33}`)
		// le=30: only the trivial observation (33 > 30)
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="30"} 1$/m)
		// le=60: both
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="60"} 2$/m)
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="\+Inf"} 2$/m)
	})

	it("adversarial pathological query records to the top bucket", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		let q = "{ id }"
		for (let i = 0; i < 50; i++) q = `{ entities(first: 1000) ${q} }`
		onExecute({args: {document: parse(q), variableValues: {}, operationName: null}})

		const out = renderQueryCostHistogram()
		// Score ~1500 lands in the 500 bucket? No — 1500 > 500 → +Inf.
		// Goes into le=1000 and +Inf but not le=500.
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="500"} 0$/m)
		expect(out).toMatch(/gaia_api_graphql_query_cost_bucket\{le="\+Inf"} 1$/m)
	})

	it("skips introspection queries", () => {
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
// Sentry metric emission + cross-plugin cost sharing via contextValue
// --------------------------------------------------------------------------

describe("useCostLogger — Sentry metric + context cost-sharing", () => {
	beforeEach(() => {
		__resetQueryCostHistogramForTests()
		vi.clearAllMocks()
	})

	it("emits a Sentry distribution metric on every (non-introspection) query", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		onExecute({
			args: {
				document: parse(`{ entity(id: "...") { id name } }`),
				variableValues: {},
				operationName: null,
				contextValue: {},
			},
		})

		expect(Sentry.metrics.distribution).toHaveBeenCalledTimes(1)
		expect(Sentry.metrics.distribution).toHaveBeenCalledWith(
			"graphql.query_cost",
			expect.any(Number),
			expect.objectContaining({attributes: expect.objectContaining({operation: "query entity"})}),
		)
	})

	it("does not emit a Sentry metric for introspection queries", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		onExecute({
			args: {
				document: parse(`{ __typename }`),
				variableValues: {},
				operationName: "IntrospectionQuery",
				contextValue: {},
			},
		})

		expect(Sentry.metrics.distribution).not.toHaveBeenCalled()
	})

	it("stashes cost on contextValue for downstream plugins to read", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		const ctx: Record<string, unknown> = {}

		onExecute({
			args: {
				document: parse(`{ entity(id: "...") { id name } }`),
				variableValues: {},
				operationName: null,
				contextValue: ctx,
			},
		})

		// Same compressed score the histogram bucketing test asserts (33 for
		// a simple entity lookup).
		expect(ctx[GRAPHQL_QUERY_COST_CONTEXT_KEY]).toBe(33)
	})

	it("does not stash cost when computation fails (shadow-mode safety)", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		const ctx: Record<string, unknown> = {}

		onExecute({
			args: {
				// biome-ignore lint/suspicious/noExplicitAny: deliberately malformed
				document: null as any,
				variableValues: {},
				operationName: null,
				contextValue: ctx,
			},
		})

		expect(ctx[GRAPHQL_QUERY_COST_CONTEXT_KEY]).toBeUndefined()
		expect(Sentry.metrics.distribution).not.toHaveBeenCalled()
	})
})

// --------------------------------------------------------------------------
// Shadow-mode safety: the plugin must never throw into yoga, regardless of
// walker output, log serialization failures, etc.
// --------------------------------------------------------------------------

describe("useCostLogger — shadow-mode safety", () => {
	it("does not throw when the cost walker throws", () => {
		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
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

	it("does not throw when the high-cost log path throws", () => {
		// Circular variable defeats JSON.stringify inside log.warn. The plugin's
		// outer try/catch must swallow it.
		const circular: Record<string, unknown> = {}
		circular.self = circular

		const plugin = useCostLogger()
		const onExecute = (plugin as {onExecute: (args: unknown) => void}).onExecute
		// 50-level deep query → compressed score ~1500, well above the 200 threshold,
		// so the log path fires.
		let q = "{ id }"
		for (let i = 0; i < 50; i++) q = `{ entities(first: 1000) ${q} }`
		expect(() =>
			onExecute({
				args: {
					document: parse(q),
					variableValues: {circular},
					operationName: null,
				},
			}),
		).not.toThrow()
	})

	it("handles a deeply-nested legit query without stack overflow", () => {
		// 30-level nesting (realistic upper bound for client queries).
		let q = "{ id }"
		for (let i = 0; i < 30; i++) q = `{ entity(id: "x") ${q} }`
		expect(() => cost(q)).not.toThrow()
	})
})
