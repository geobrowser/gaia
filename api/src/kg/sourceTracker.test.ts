import {describe, expect, it} from "vitest"
import {createSourceTracker} from "./sourceTracker"

const base = {maxSources: 100, burst: 3, refillPerSec: 1, quietMs: 60_000}

describe("createSourceTracker", () => {
	it("logs a source's burst in full, then withholds and counts the rest", () => {
		const t = createSourceTracker(base)
		const decisions = [0, 1, 2, 3, 4].map(() => t.record("a", 1_000))
		expect(decisions.map((d) => d.emit)).toEqual([true, true, true, false, false])
		expect(decisions.map((d) => d.episodeTotal)).toEqual([1, 2, 3, 4, 5])
	})

	it("reports what it withheld on the next line it logs", () => {
		const t = createSourceTracker(base)
		for (let i = 0; i < 5; i++) t.record("a", 1_000)
		const next = t.record("a", 2_000) // one token refilled after 1s
		expect(next.emit).toBe(true)
		expect(next.suppressedSinceLastEmit).toBe(2)
		expect(t.record("a", 2_000).emit).toBe(false)
	})

	it("refills at the sustained rate and never above the burst", () => {
		const t = createSourceTracker(base)
		for (let i = 0; i < 3; i++) t.record("a", 0)
		// 100s idle refills to the burst cap of 3, not 100 — but that gap also starts a new episode.
		const after = [0, 1, 2, 3].map(() => t.record("a", 100_000).emit)
		expect(after).toEqual([true, true, true, false])
	})

	it("keeps sources independent, so one flood cannot silence another source", () => {
		const t = createSourceTracker(base)
		for (let i = 0; i < 50; i++) t.record("flooder", 1_000)
		expect(t.record("bystander", 1_000).emit).toBe(true)
	})

	it("starts a new episode after a quiet gap, and not before", () => {
		const t = createSourceTracker(base)
		expect(t.record("a", 0).isNewEpisode).toBe(true)
		expect(t.record("a", 59_999).isNewEpisode).toBe(false)
		const fresh = t.record("a", 59_999 + 60_000)
		expect(fresh.isNewEpisode).toBe(true)
		expect(fresh.episodeTotal).toBe(1)
	})

	it("stays within maxSources by evicting the least recently seen", () => {
		const t = createSourceTracker({...base, maxSources: 3})
		t.record("a", 0)
		t.record("b", 1)
		t.record("c", 2)
		t.record("a", 3) // touch a, so b is now the oldest
		t.record("d", 4)
		expect(t.size()).toBe(3)
		expect(t.evictions()).toBe(1)
		// b was evicted, so it comes back as a brand-new source with a full burst.
		expect(t.record("b", 5).isNewEpisode).toBe(true)
		// a survived: still the same episode.
		expect(t.record("a", 6).isNewEpisode).toBe(false)
	})

	it("holds memory flat under an address-rotation flood", () => {
		const t = createSourceTracker({...base, maxSources: 1_000})
		for (let i = 0; i < 50_000; i++) t.record(`10.0.${i >> 8}.${i & 255}`, i)
		expect(t.size()).toBe(1_000)
		expect(t.evictions()).toBe(49_000)
	})

	it("caps total lines across sources with the global budget", () => {
		const t = createSourceTracker({...base, global: {burst: 5, refillPerSec: 1}})
		// 20 distinct sources, each with a full personal burst, at the same instant.
		const emitted = Array.from({length: 20}, (_, i) => t.record(`s${i}`, 1_000)).filter((d) => d.emit).length
		expect(emitted).toBe(5)
		expect(t.globallySuppressed()).toBe(15)
		// A globally withheld event is still owed to its source and reported later.
		const later = t.record("s10", 10_000)
		expect(later.emit).toBe(true)
		expect(later.suppressedSinceLastEmit).toBe(1)
	})

	it("rejects nonsensical limits", () => {
		expect(() => createSourceTracker({...base, burst: 0})).toThrow()
		expect(() => createSourceTracker({...base, maxSources: 0})).toThrow()
		expect(() => createSourceTracker({...base, refillPerSec: 0})).toThrow()
	})
})
