/**
 * A token bucket for the shadow's geo-db reads. The wide comparisons read one card per space an entity lives
 * in, so a burst of them could load geo-db (and the api's pipe pool) well beyond the sampled request rate. The
 * budget caps the sustained rate; a comparison that finds it empty is not made and counts as `throttled`.
 */

export type ReadBudget = {
	/** Takes `n` reads from the bucket, all or none. True when they were available. */
	tryAcquire: (n: number) => boolean
}

export type ReadBudgetOptions = {
	/** Sustained reads per second. */
	perSecond: number
	/** Most reads that may be spent at once (the bucket's size). */
	burst: number
	/** Milliseconds, monotonic. Tests pass their own. */
	now?: () => number
}

export function createReadBudget({perSecond, burst, now = () => performance.now()}: ReadBudgetOptions): ReadBudget {
	let tokens = burst
	let last = now()
	return {
		tryAcquire(n) {
			const at = now()
			tokens = Math.min(burst, tokens + ((at - last) / 1000) * perSecond)
			last = at
			if (n > tokens) return false
			tokens -= n
			return true
		},
	}
}
