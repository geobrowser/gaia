import {describe, expect, it} from "vitest"
import {createErrorEpisodeTracker} from "../errorEpisodeTracker"

const QUIET = 1_000
const HEARTBEAT = 10_000

function tracker() {
	return createErrorEpisodeTracker({quietMs: QUIET, heartbeatMs: HEARTBEAT})
}

describe("createErrorEpisodeTracker", () => {
	it("logs the first occurrence", () => {
		const t = tracker()
		const first = t.record("pool_error", 0)
		expect(first.shouldLog).toBe(true)
		expect(first.isOnset).toBe(true)
		expect(first.total).toBe(1)
	})

	it("swallows the storm behind it", () => {
		const t = tracker()
		t.record("pool_error", 0)
		// 500 more inside the quiet window — this is the 28,096-event case from GEO-2845,
		// where one ongoing condition produced one Sentry issue event per connection.
		for (let i = 1; i <= 500; i++) {
			expect(t.record("pool_error", i).shouldLog).toBe(false)
		}
	})

	it("counts what it swallowed and reports it on the next log", () => {
		const t = tracker()
		t.record("pool_error", 0)
		for (let i = 1; i <= 9; i++) t.record("pool_error", i)

		// Quiet gap ends the episode; the next occurrence is a new onset and carries the
		// count, so the magnitude is visible rather than inferred from a metric.
		const next = t.record("pool_error", 9 + QUIET)
		expect(next.shouldLog).toBe(true)
		expect(next.isOnset).toBe(true)
		expect(next.suppressed).toBe(9)
	})

	/**
	 * The failure mode worth naming: episode-on-quiet ALONE means a permanently broken
	 * pool never goes quiet, logs once, and is then silent forever — a total outage
	 * producing less signal than a flapping one.
	 */
	it("keeps logging during an episode that never goes quiet", () => {
		const t = tracker()
		t.record("pool_error", 0)

		// Continuous failures, one every 100ms — never a quiet gap.
		let logged = 0
		let lastSuppressed = 0
		for (let ms = 100; ms <= HEARTBEAT * 3; ms += 100) {
			const r = t.record("pool_error", ms)
			if (r.shouldLog) {
				logged++
				lastSuppressed = r.suppressed
				expect(r.isOnset).toBe(false) // a heartbeat, not a new episode
			}
		}

		expect(logged).toBe(3) // one per heartbeat window, not one per occurrence
		expect(lastSuppressed).toBeGreaterThan(90) // and it says how many it stood in for
	})

	it("never lets one storm hide an unrelated error", () => {
		const t = tracker()
		t.record("pool_error", 0)
		for (let i = 1; i <= 100; i++) t.record("pool_error", i)

		// A different failure during the storm must still be reported: keying per error
		// identity is what keeps a noisy one from masking a new, real one.
		const other = t.record("statement_timeout_failed", 50)
		expect(other.shouldLog).toBe(true)
		expect(other.isOnset).toBe(true)
	})

	it("counts every occurrence even while suppressing", () => {
		const t = tracker()
		t.record("pool_error", 0)
		for (let i = 1; i <= 5; i++) t.record("pool_error", i)
		// `total` is the true rate; suppression must never lose it.
		expect(t.record("pool_error", 6).total).toBe(7)
	})
})
