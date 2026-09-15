/**
 * Collapses a storm of identical operational errors into one log per episode,
 * without going silent while the problem persists.
 *
 * WHY THIS EXISTS
 *
 * `log.error` routes to `Sentry.captureMessage`, which creates one issue event per
 * call (see services/telemetry.ts). Two handlers on the GraphQL pool fire once per
 * *physical connection event* rather than per request, and behind PgBouncer
 * connections churn constantly — so a single ongoing condition became 28,096 and
 * 13,244 Sentry events in 14 days. Together with the load-shed 503s that gaia #930
 * addressed, that is ~88% of gaia-api's error volume, which exhausted the plan
 * quota and left the whole organisation blind (GEO-2845).
 *
 * WHY NOT JUST DOWNGRADE TO WARN
 *
 * Because a pool error IS a real problem, and the first one is genuinely worth
 * paging on. What carries no information is the thousandth identical one in the
 * same minute. So the onset keeps ERROR — one Sentry issue, alerting intact — and
 * the repeats are counted rather than reported. This is the same trade #930 made
 * for shedding, and the same shape as `shedEpisodeTracker`, which logs an onset per
 * saturation episode while the metric counts every occurrence.
 *
 * WHY A HEARTBEAT
 *
 * Episode-on-quiet alone has a failure mode worth naming: a *permanently* broken
 * pool never goes quiet, so it would log once and then stay silent forever —
 * turning a total outage into less signal than a flapping one. `heartbeatMs` bounds
 * that: an ongoing episode re-logs at most that often, carrying the suppressed count
 * so the magnitude is visible rather than inferred.
 *
 * Pure state container — no logging or metrics side effects — so it unit-tests
 * without mocks, matching `shedEpisodeTracker`.
 */

export type ErrorEpisode = {
	/** Log this occurrence? True on episode onset and on each heartbeat. */
	shouldLog: boolean
	/** Occurrences swallowed since the last logged one. Meaningful only when `shouldLog`. */
	suppressed: number
	/** Occurrences in this episode so far, including this one. */
	total: number
	/** True when this is a new episode rather than a heartbeat of an ongoing one. */
	isOnset: boolean
}

export type ErrorEpisodeTracker = {
	record(key: string, nowMs?: number): ErrorEpisode
}

/** A gap this long with no occurrence ends the episode, so the next one is a fresh onset. */
const DEFAULT_QUIET_MS = 60_000
/** An episode that never goes quiet still re-logs this often, so an outage is never silent. */
const DEFAULT_HEARTBEAT_MS = 300_000

type EpisodeState = {
	lastSeenMs: number
	lastLoggedMs: number
	suppressedSinceLog: number
	total: number
}

export function createErrorEpisodeTracker(options: {quietMs?: number; heartbeatMs?: number} = {}): ErrorEpisodeTracker {
	const quietMs = options.quietMs ?? DEFAULT_QUIET_MS
	const heartbeatMs = options.heartbeatMs ?? DEFAULT_HEARTBEAT_MS
	// Keyed by caller-supplied identity so two different failures cannot mask each
	// other — one storm must not make an unrelated error invisible.
	const episodes = new Map<string, EpisodeState>()

	return {
		record(key, nowMs = Date.now()) {
			const previous = episodes.get(key)

			const isOnset = previous === undefined || nowMs - previous.lastSeenMs >= quietMs
			if (isOnset) {
				// Report what the *previous* episode swallowed before it went quiet, so a
				// burst that ended is still accounted for rather than silently dropped.
				const suppressed = previous?.suppressedSinceLog ?? 0
				episodes.set(key, {lastSeenMs: nowMs, lastLoggedMs: nowMs, suppressedSinceLog: 0, total: 1})
				return {shouldLog: true, suppressed, total: 1, isOnset: true}
			}

			const total = previous.total + 1
			if (nowMs - previous.lastLoggedMs >= heartbeatMs) {
				const suppressed = previous.suppressedSinceLog
				episodes.set(key, {lastSeenMs: nowMs, lastLoggedMs: nowMs, suppressedSinceLog: 0, total})
				return {shouldLog: true, suppressed, total, isOnset: false}
			}

			episodes.set(key, {
				lastSeenMs: nowMs,
				lastLoggedMs: previous.lastLoggedMs,
				suppressedSinceLog: previous.suppressedSinceLog + 1,
				total,
			})
			return {shouldLog: false, suppressed: 0, total, isOnset: false}
		},
	}
}
