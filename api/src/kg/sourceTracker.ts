/**
 * Per-source rate limiting for log output, with bounded memory.
 *
 * WHY THIS EXISTS
 *
 * Security signals are keyed by caller IP, and the callers we care about are hostile. A scanner can
 * send thousands of bad requests a second, and a distributed one can rotate through thousands of
 * addresses. Logging every event would let an attacker turn our log pipeline into the thing they
 * flood, and an unbounded per-IP map would let them grow the heap without limit.
 *
 * So each source gets a token bucket: the first `burst` events are logged in full, after which it
 * is held to `refillPerSec`, and everything suppressed is counted and reported on the next event
 * that does get logged. Prometheus counters still count every event; only the log lines are
 * rationed. The table of sources is an LRU capped at `maxSources`, so memory is bounded no matter
 * how many addresses an attacker controls — eviction only forgets a source's rate state, it never
 * drops a count.
 *
 * It also tracks episodes the same way `errorEpisodeTracker` does: a source that goes quiet for
 * `quietMs` starts a new episode when it returns, which is what decides whether an alert-worthy
 * event is a fresh incident or more of the same one.
 *
 * Pure state container with an injectable clock — no logging or metrics — so it unit-tests without
 * mocks.
 */

export type SourceDecision = {
	/** Log this event. False when the source (or the global budget) is out of tokens. */
	emit: boolean
	/** Events from this source not logged since its last logged one. Meaningful only when `emit`. */
	suppressedSinceLastEmit: number
	/** First event from this source, or the first after a quiet gap of at least `quietMs`. */
	isNewEpisode: boolean
	/** Events from this source in the current episode, including this one. */
	episodeTotal: number
}

export type SourceTrackerOptions = {
	/** Most sources held at once; the least recently seen is evicted beyond this. */
	maxSources: number
	/** Events a source may log back to back before rate limiting starts. */
	burst: number
	/** Sustained events per second a source may log once its burst is spent. */
	refillPerSec: number
	/** A gap this long ends a source's episode. */
	quietMs: number
	/**
	 * Optional ceiling on log lines per second across all sources together, so an attacker rotating
	 * addresses cannot multiply their log budget by the number of addresses they hold.
	 */
	global?: {burst: number; refillPerSec: number}
}

export type SourceTracker = {
	record(source: string, nowMs?: number): SourceDecision
	/** Sources currently held. */
	size(): number
	/** Sources evicted to stay within `maxSources`, since creation. */
	evictions(): number
	/** Events not logged because the global budget was spent, since creation. */
	globallySuppressed(): number
}

type Bucket = {tokens: number; refilledAtMs: number}

type SourceState = Bucket & {
	lastSeenMs: number
	suppressedSinceEmit: number
	episodeTotal: number
}

function refill(bucket: Bucket, capacity: number, perSec: number, nowMs: number): void {
	const elapsedSec = Math.max(0, nowMs - bucket.refilledAtMs) / 1000
	bucket.tokens = Math.min(capacity, bucket.tokens + elapsedSec * perSec)
	bucket.refilledAtMs = nowMs
}

export function createSourceTracker(options: SourceTrackerOptions): SourceTracker {
	const {maxSources, burst, refillPerSec, quietMs, global} = options
	if (maxSources < 1 || burst < 1 || refillPerSec <= 0 || quietMs <= 0) {
		throw new Error("createSourceTracker: maxSources and burst must be >= 1, rates and quietMs > 0")
	}

	// A Map iterates in insertion order, so deleting and re-inserting on every touch keeps the least
	// recently seen source first — an O(1) LRU with no extra bookkeeping.
	const sources = new Map<string, SourceState>()
	const globalBucket: Bucket | null = global ? {tokens: global.burst, refilledAtMs: Number.NEGATIVE_INFINITY} : null
	let evicted = 0
	let globalDrops = 0

	return {
		record(source, nowMs = Date.now()) {
			let state = sources.get(source)
			const isNewEpisode = state === undefined || nowMs - state.lastSeenMs >= quietMs

			if (state === undefined) {
				state = {tokens: burst, refilledAtMs: nowMs, lastSeenMs: nowMs, suppressedSinceEmit: 0, episodeTotal: 0}
			} else {
				sources.delete(source)
				refill(state, burst, refillPerSec, nowMs)
			}
			state.lastSeenMs = nowMs
			state.episodeTotal = isNewEpisode ? 1 : state.episodeTotal + 1
			sources.set(source, state)

			if (sources.size > maxSources) {
				const oldest = sources.keys().next().value
				if (oldest !== undefined) {
					sources.delete(oldest)
					evicted++
				}
			}

			let emit = state.tokens >= 1
			if (emit && globalBucket && global) {
				if (globalBucket.refilledAtMs === Number.NEGATIVE_INFINITY) globalBucket.refilledAtMs = nowMs
				refill(globalBucket, global.burst, global.refillPerSec, nowMs)
				if (globalBucket.tokens >= 1) {
					globalBucket.tokens -= 1
				} else {
					emit = false
					globalDrops++
				}
			}

			if (!emit) {
				state.suppressedSinceEmit++
				return {emit: false, suppressedSinceLastEmit: 0, isNewEpisode, episodeTotal: state.episodeTotal}
			}

			state.tokens -= 1
			const suppressedSinceLastEmit = state.suppressedSinceEmit
			state.suppressedSinceEmit = 0
			return {emit: true, suppressedSinceLastEmit, isNewEpisode, episodeTotal: state.episodeTotal}
		},
		size: () => sources.size,
		evictions: () => evicted,
		globallySuppressed: () => globalDrops,
	}
}
