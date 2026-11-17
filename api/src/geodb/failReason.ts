/**
 * Why a shadow's geo-db read failed, as a closed set safe to use as a metric label (never free text).
 *
 * - timeout: the read outlasted its deadline (`GeoDbError` reason `timeout`, or a serve timeout).
 * - client_failed: the pipe worker died, refused the line, broke its framing or answered something malformed.
 * - cooling_down: the pool is backing off after a worker that never became ready, so the read was not tried.
 * - other: everything else (an overloaded pool, a bad request, a non-GeoDbError, a failure with no read error behind it).
 *
 * The shadow counters (`gaia_api_geodb_card_shadow_total`, `gaia_api_profile_shadow_total`,
 * `gaia_api_gateway_shadow_total`) keep their `result="failed"` series unchanged; each gains a sibling
 * `*_failed_total{reason}` counter whose reasons sum to it.
 */

import {AsyncLocalStorage} from "node:async_hooks"

export const FAIL_REASONS = ["timeout", "client_failed", "cooling_down", "other"] as const
export type FailReason = (typeof FAIL_REASONS)[number]

/** The message of the `client_failed` a pool throws while it backs off after a worker that never started. */
export const BACKING_OFF_MESSAGE = "geodb-client is not starting; backing off"

export function failReason(error: unknown): FailReason {
	// Duck-typed so this file does not import the client (which imports it).
	if (!(error instanceof Error) || error.name !== "GeoDbError") return "other"
	const reason = (error as Error & {reason?: unknown}).reason
	if (reason === "timeout") return "timeout"
	if (reason === "client_failed") return error.message === BACKING_OFF_MESSAGE ? "cooling_down" : "client_failed"
	return "other"
}

const scope = new AsyncLocalStorage<{reason?: FailReason}>()

/**
 * Runs one shadow so the pool's read failures inside it can be noted without every call site carrying the error
 * to its `failed` count. The first failure noted wins.
 */
export function withFailureScope<T>(run: () => T): T {
	return scope.run({}, run)
}

/** Called by the pool when a read rejects; a no-op outside a `withFailureScope`. */
export function noteReadFailure(error: unknown): void {
	const store = scope.getStore()
	if (store && store.reason === undefined) store.reason = failReason(error)
}

/** The reason noted in the running shadow, `other` when no pool read failed (or there is no scope). */
export function notedFailReason(): FailReason {
	return scope.getStore()?.reason ?? "other"
}
