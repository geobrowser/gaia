import {parse, print} from "graphql"
import {afterEach, describe, expect, it, vi} from "vitest"
import {graphqlQueryFingerprint} from "../../services/queryFingerprint"
import {log} from "../../services/telemetry"
import {
	ADMISSION_COST_FLOOR,
	getInFlightExpensiveCount,
	MAX_CONCURRENT_EXPENSIVE,
	resetAdmissionControl,
	useAdmissionControl,
} from "../admissionControl"
import {GRAPHQL_QUERY_COST_CONTEXT_KEY} from "../costLoggerPlugin"

type ExecuteHook = (args: {args: unknown}) => {onExecuteDone?: () => void} | undefined
type ResponseHook = (args: {request: Request}) => void

function hook() {
	return (useAdmissionControl() as unknown as {onExecute: ExecuteHook}).onExecute
}

function plugin() {
	return useAdmissionControl() as unknown as {onExecute: ExecuteHook; onResponse: ResponseHook}
}

/** A request context carrying the cost the cost plugin would have stashed. */
function ctx(cost: number | undefined, request?: Request) {
	const base = cost === undefined ? {} : {[GRAPHQL_QUERY_COST_CONTEXT_KEY]: cost}
	return request ? {...base, request} : base
}

function execute(onExecute: ExecuteHook, cost: number | undefined, operationName = "op") {
	return onExecute({args: {contextValue: ctx(cost), operationName}})
}

afterEach(() => {
	resetAdmissionControl()
})

describe("useAdmissionControl", () => {
	it("does not count operations below the cost floor", () => {
		const onExecute = hook()
		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE * 3; i++) {
			expect(() => execute(onExecute, ADMISSION_COST_FLOOR - 1)).not.toThrow()
		}
		expect(getInFlightExpensiveCount()).toBe(0)
	})

	it("lets cheap traffic through while the expensive limit is saturated", () => {
		const onExecute = hook()
		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE; i++) execute(onExecute, ADMISSION_COST_FLOOR)
		expect(getInFlightExpensiveCount()).toBe(MAX_CONCURRENT_EXPENSIVE)

		// The whole point of the cost gate: a burst of light lookups must not be
		// refused because heavy feed queries are in flight.
		expect(() => execute(onExecute, 10)).not.toThrow()
		expect(() => execute(onExecute, ADMISSION_COST_FLOOR)).toThrow(/at capacity/)
	})

	it("rejects with 503 and Retry-After once the limit is reached", () => {
		const onExecute = hook()
		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE; i++) execute(onExecute, 250)

		try {
			execute(onExecute, 250)
			throw new Error("expected a rejection")
		} catch (error) {
			const ext = (error as {extensions?: Record<string, unknown>}).extensions
			expect(ext?.code).toBe("SERVICE_UNAVAILABLE")
			expect(ext?.http).toEqual({status: 503, headers: {"Retry-After": "1"}})
		}
	})

	it("frees a slot when the operation finishes", () => {
		const onExecute = hook()
		const handles = []
		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE; i++) handles.push(execute(onExecute, 250))
		expect(() => execute(onExecute, 250)).toThrow()

		handles[0]?.onExecuteDone?.()
		expect(getInFlightExpensiveCount()).toBe(MAX_CONCURRENT_EXPENSIVE - 1)
		expect(() => execute(onExecute, 250)).not.toThrow()
	})

	it("treats a missing cost as cheap rather than guessing", () => {
		// Introspection, or a cost walk that threw. Refusing on unknown cost
		// would turn a cost-plugin bug into an outage.
		const onExecute = hook()
		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE * 2; i++) {
			expect(() => execute(onExecute, undefined)).not.toThrow()
		}
		expect(getInFlightExpensiveCount()).toBe(0)
	})

	it("self-heals if a release is missed, instead of wedging the pod shut", () => {
		const onExecute = hook()
		// Fill every slot and deliberately never call onExecuteDone — the leak
		// that would otherwise reject all expensive traffic until a restart.
		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE; i++) execute(onExecute, 250)
		expect(() => execute(onExecute, 250)).toThrow()

		// Entries older than MAX_AGE_MS are pruned on the next check.
		const wellPastMaxAge = Date.now() + 120_000
		expect(getInFlightExpensiveCount(wellPastMaxAge)).toBe(0)
		expect(() => execute(onExecute, 250)).not.toThrow()
	})

	it("releases the slot via onResponse when onExecuteDone never fires", () => {
		// The real leak path, and the reason onExecuteDone alone is not enough.
		// envelop runs handleMaybePromise(beforeHooks, thenExecuteAndAfterHooks)
		// with no error handler, so when a LATER plugin's onExecute throws —
		// usePgClient shedding, or a failed pool.connect(), both of which happen
		// exactly during an incident — the after-hooks are skipped entirely.
		// Without the backstop the pod would progressively wedge shut under the
		// conditions the limiter exists to survive.
		const p = plugin()
		const requests: Request[] = []

		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE; i++) {
			const req = new Request(`https://example.test/graphql?i=${i}`)
			requests.push(req)
			// Deliberately discard the returned hooks: simulate onExecuteDone
			// never being invoked.
			p.onExecute({args: {contextValue: ctx(250, req), operationName: "op"}})
		}

		expect(getInFlightExpensiveCount()).toBe(MAX_CONCURRENT_EXPENSIVE)
		expect(() =>
			p.onExecute({args: {contextValue: ctx(250, new Request("https://example.test/x")), operationName: "op"}}),
		).toThrow(/at capacity/)

		for (const req of requests) p.onResponse({request: req})

		expect(getInFlightExpensiveCount()).toBe(0)
		expect(() =>
			p.onExecute({args: {contextValue: ctx(250, new Request("https://example.test/y")), operationName: "op"}}),
		).not.toThrow()
	})

	it("onResponse is harmless for a request that never took a slot", () => {
		const p = plugin()
		expect(() => p.onResponse({request: new Request("https://example.test/none")})).not.toThrow()
		expect(getInFlightExpensiveCount()).toBe(0)
	})
})

/**
 * GEO-2881. A rejection used to log `operationName: "anonymous", cost: 214` and nothing
 * else, which is not enough to act on: most callers send unnamed documents, and a rejected
 * operation never reaches useCostLogger's high-cost warning, so it appears nowhere else.
 * 106 of 131 rejections in a production hour were unidentifiable.
 */
describe("useAdmissionControl rejection is identifiable", () => {
	function saturate(onExecute: ExecuteHook) {
		for (let i = 0; i < MAX_CONCURRENT_EXPENSIVE; i++) execute(onExecute, ADMISSION_COST_FLOOR)
	}

	it("logs a fingerprint matching the one the cost and slow-query logs use", () => {
		const warn = vi.spyOn(log, "warn").mockImplementation(() => {})
		try {
			const onExecute = hook()
			saturate(onExecute)

			const document = parse("query { entities { id } }")
			expect(() =>
				onExecute({args: {contextValue: ctx(ADMISSION_COST_FLOOR), operationName: null, document}}),
			).toThrow()

			const rejection = warn.mock.calls.find(([message]) => String(message).includes("rejected an operation"))
			expect(rejection).toBeDefined()
			const fields = rejection?.[1] as {queryFingerprint?: string | null}
			// Correlatable with the `gql:` ids in the cost and instrumentation logs — the
			// whole point is being able to join a rejection to those records.
			// Derived from the printer rather than a hardcoded string: pinning graphql-js's
			// exact whitespace here would make this fail on a library upgrade for no reason.
			expect(fields.queryFingerprint).toBe(graphqlQueryFingerprint(print(document)))
			expect(fields.queryFingerprint).toMatch(/^gql:[0-9a-f]{8}$/)
		} finally {
			warn.mockRestore()
		}
	})

	it("degrades to null rather than throwing when there is no document", () => {
		const warn = vi.spyOn(log, "warn").mockImplementation(() => {})
		try {
			const onExecute = hook()
			saturate(onExecute)
			// A plugin that threw while refusing a request would turn a clean 503 into a 500,
			// so the fingerprint is never allowed to be the thing that fails.
			expect(() => execute(onExecute, ADMISSION_COST_FLOOR)).toThrow(/at capacity for expensive queries/)
			const rejection = warn.mock.calls.find(([message]) => String(message).includes("rejected an operation"))
			expect((rejection?.[1] as {queryFingerprint?: string | null}).queryFingerprint).toBeNull()
		} finally {
			warn.mockRestore()
		}
	})
})
