import {afterEach, describe, expect, it} from "vitest"
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
