import {describe, expect, it} from "vitest"
import type {SaturationSnapshot} from "../../services/dbSaturation"
import {createShedEpisodeTracker} from "../shedEpisodeTracker"

function snapshot(overrides: Partial<SaturationSnapshot> = {}): SaturationSnapshot {
	return {
		isPressured: false,
		isSaturated: false,
		reasons: [],
		utilizationPercent: 0,
		waitingCount: 0,
		recentAcquireTimeouts: 0,
		activeSince: null,
		lastSignalAt: null,
		...overrides,
	}
}

describe("createShedEpisodeTracker", () => {
	it("logs on the first shed of a saturation episode", () => {
		const tracker = createShedEpisodeTracker()

		const first = tracker.shouldLogOnset(snapshot({isSaturated: true, activeSince: "2026-04-17T16:00:00.000Z"}))
		expect(first).toBe(true)
	})

	it("does not log again within the same episode", () => {
		const tracker = createShedEpisodeTracker()
		const sameEpisode = snapshot({isSaturated: true, activeSince: "2026-04-17T16:00:00.000Z"})

		tracker.shouldLogOnset(sameEpisode)
		expect(tracker.shouldLogOnset(sameEpisode)).toBe(false)
		expect(tracker.shouldLogOnset(sameEpisode)).toBe(false)
	})

	it("logs again for a new episode with a different activeSince", () => {
		const tracker = createShedEpisodeTracker()

		tracker.shouldLogOnset(snapshot({isSaturated: true, activeSince: "2026-04-17T16:00:00.000Z"}))

		const nextEpisode = tracker.shouldLogOnset(
			snapshot({isSaturated: true, activeSince: "2026-04-17T16:10:00.000Z"}),
		)
		expect(nextEpisode).toBe(true)
	})

	it("does not log when the snapshot is not saturated", () => {
		const tracker = createShedEpisodeTracker()
		const unsaturated = snapshot({isSaturated: false, activeSince: null})

		expect(tracker.shouldLogOnset(unsaturated)).toBe(false)
	})

	it("re-logs after a release-and-reactivate cycle", () => {
		const tracker = createShedEpisodeTracker()

		tracker.shouldLogOnset(snapshot({isSaturated: true, activeSince: "2026-04-17T16:00:00.000Z"}))
		tracker.shouldLogOnset(snapshot({isSaturated: false}))

		const reactivated = tracker.shouldLogOnset(
			snapshot({isSaturated: true, activeSince: "2026-04-17T16:05:00.000Z"}),
		)
		expect(reactivated).toBe(true)
	})

	it("is defensive against isSaturated without activeSince", () => {
		const tracker = createShedEpisodeTracker()
		expect(tracker.shouldLogOnset(snapshot({isSaturated: true, activeSince: null}))).toBe(false)
	})

	it("isolates state between independent tracker instances", () => {
		const a = createShedEpisodeTracker()
		const b = createShedEpisodeTracker()
		const shared = snapshot({isSaturated: true, activeSince: "2026-04-17T16:00:00.000Z"})

		expect(a.shouldLogOnset(shared)).toBe(true)
		expect(b.shouldLogOnset(shared)).toBe(true)
		expect(a.shouldLogOnset(shared)).toBe(false)
	})
})
