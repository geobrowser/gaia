import {parse} from "graphql"
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"
import {log} from "../../services/telemetry"
import {
	__resetResponseByteMetricsForTests,
	estimateJsonBytes,
	GRAPHQL_RESPONSE_BYTES_CONTEXT_KEY,
	measureJsonBytes,
	RESPONSE_BYTE_CEILING,
	RESPONSE_BYTE_LOG_THRESHOLD,
	renderResponseByteMetrics,
	useResponseBudget,
} from "../responseBudgetPlugin"

// --------------------------------------------------------------------------
// measureJsonBytes — the counting primitive
// --------------------------------------------------------------------------
// Two properties matter, and they pull in opposite directions:
//   1. Unbounded, it must agree with JSON.stringify().length, because that is
//      what the number is claiming to be.
//   2. Bounded, it must stop early — the whole reason it can run on every
//      response is that an oversized one is abandoned rather than walked.
// Tests for (1) that don't also pin (2) would pass on an implementation that
// silently ignored `limit`, so early exit is asserted by counting node visits,
// not by trusting the returned flag.

describe("measureJsonBytes — unbounded agreement with JSON.stringify", () => {
	const agrees = (value: unknown) =>
		expect(measureJsonBytes(value, Number.POSITIVE_INFINITY).bytes).toBe(JSON.stringify(value).length)

	it("matches on scalars", () => {
		for (const value of [null, true, false, 0, 1234, -42, 1.5, "abc", ""]) agrees(value)
	})

	it("matches on a realistic entity payload", () => {
		agrees({
			entities: {
				nodes: [
					{
						id: "0f8c1a2e-1111-4444-8888-aaaabbbbcccc",
						name: "Claim",
						description: null,
						relations: {
							nodes: [
								{id: "r1", toEntity: {id: "e1", name: "Thing"}},
								{id: "r2", toEntity: {id: "e2", name: null}},
							],
						},
					},
				],
			},
		})
	})

	it("matches on empty containers and nesting", () => {
		agrees({})
		agrees([])
		agrees({a: {}, b: []})
		agrees({a: [{b: [{c: [1, 2, 3]}]}]})
	})

	it("matches on the values JSON.stringify treats specially", () => {
		// undefined / functions omitted from objects, but null'd inside arrays.
		agrees({a: 1, b: undefined, c: 2})
		agrees([1, undefined, 2])
		// Non-finite numbers serialize as null.
		agrees({a: Number.NaN, b: Number.POSITIVE_INFINITY})
		// toJSON is respected rather than walked.
		agrees({at: new Date("2026-08-19T21:12:57.000Z")})
	})

	it("`exceeded` is false when unbounded, whatever the size", () => {
		const big = {rows: Array.from({length: 5000}, (_, i) => ({id: i, name: `row-${i}`}))}
		expect(measureJsonBytes(big, Number.POSITIVE_INFINITY).exceeded).toBe(false)
	})
})

describe("measureJsonBytes — early exit", () => {
	/**
	 * Build an array whose elements each report when they are visited. `toJSON`
	 * is the hook because the walker honours it, so a visit is observable
	 * without a Proxy.
	 */
	function countingRows(count: number, payload: string) {
		let visits = 0
		const rows = Array.from({length: count}, () => ({
			toJSON() {
				visits++
				return payload
			},
		}))
		return {rows, visited: () => visits}
	}

	it("stops walking once the running total passes the limit", () => {
		// 1000 rows x ~1002 bytes each ≈ 1 MB total; a 10 KB limit should be hit
		// after roughly 10 rows.
		const {rows, visited} = countingRows(1000, "x".repeat(1000))

		const result = measureJsonBytes(rows, 10_000)

		expect(result.exceeded).toBe(true)
		// Generous bound: the point is "a small fraction", not an exact count.
		expect(visited()).toBeLessThan(50)
	})

	it("visits everything when the limit is not reached — the control for the above", () => {
		// Same payload, limit above the total. If this did not visit all 1000,
		// the assertion above would pass on a walker that just stopped early
		// always.
		const {rows, visited} = countingRows(1000, "x".repeat(1000))

		const result = measureJsonBytes(rows, 100_000_000)

		expect(result.exceeded).toBe(false)
		expect(visited()).toBe(1000)
	})

	it("stops inside a wide object, not just a long array", () => {
		let visits = 0
		const wide: Record<string, unknown> = {}
		for (let i = 0; i < 1000; i++) {
			wide[`key${i}`] = {
				toJSON() {
					visits++
					return "y".repeat(1000)
				},
			}
		}

		expect(measureJsonBytes(wide, 10_000).exceeded).toBe(true)
		expect(visits).toBeLessThan(50)
	})

	it("reports a lower bound, not the real size, once it stops", () => {
		const {rows} = countingRows(1000, "x".repeat(1000))

		const bounded = measureJsonBytes(rows, 10_000)
		const exact = measureJsonBytes(rows, Number.POSITIVE_INFINITY)

		expect(bounded.bytes).toBeGreaterThan(10_000)
		expect(bounded.bytes).toBeLessThan(exact.bytes)
	})

	it("does not flag a value that only reaches the limit exactly", () => {
		// "abcdefgh" serializes to 10 bytes with its quotes.
		expect(measureJsonBytes("abcdefgh", 10)).toEqual({bytes: 10, exceeded: false})
		expect(measureJsonBytes("abcdefghi", 10)).toEqual({bytes: 11, exceeded: true})
	})

	it("flags an oversized scalar, which never re-enters the walk", () => {
		// A single huge string is measured in one step, so the top-of-walk guard
		// cannot have fired. The final comparison has to catch it.
		expect(measureJsonBytes("z".repeat(50_000), 10_000).exceeded).toBe(true)
	})

	it("does not blow the stack on deeply nested data", () => {
		let nested: unknown = {leaf: true}
		for (let i = 0; i < 2000; i++) nested = {child: nested}
		expect(() => measureJsonBytes(nested, 1_000_000)).not.toThrow()
	})
})

describe("estimateJsonBytes", () => {
	it("is the unbounded measurement", () => {
		const value = {a: [1, 2, "three"], b: {c: null}}
		expect(estimateJsonBytes(value)).toBe(JSON.stringify(value).length)
	})
})

// --------------------------------------------------------------------------
// Estimator bias — load-bearing for both the ceiling and the cache pre-check
// --------------------------------------------------------------------------
// Every shortcut in the walker (UTF-16 code units, no escape scanning) has to
// bias DOWNWARD, so "the estimate says oversized" implies "really oversized".
// valkeyCache.exceedsCacheableBytesEstimate depends on exactly this.

describe("measureJsonBytes — bias is never upward", () => {
	const cases: Record<string, unknown> = {
		ascii: {name: "plain ascii text"},
		cjk: {name: "地理協定書のエンティティ名"},
		emoji: {name: "🌍🛰️🗺️ mapped"},
		quotes: {name: 'he said "hello" and \\ left'},
		newlines: {name: "line one\nline two\ttabbed"},
		controlChars: {name: ""},
		mixed: {name: 'café 北京 🎉 "quoted"\n'},
	}

	for (const [label, value] of Object.entries(cases)) {
		it(`never over-counts real UTF-8 bytes: ${label}`, () => {
			const estimate = estimateJsonBytes(value)
			const real = Buffer.byteLength(JSON.stringify(value), "utf8")
			expect(estimate).toBeLessThanOrEqual(real)
		})
	}
})

// --------------------------------------------------------------------------
// useResponseBudget
// --------------------------------------------------------------------------
// The env-derived thresholds are read at module load, so these tests assert
// against the shipped defaults (log threshold 10 MB, ceiling 0 = off) rather
// than re-importing the module per case. Enforcement is exercised by calling
// the same code path with an explicit ceiling where the plugin reads one.

type PluginHarness = {
	run: (result: unknown, ctx?: Record<string, unknown>) => unknown
	setResult: ReturnType<typeof vi.fn>
}

const QUERY = `{ entities(first: 1000) { id name } }`

function harness(document = parse(QUERY), variableValues: Record<string, unknown> = {}): PluginHarness {
	const plugin = useResponseBudget()
	const onExecute = (
		plugin as {
			onExecute: (payload: unknown) => {onExecuteDone: (payload: unknown) => void}
		}
	).onExecute
	const setResult = vi.fn()

	return {
		setResult,
		run(result, ctx = {}) {
			const {onExecuteDone} = onExecute({
				args: {document, variableValues, operationName: null, contextValue: ctx},
			})
			onExecuteDone({result, setResult})
			return ctx
		},
	}
}

/** A `data` payload whose estimated size is at least `bytes`. */
function payloadOfAtLeast(bytes: number) {
	return {entities: [{id: "e", name: "x".repeat(bytes)}]}
}

describe("useResponseBudget — observation", () => {
	beforeEach(() => {
		__resetResponseByteMetricsForTests()
		vi.clearAllMocks()
	})

	afterEach(() => {
		vi.restoreAllMocks()
	})

	it("ships with enforcement off, so a large response is measured and still delivered", () => {
		// Guards the Phase 1 promise. If this ever fails, someone armed the
		// ceiling by changing a default rather than by setting the env var.
		expect(RESPONSE_BYTE_CEILING).toBe(0)

		const warn = vi.spyOn(log, "warn").mockImplementation(() => {})
		const h = harness()

		h.run({data: payloadOfAtLeast(RESPONSE_BYTE_LOG_THRESHOLD + 1000)})

		expect(h.setResult).not.toHaveBeenCalled()
		expect(warn).toHaveBeenCalledWith(
			"GraphQL response over byte budget",
			expect.objectContaining({enforced: false}),
		)
	})

	it("records every response in the histogram, not just large ones", () => {
		const h = harness()

		h.run({data: {entities: [{id: "a"}]}})
		h.run({data: {entities: [{id: "b"}]}})

		const metrics = renderResponseByteMetrics()
		expect(metrics).toContain("gaia_api_graphql_response_bytes_count 2")
		expect(metrics).toMatch(/gaia_api_graphql_response_bytes_bucket\{le="10000"\} 2/)
	})

	it("counts over-threshold responses without counting them as refused", () => {
		vi.spyOn(log, "warn").mockImplementation(() => {})
		const h = harness()

		h.run({data: payloadOfAtLeast(RESPONSE_BYTE_LOG_THRESHOLD + 1000)})

		const metrics = renderResponseByteMetrics()
		expect(metrics).toContain("gaia_api_graphql_response_budget_exceeded_total 1")
		expect(metrics).toContain("gaia_api_graphql_response_budget_refused_total 0")
	})

	it("does not warn about an ordinary response", () => {
		const warn = vi.spyOn(log, "warn").mockImplementation(() => {})
		const h = harness()

		h.run({data: {entities: [{id: "a", name: "small"}]}})

		expect(warn).not.toHaveBeenCalled()
	})

	it("shares its measurement on the request context", () => {
		const h = harness()
		const data = {entities: [{id: "a", name: "small"}]}

		const ctx = h.run({data}) as Record<string, unknown>

		expect(ctx[GRAPHQL_RESPONSE_BYTES_CONTEXT_KEY]).toEqual({
			bytes: JSON.stringify(data).length,
			exceeded: false,
		})
	})

	it("attributes the caller on the warning", () => {
		const warn = vi.spyOn(log, "warn").mockImplementation(() => {})
		const h = harness()

		h.run(
			{data: payloadOfAtLeast(RESPONSE_BYTE_LOG_THRESHOLD + 1000)},
			{
				request: new Request("https://api.test/graphql", {
					headers: {"user-agent": "postgres_to_geo/1.0", origin: "https://explorers.test"},
				}),
			},
		)

		expect(warn).toHaveBeenCalledWith(
			"GraphQL response over byte budget",
			expect.objectContaining({
				userAgent: "postgres_to_geo/1.0",
				origin: "https://explorers.test",
				queryFingerprint: expect.any(String),
			}),
		)
	})

	it("keeps caller keys present as null when the headers are absent", () => {
		// A server-side caller sends neither header. The keys must still exist so
		// log queries can filter on `userAgent = null` rather than silently
		// matching nothing.
		const warn = vi.spyOn(log, "warn").mockImplementation(() => {})
		const h = harness()

		h.run({data: payloadOfAtLeast(RESPONSE_BYTE_LOG_THRESHOLD + 1000)}, {})

		expect(warn).toHaveBeenCalledWith(
			"GraphQL response over byte budget",
			expect.objectContaining({userAgent: null, origin: null, clientIp: null}),
		)
	})
})

describe("useResponseBudget — leaves alone what it should", () => {
	beforeEach(() => {
		__resetResponseByteMetricsForTests()
	})

	it("ignores an errors-only result", () => {
		const h = harness()
		h.run({errors: [{message: "boom"}]})
		expect(renderResponseByteMetrics()).toContain("gaia_api_graphql_response_bytes_count 0")
	})

	it("ignores a null data result", () => {
		const h = harness()
		h.run({data: null})
		expect(renderResponseByteMetrics()).toContain("gaia_api_graphql_response_bytes_count 0")
	})

	it("ignores an async-iterable result (@defer / @stream)", () => {
		const h = harness()
		const stream = {
			async *[Symbol.asyncIterator]() {
				yield {data: {entities: []}}
			},
		}

		h.run(stream)

		expect(h.setResult).not.toHaveBeenCalled()
		expect(renderResponseByteMetrics()).toContain("gaia_api_graphql_response_bytes_count 0")
	})
})

describe("useResponseBudget — shadow-mode safety", () => {
	beforeEach(() => {
		__resetResponseByteMetricsForTests()
	})

	afterEach(() => {
		vi.restoreAllMocks()
	})

	it("does not throw, or refuse, when the measurement throws", () => {
		const h = harness()
		const data = {
			get entities(): unknown {
				throw new Error("hydration exploded")
			},
		}

		expect(() => h.run({data})).not.toThrow()
		expect(h.setResult).not.toHaveBeenCalled()
	})

	it("does not throw when the warning path throws", () => {
		// A circular value in `variables` defeats JSON.stringify inside log.warn.
		// Spied rather than module-mocked on purpose: stubbing log.warn would
		// make this test assert nothing.
		const circular: Record<string, unknown> = {}
		circular.self = circular
		const h = harness(parse(QUERY), {circular})

		expect(() => h.run({data: payloadOfAtLeast(RESPONSE_BYTE_LOG_THRESHOLD + 1000)})).not.toThrow()
	})
})

// --------------------------------------------------------------------------
// Enforcement — module re-imported with the ceiling armed
// --------------------------------------------------------------------------

describe("useResponseBudget — enforcement", () => {
	const CEILING = 1_000_000

	/**
	 * Re-import the module with the ceiling armed. `vi.resetModules()` gives the
	 * fresh copy a fresh `services/telemetry` too, so the spy has to be taken on
	 * *that* instance — spying the top-level import would silently observe
	 * nothing and leave every assertion below passing on zero calls.
	 */
	async function armed() {
		vi.resetModules()
		vi.stubEnv("GRAPHQL_RESPONSE_BYTE_CEILING", String(CEILING))
		const mod = await import("../responseBudgetPlugin")
		const {log: freshLog} = await import("../../services/telemetry")
		const warn = vi.spyOn(freshLog, "warn").mockImplementation(() => {})

		const plugin = mod.useResponseBudget()
		const onExecute = (plugin as {onExecute: (p: unknown) => {onExecuteDone: (p: unknown) => void}}).onExecute
		const setResult = vi.fn()

		return {
			mod,
			warn,
			setResult,
			run(data: unknown) {
				const {onExecuteDone} = onExecute({
					args: {document: parse(QUERY), variableValues: {}, operationName: null, contextValue: {}},
				})
				onExecuteDone({result: {data}, setResult})
			},
		}
	}

	afterEach(() => {
		vi.unstubAllEnvs()
		vi.resetModules()
		vi.restoreAllMocks()
	})

	it("replaces an over-ceiling response with a client error and drops the data", async () => {
		const h = await armed()

		h.run(payloadOfAtLeast(CEILING + 1000))

		expect(h.setResult).toHaveBeenCalledTimes(1)
		const replacement = h.setResult.mock.calls[0]?.[0] as {
			data?: unknown
			errors: {extensions: Record<string, unknown>}[]
		}
		expect(replacement.data).toBeUndefined()
		expect(replacement.errors).toHaveLength(1)
		// BAD_USER_INPUT specifically: errorMasking only unmasks that code and
		// SERVICE_UNAVAILABLE, and instrumentationPlugin.isClientError uses the
		// same set to keep refusals out of Sentry. Any other code silently
		// becomes "Unexpected error." for the caller plus a Sentry issue per
		// refusal.
		expect(replacement.errors[0]?.extensions).toMatchObject({
			code: "BAD_USER_INPUT",
			maxResponseSizeBytes: CEILING,
			http: {status: 400},
		})
	})

	it("delivers a response under the ceiling untouched", async () => {
		const h = await armed()

		h.run({entities: [{id: "a", name: "small"}]})

		expect(h.setResult).not.toHaveBeenCalled()
	})

	it("counts the refusal and flags the measurement as a lower bound", async () => {
		const h = await armed()

		// Many rows rather than one huge string, so the walk stops mid-way and
		// the reported size really is partial.
		h.run({entities: Array.from({length: 5000}, (_, i) => ({id: `e${i}`, name: "x".repeat(1000)}))})

		expect(h.warn).toHaveBeenCalledWith(
			"GraphQL response refused: byte ceiling",
			expect.objectContaining({enforced: true, responseSizeBytesIsLowerBound: true}),
		)
		const metrics = h.mod.renderResponseByteMetrics()
		expect(metrics).toContain("gaia_api_graphql_response_budget_refused_total 1")
		expect(metrics).toContain("gaia_api_graphql_response_budget_exceeded_total 1")
	})
})

// --------------------------------------------------------------------------
// End-to-end through yoga
// --------------------------------------------------------------------------
// The refusal depends on three behaviours of the surrounding stack that unit
// tests over the plugin cannot see:
//   1. `setResult` inside `onExecuteDone` actually replaces the HTTP response;
//   2. yoga reads `extensions.http.status` off an error in the result;
//   3. the api's `maskedErrors` config lets the message through — anything
//      other than BAD_USER_INPUT / SERVICE_UNAVAILABLE becomes "Unexpected
//      error." (see errorMasking.shouldUnmaskError), which would leave the
//      caller a 400 with no idea what to change.
// Uses a hand-rolled schema rather than the PostGraphile one so no database is
// needed; the plugin only ever looks at `result.data`.

describe("useResponseBudget — through yoga", () => {
	const CEILING = 50_000

	async function server() {
		vi.resetModules()
		vi.stubEnv("GRAPHQL_RESPONSE_BYTE_CEILING", String(CEILING))

		const [{useResponseBudget: armedPlugin}, {makeExecutableSchema}, {createYoga, maskError}, {shouldUnmaskError}] =
			await Promise.all([
				import("../responseBudgetPlugin"),
				import("@graphql-tools/schema"),
				import("graphql-yoga"),
				import("../errorMasking"),
			])
		const {log: freshLog} = await import("../../services/telemetry")
		vi.spyOn(freshLog, "warn").mockImplementation(() => {})

		const schema = makeExecutableSchema({
			typeDefs: `type Query { padding(bytes: Int!): String! }`,
			resolvers: {Query: {padding: (_: unknown, {bytes}: {bytes: number}) => "x".repeat(bytes)}},
		})

		return createYoga({
			schema,
			// Same masking policy as the real server (postgraphile.ts).
			maskedErrors: {
				maskError(error, message, isDev) {
					if (shouldUnmaskError(error)) return error
					return maskError(error, message, isDev)
				},
			},
			plugins: [armedPlugin()],
		})
	}

	async function post(yoga: Awaited<ReturnType<typeof server>>, query: string) {
		const response = await yoga.fetch(
			new Request("http://yoga.test/graphql", {
				method: "POST",
				headers: {"content-type": "application/json"},
				body: JSON.stringify({query}),
			}),
		)
		return {status: response.status, body: (await response.json()) as Record<string, unknown>}
	}

	afterEach(() => {
		vi.unstubAllEnvs()
		vi.resetModules()
		vi.restoreAllMocks()
	})

	it("answers 400 with an actionable, unmasked message and no data", async () => {
		const yoga = await server()

		const {status, body} = await post(yoga, `{ padding(bytes: ${CEILING * 2}) }`)

		expect(status).toBe(400)
		expect(body.data).toBeFalsy()
		const errors = body.errors as {message: string; extensions: Record<string, unknown>}[]
		expect(errors).toHaveLength(1)
		expect(errors[0]?.message).toContain("exceeds the maximum")
		expect(errors[0]?.message).not.toContain("Unexpected error")
		expect(errors[0]?.extensions).toMatchObject({code: "BAD_USER_INPUT", maxResponseSizeBytes: CEILING})
	})

	it("answers 200 with data for a response under the ceiling", async () => {
		const yoga = await server()

		const {status, body} = await post(yoga, `{ padding(bytes: 100) }`)

		expect(status).toBe(200)
		expect(body.errors).toBeUndefined()
		expect((body.data as {padding: string}).padding).toHaveLength(100)
	})
})
