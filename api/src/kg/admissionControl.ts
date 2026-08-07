import {GraphQLError} from "graphql"
import type {Plugin} from "graphql-yoga"
import {log} from "../services/telemetry"
import {GRAPHQL_QUERY_COST_CONTEXT_KEY, type GraphqlCostContext} from "./costLoggerPlugin"

/**
 * Bounds how many *expensive* GraphQL operations one pod executes at once.
 *
 * Why this exists
 * ---------------
 * On 2026-08-06 the api spent hours at 8.5% 5xx and a 17.8s p99. Nothing was
 * wrong with any individual query: the heaviest caller scored 228 on the cost
 * model, and 44% of all normal traffic already scores above 200. Postgres was
 * idle (33 of 1000 connections). The pod's own pool had 19 of 33 connections
 * free with nothing queued.
 *
 * What was exhausted was the single JS thread. Enough ordinary-cost queries
 * arrived together that serialising their results starved the event loop, and
 * everything — including /health/liveness — stopped being answered on time.
 *
 * Nothing in the stack bounded that. `dbSaturation` measures *connections*,
 * which is why it read healthy throughout. The cost model measures a single
 * query's worst case, which cannot see aggregate load. Concurrency is the
 * missing axis.
 *
 * Design notes
 * ------------
 * - **Cost-gated.** Cheap operations are never blocked, however many arrive.
 *   Only work above ADMISSION_COST_FLOOR counts against the limit, so a
 *   burst of `entity(id:)` lookups cannot be refused because a few heavy
 *   feed queries are in flight. This keeps the failure contained to the
 *   traffic that actually causes it.
 *
 * - **Fast-fail, not queue.** Queuing converts overload into latency, which
 *   is what already happens and what killed the liveness probe. A 503 with
 *   Retry-After costs microseconds and lets the caller decide. Note the one
 *   known heavy client (news) retries 503s, so the ceiling must be high
 *   enough that this is genuinely rare.
 *
 * - **Leak-resistant.** A counter that fails to decrement would wedge the pod
 *   permanently closed — strictly worse than the problem. In-flight work is
 *   tracked with start timestamps and anything older than MAX_AGE_MS is
 *   pruned, so a missed release self-heals within a minute instead of
 *   requiring a restart.
 *
 *   envelop's onExecuteDone alone is NOT sufficient: the orchestrator runs
 *   handleMaybePromise(beforeHooks, thenExecuteAndAfterHooks) with no error
 *   handler, so if a *later* plugin's onExecute throws — usePgClient
 *   pool-shedding or a failed pool.connect(), both incident conditions — the
 *   success continuation is skipped and the after-hooks never run. At the
 *   measured 0.887 expensive ops/sec/pod a 60s prune window would leak ~53
 *   slots, more than the limit itself. So there is also an onResponse
 *   backstop keyed by the HTTP Request, mirroring usePgClient which hit the
 *   same behaviour (api/docs/gql-pool-leak-investigation.md).
 *
 * - **A ceiling, not a tuning knob.** The default is set from measurement,
 *   not intuition. Over 30 minutes of healthy traffic, reconstructing request
 *   intervals from logged durations gives a peak of **5** concurrent
 *   slow/large operations on the busiest pod. That is a lower bound — only
 *   slow and large operations are logged — but the fast remainder is short
 *   enough to add little. Arrival rate is 0.887 expensive (cache-miss)
 *   operations per second per pod, bursting to 30 in a single pod-second.
 *
 *   48 is ~10x the observed peak: high enough that an ordinary burst cannot
 *   trip it, low enough to stop a runaway pile-up. An earlier draft used 16,
 *   which is only ~3x — too tight for a control that answers 503 to a client
 *   that retries them, since a false rejection would add load rather than
 *   shed it. Every engagement is logged; if the warning appears at all in
 *   normal operation the number is still wrong.
 */

function parseIntEnv(name: string, fallback: number, min: number): number {
	const raw = process.env[name]
	if (raw === undefined) return fallback
	const parsed = Number.parseInt(raw, 10)
	if (!Number.isFinite(parsed) || parsed < min) return fallback
	return parsed
}

/** Max concurrent expensive operations per pod. 0 disables admission control. */
export const MAX_CONCURRENT_EXPENSIVE = parseIntEnv("GRAPHQL_MAX_CONCURRENT_EXPENSIVE", 48, 0)

/** Only operations at or above this cost count against the limit. */
export const ADMISSION_COST_FLOOR = parseIntEnv("GRAPHQL_ADMISSION_COST_FLOOR", 200, 0)

/**
 * Safety valve. Longer than PG_QUERY_TIMEOUT_MS (35s default) so a legitimately
 * slow query is never pruned while still running — pruning early would
 * under-count and defeat the limit, which is the milder of the two failures.
 */
const MAX_AGE_MS = parseIntEnv("GRAPHQL_ADMISSION_MAX_AGE_MS", 60_000, 1_000)

type InFlight = {startedAtMs: number; operation: string}

const inFlight = new Map<symbol, InFlight>()

/**
 * Token per in-flight HTTP request, so the onResponse backstop can release a
 * slot whose onExecuteDone never ran. Weak, so an abandoned request cannot
 * retain memory.
 */
const requestTokens = new WeakMap<Request, symbol>()

/** Drop entries whose release was missed, so a leak cannot wedge the pod. */
function pruneStale(nowMs: number): void {
	if (inFlight.size === 0) return
	for (const [token, entry] of inFlight) {
		if (nowMs - entry.startedAtMs > MAX_AGE_MS) {
			inFlight.delete(token)
			log.warn("Admission control pruned a stale in-flight entry", {
				operation: entry.operation,
				ageMs: nowMs - entry.startedAtMs,
			})
		}
	}
}

/** Current count of tracked expensive operations. Exported for tests + metrics. */
export function getInFlightExpensiveCount(nowMs = Date.now()): number {
	pruneStale(nowMs)
	return inFlight.size
}

/** Test-only: drop all tracked state. */
export function resetAdmissionControl(): void {
	inFlight.clear()
}

function operationLabel(args: {operationName?: string | null}): string {
	return args.operationName ?? "anonymous"
}

export function useAdmissionControl(): Plugin {
	return {
		onExecute({args}) {
			if (MAX_CONCURRENT_EXPENSIVE <= 0) return

			// Cost is computed by useCostLogger, which must run first — it
			// stashes the score on the shared request context. If it is absent
			// (introspection, or the cost walk failed) treat the operation as
			// cheap and let it through rather than guessing.
			const ctx = args.contextValue as GraphqlCostContext | undefined
			const cost = ctx?.[GRAPHQL_QUERY_COST_CONTEXT_KEY]
			if (typeof cost !== "number" || cost < ADMISSION_COST_FLOOR) return

			const nowMs = Date.now()
			pruneStale(nowMs)

			if (inFlight.size >= MAX_CONCURRENT_EXPENSIVE) {
				log.warn("GraphQL admission control rejected an operation", {
					operationName: operationLabel(args),
					cost,
					inFlight: inFlight.size,
					limit: MAX_CONCURRENT_EXPENSIVE,
					costFloor: ADMISSION_COST_FLOOR,
				})
				throw new GraphQLError("Server is at capacity for expensive queries; please retry.", {
					extensions: {
						code: "SERVICE_UNAVAILABLE",
						http: {status: 503, headers: {"Retry-After": "1"}},
					},
				})
			}

			const token = Symbol("admission")
			inFlight.set(token, {startedAtMs: nowMs, operation: operationLabel(args)})

			// Key the token to the HTTP request so onResponse can clean up when the
			// after-hooks are skipped. usePgClient keys on the same value.
			const request = (args.contextValue as {request?: Request} | undefined)?.request
			if (request) requestTokens.set(request, token)

			return {
				onExecuteDone() {
					inFlight.delete(token)
					if (request) requestTokens.delete(request)
				},
			}
		},

		// Backstop. Fires after the HTTP response regardless of whether execute()
		// completed, threw, or never ran because a later onExecute hook threw. A
		// token still present here means onExecuteDone did not fire.
		onResponse({request}) {
			const token = requestTokens.get(request)
			if (token) {
				inFlight.delete(token)
				requestTokens.delete(request)
			}
		},
	}
}
