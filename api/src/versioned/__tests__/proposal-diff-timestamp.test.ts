import {describe, expect, it} from "vitest"
import {editCreatedAtToSeconds} from "../proposal-diff"

// Regression: grouped historical proposal diffs were returning empty pages
// because the conversion from the edit's `createdAt` (microseconds) to the
// seconds-based timestamp expected by PostgreSQL's `to_timestamp()` used the
// wrong divisor (1_000 instead of 1_000_000). That made the base-version
// lookup resolve to ~"now", so the base state already contained the proposal's
// edits and `diffEntitySnapshots` produced empty results for every entity in
// the group.

describe("editCreatedAtToSeconds", () => {
	it("converts exactly one second worth of microseconds to 1 second", () => {
		expect(editCreatedAtToSeconds(1_000_000n)).toBe(1n)
	})

	it("converts zero to zero", () => {
		expect(editCreatedAtToSeconds(0n)).toBe(0n)
	})

	it("converts a realistic 2024-era timestamp (~1.7e15 µs) to ~1.7e9 s", () => {
		// 2024-01-01T00:00:00Z ≈ 1_704_067_200 seconds since epoch
		// = 1_704_067_200_000_000 microseconds
		expect(editCreatedAtToSeconds(1_704_067_200_000_000n)).toBe(1_704_067_200n)
	})

	it("truncates sub-second microsecond fragments (floor division)", () => {
		// 1.5 seconds = 1_500_000 microseconds → 1 second (floor)
		expect(editCreatedAtToSeconds(1_500_000n)).toBe(1n)
	})
})
