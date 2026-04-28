import {type ASTNode, GraphQLError, Kind, parse} from "graphql"
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"

// Mock telemetry so we can assert what gets logged. Must precede the import
// of the plugin. `log` is destructured at call-time by the plugin, so this
// stub reaches into the same module.
vi.mock("../../services/telemetry", () => ({
	log: {
		debug: vi.fn(),
		info: vi.fn(),
		warn: vi.fn(),
		error: vi.fn(),
	},
}))

// Mock Sentry to prevent real network calls + so we can ignore captureException.
vi.mock("@sentry/node", () => ({
	captureException: vi.fn(),
	metrics: {distribution: vi.fn()},
}))

import {log} from "../../services/telemetry"
import {GRAPHQL_QUERY_COST_CONTEXT_KEY} from "../costLoggerPlugin"
import {
	__resetResponseSizeHistogramForTests,
	isClientError,
	renderResponseSizeHistogram,
	useGraphQLInstrumentation,
} from "../instrumentationPlugin"

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

// --------------------------------------------------------------------------
// Slow-query / large-response logs include cost when stashed by useCostLogger
// --------------------------------------------------------------------------

describe("useGraphQLInstrumentation — query cost in slow / large logs", () => {
	const SLOW_QUERY_THRESHOLD_MS = 3000

	beforeEach(() => {
		vi.clearAllMocks()
		vi.useFakeTimers()
	})

	afterEach(() => {
		vi.useRealTimers()
	})

	function runRequest(opts: {durationMs: number; cost?: number; responseData?: unknown}) {
		const plugin = useGraphQLInstrumentation()
		const onExecute = (plugin as {onExecute: (args: unknown) => {onExecuteDone: (r: unknown) => void}}).onExecute

		const ctx: Record<string, unknown> = {}
		if (opts.cost !== undefined) {
			ctx[GRAPHQL_QUERY_COST_CONTEXT_KEY] = opts.cost
		}

		const startTime = Date.now()
		vi.setSystemTime(startTime)

		const args = {
			document: parse(`{ entity(id: "...") { id name } }`),
			variableValues: {},
			operationName: null,
			contextValue: ctx,
		}

		const handle = onExecute({args})
		vi.setSystemTime(startTime + opts.durationMs)
		handle.onExecuteDone({result: {data: opts.responseData ?? {ok: true}}})
	}

	it("Slow GraphQL query log includes the cost field when context has it", () => {
		runRequest({durationMs: SLOW_QUERY_THRESHOLD_MS + 1, cost: 235})

		expect(log.warn).toHaveBeenCalledWith("Slow GraphQL query", expect.objectContaining({queryCost: 235}))
	})

	it("Slow GraphQL query log omits cost when context has none", () => {
		runRequest({durationMs: SLOW_QUERY_THRESHOLD_MS + 1})

		const slowCalls = (log.warn as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === "Slow GraphQL query")
		expect(slowCalls).toHaveLength(1)
		const fields = slowCalls[0]?.[1]
		expect(fields).not.toHaveProperty("queryCost")
	})

	it("Large GraphQL response log includes the cost field when context has it", () => {
		// Build a response that stringifies to >1MB so the large-response branch fires.
		const big = {data: "x".repeat(1_100_000)}
		runRequest({durationMs: 1500, cost: 250, responseData: big})

		expect(log.warn).toHaveBeenCalledWith("Large GraphQL response", expect.objectContaining({queryCost: 250}))
	})

	it("does not fire slow log under threshold", () => {
		runRequest({durationMs: SLOW_QUERY_THRESHOLD_MS - 1, cost: 235})

		const slowCalls = (log.warn as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === "Slow GraphQL query")
		expect(slowCalls).toHaveLength(0)
	})
})

// --------------------------------------------------------------------------
// Prometheus histogram exposed via /health/metrics
// --------------------------------------------------------------------------

describe("response size histogram", () => {
	beforeEach(() => {
		__resetResponseSizeHistogramForTests()
		vi.clearAllMocks()
		vi.useFakeTimers()
	})

	afterEach(() => {
		vi.useRealTimers()
	})

	function runRequest(opts: {durationMs: number; responseData: unknown}) {
		const plugin = useGraphQLInstrumentation()
		const onExecute = (plugin as {onExecute: (args: unknown) => {onExecuteDone: (r: unknown) => void}}).onExecute

		const startTime = Date.now()
		vi.setSystemTime(startTime)

		const handle = onExecute({
			args: {
				document: parse(`{ entity(id: "...") { id name } }`),
				variableValues: {},
				operationName: null,
				contextValue: {},
			},
		})
		vi.setSystemTime(startTime + opts.durationMs)
		handle.onExecuteDone({result: {data: opts.responseData}})
	}

	it("empty state emits zero counts for all buckets", () => {
		const out = renderResponseSizeHistogram()
		expect(out).toMatch(/# TYPE gaia_api_graphql_response_size_bytes histogram/)
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_bucket\{le="\+Inf"} 0/)
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_count 0/)
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_sum 0/)
	})

	it("does not record fast queries (durationMs < 1000ms gate)", () => {
		runRequest({durationMs: 500, responseData: {x: "y"}})
		expect(renderResponseSizeHistogram()).toContain("gaia_api_graphql_response_size_bytes_count 0")
	})

	it("records observations for slow queries into the right buckets", () => {
		// ~300 KB response — falls into le=500_000
		runRequest({durationMs: 1500, responseData: {data: "x".repeat(300_000)}})
		// ~1.5 MB response — falls into le=2_000_000
		runRequest({durationMs: 1500, responseData: {data: "x".repeat(1_500_000)}})

		const out = renderResponseSizeHistogram()
		expect(out).toContain("gaia_api_graphql_response_size_bytes_count 2")
		// 500K bucket — only the 300K observation
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_bucket\{le="500000"} 1$/m)
		// 1M bucket — still only the 300K (1.5M > 1M)
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_bucket\{le="1000000"} 1$/m)
		// 2M bucket — both fit
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_bucket\{le="2000000"} 2$/m)
		// +Inf — both
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_bucket\{le="\+Inf"} 2$/m)
	})

	it("very large responses land beyond the top bucket (+Inf only)", () => {
		// 110 MB exceeds the 100 MB top bucket
		runRequest({durationMs: 1500, responseData: {data: "x".repeat(110_000_000)}})

		const out = renderResponseSizeHistogram()
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_bucket\{le="100000000"} 0$/m)
		expect(out).toMatch(/gaia_api_graphql_response_size_bytes_bucket\{le="\+Inf"} 1$/m)
	})
})
