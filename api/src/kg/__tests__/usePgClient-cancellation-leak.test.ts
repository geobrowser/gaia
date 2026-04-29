/**
 * Regression test for the GraphQL pool connection-leak fix described in
 * api/docs/gql-pool-leak-investigation.md (fix #1).
 *
 * Background — the bug:
 *   When a GraphQL request is cancelled mid-execution (HTTP client closes the
 *   socket -> request AbortSignal fires -> `useExecutionCancellation` aborts
 *   `execute()`), the cleanup callback `onExecuteDone` is not reliably
 *   invoked. Pre-fix, `usePgClient` only released the pgClient inside
 *   `onExecuteDone`, so cancelled requests permanently leaked one pool slot.
 *
 * The fix (mirrored here in the inlined plugin):
 *   Track each checked-out pgClient against its HTTP Request via a WeakMap,
 *   and release it from Yoga's `onResponse` hook as a belt-and-braces
 *   cleanup. `onResponse` fires for every request regardless of how
 *   execution settled, so the abort path now reliably releases the client.
 *
 * Test strategy:
 *   - Tracking pool: counts checkouts and releases. Hands out fake clients
 *     whose `query()` blocks on a deferred so the test can pin a query
 *     "in flight" indefinitely.
 *   - The plugin under test mirrors `usePgClient` from
 *     `api/src/kg/postgraphile.ts` (minus production-only logging and Sentry).
 *     Inlined (not imported) because the production module has heavy
 *     module-load side effects (creates a real pg pool, builds the
 *     PostGraphile schema). What we're testing is the *contract* — checkout
 *     in `onExecute`, release in BOTH `onExecuteDone` (happy path) AND
 *     `onResponse` (abort path).
 *   - Yoga server wires `usePgClient` + `useExecutionCancellation` in the same
 *     order as production (`postgraphile.ts:370-378`).
 *
 * Tests:
 *   - "happy path": sanity-checks the harness. Release fires on normal
 *     completion.
 *   - "regression: client aborts mid-query": the load-bearing assertion.
 *     If this test ever fails with `checkedOutCount > 0`, the leak has
 *     regressed — re-read the investigation doc and check the
 *     `onResponse` cleanup path in `usePgClient`.
 *
 * Uses bun:test (not vitest) to mirror cache-integration.test.ts and avoid
 * duplicate graphql module conflicts between graphql-yoga and our schema deps.
 */
import {describe, expect, it} from "bun:test"
import {GraphQLObjectType, GraphQLSchema, GraphQLString} from "graphql"
import {createYoga, type Plugin, useExecutionCancellation} from "graphql-yoga"

// --- Fake pg pool / client --------------------------------------------------

type FakePoolClient = {
	id: number
	query: (sql: string) => Promise<{rows: unknown[]}>
	release: (err?: unknown) => void
}

type Deferred = {promise: Promise<{rows: unknown[]}>; resolve: () => void; reject: (e: unknown) => void}

function createDeferred(): Deferred {
	let resolve!: () => void
	let reject!: (e: unknown) => void
	const promise = new Promise<{rows: unknown[]}>((res, rej) => {
		resolve = () => res({rows: [{ok: true}]})
		reject = rej
	})
	return {promise, resolve, reject}
}

function createTrackingPool() {
	let nextId = 0
	let connectCount = 0
	let releaseCleanCount = 0
	let releaseDestroyCount = 0
	const checkedOut = new Set<number>()

	// Each client's pending query exposes its deferred so the test can resolve
	// or reject it deterministically. Also, a "queryStarted" gate lets the test
	// await the moment the resolver actually invokes pgClient.query() — so we
	// never abort before checkout has happened.
	const pendingQueries: Deferred[] = []
	let queryStartedResolve: (() => void) | null = null
	let queryStarted: Promise<void> = new Promise((r) => {
		queryStartedResolve = r
	})

	function resetQueryStartGate() {
		queryStarted = new Promise((r) => {
			queryStartedResolve = r
		})
	}

	return {
		get connectCount() {
			return connectCount
		},
		get releaseCount() {
			return releaseCleanCount + releaseDestroyCount
		},
		get releaseCleanCount() {
			return releaseCleanCount
		},
		get releaseDestroyCount() {
			return releaseDestroyCount
		},
		get checkedOutCount() {
			return checkedOut.size
		},
		waitForQueryStart() {
			return queryStarted
		},
		resolvePendingQueries() {
			while (pendingQueries.length > 0) {
				pendingQueries.shift()?.resolve()
			}
		},
		async connect(): Promise<FakePoolClient> {
			connectCount++
			const id = nextId++
			checkedOut.add(id)

			const client: FakePoolClient = {
				id,
				query(_sql: string) {
					const deferred = createDeferred()
					pendingQueries.push(deferred)
					queryStartedResolve?.()
					resetQueryStartGate()
					return deferred.promise
				},
				release(err?: unknown) {
					checkedOut.delete(id)
					if (err !== undefined) releaseDestroyCount++
					else releaseCleanCount++
				},
			}
			return client
		},
	}
}

// --- Plugin under test -------------------------------------------------------
//
// Mirrors `usePgClient` from api/src/kg/postgraphile.ts (minus production-only
// logging and Sentry instrumentation). The shape of onExecute / onExecuteDone /
// onResponse is identical to production — that's the contract we're validating.
//
// As of fix #1 from the investigation doc, this plugin tracks the checked-out
// pgClient against the HTTP Request via a WeakMap and releases it from the
// `onResponse` hook as a belt-and-braces cleanup. With this in place, the
// "LEAK" test below should fail (because the leak no longer occurs), which is
// the desired signal that the fix actually fixed the bug.
function usePgClient(pool: ReturnType<typeof createTrackingPool>): Plugin<{pgClient: FakePoolClient}> {
	const requestPgClients = new WeakMap<Request, FakePoolClient>()

	return {
		async onExecute({extendContext, args}) {
			const pgClient = await pool.connect()
			extendContext({pgClient})

			const request = (args.contextValue as {request?: Request})?.request
			if (request) {
				requestPgClients.set(request, pgClient)
			}

			return {
				onExecuteDone({result}) {
					const errors = "errors" in result ? result.errors : undefined
					if (errors?.length) {
						pgClient.release(errors[0])
					} else {
						pgClient.release()
					}
					if (request) requestPgClients.delete(request)
				},
			}
		},

		onResponse({request}) {
			const pgClient = requestPgClients.get(request)
			if (pgClient) {
				pgClient.release(new Error("graphql request ended without normal cleanup"))
				requestPgClients.delete(request)
			}
		},
	}
}

// --- Test schema -------------------------------------------------------------
//
// One field. Resolver awaits pgClient.query(...) which only resolves when the
// test calls pool.resolvePendingQueries(). This pins the request "in flight"
// so the test can abort it during execution.
function createSchema() {
	return new GraphQLSchema({
		query: new GraphQLObjectType({
			name: "Query",
			fields: {
				slow: {
					type: GraphQLString,
					async resolve(_root, _args, ctx: {pgClient: FakePoolClient}) {
						await ctx.pgClient.query("SELECT pg_sleep(60)")
						return "done"
					},
				},
			},
		}),
	})
}

// --- Tests -------------------------------------------------------------------

describe("usePgClient cleanup contract under cancellation", () => {
	it("happy path: completes normally and releases the pgClient", async () => {
		const pool = createTrackingPool()
		const yoga = createYoga({
			schema: createSchema(),
			plugins: [usePgClient(pool), useExecutionCancellation()],
		})

		const fetchPromise = yoga.fetch("http://localhost/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query: "{ slow }"}),
		})

		// Wait until the resolver actually starts a query, then resolve it so
		// execute() completes naturally.
		await pool.waitForQueryStart()
		pool.resolvePendingQueries()

		const response = await fetchPromise
		expect(response.status).toBe(200)
		const body = (await response.json()) as {data: {slow: string}}
		expect(body.data.slow).toBe("done")

		// Sanity check the harness: one connect, one release, zero leaked.
		expect(pool.connectCount).toBe(1)
		expect(pool.releaseCount).toBe(1)
		expect(pool.checkedOutCount).toBe(0)
	})

	it("regression: client aborts mid-query — pgClient is still released (no leak)", async () => {
		const pool = createTrackingPool()
		const yoga = createYoga({
			schema: createSchema(),
			plugins: [usePgClient(pool), useExecutionCancellation()],
		})

		const controller = new AbortController()

		// Fire the request with the abortable signal. We do NOT await it yet —
		// we need to abort *during* execution.
		const fetchPromise = yoga
			.fetch("http://localhost/graphql", {
				method: "POST",
				headers: {"content-type": "application/json"},
				body: JSON.stringify({query: "{ slow }"}),
				signal: controller.signal,
			})
			.catch((err) => err) // swallow the AbortError so the test can proceed

		// Wait for the resolver to actually call pgClient.query(). Until this
		// resolves, the request hasn't reached the leak window.
		await pool.waitForQueryStart()

		// At this point: pool has one client checked out, no releases yet.
		expect(pool.connectCount).toBe(1)
		expect(pool.checkedOutCount).toBe(1)
		expect(pool.releaseCount).toBe(0)

		// Now abort the HTTP request. This is the production scenario: client
		// closed the socket, request signal aborts, useExecutionCancellation
		// cancels execute().
		controller.abort()

		// Give yoga generous time to run any cleanup. If onExecuteDone fires on
		// abort, release() will be called inside this window. If it doesn't,
		// nothing further changes — the client is leaked.
		await new Promise((r) => setTimeout(r, 200))

		// Drain any still-pending queries (simulates Postgres' statement_timeout
		// firing later). This proves the leak isn't just "release happens
		// after the query settles" — it never fires, period.
		pool.resolvePendingQueries()
		await new Promise((r) => setTimeout(r, 200))

		// Wait for the fetch to settle so the test exits cleanly.
		await fetchPromise

		// Post-fix: the WeakMap+onResponse cleanup releases the pgClient even
		// when execute() was aborted. The connection should be back in the pool
		// and exactly one release call should have happened (either via
		// onExecuteDone with errors, or via the onResponse fallback in destroy
		// mode — depending on Yoga's internal abort handling, but exactly one).
		// If this test ever fails again with checkedOutCount > 0, the leak has
		// regressed.
		expect(pool.connectCount).toBe(1)
		expect(pool.checkedOutCount).toBe(0)
		expect(pool.releaseCount).toBe(1)
	})
})
