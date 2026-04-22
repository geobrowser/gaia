/**
 * Tests for proposal status computation.
 *
 * These tests verify that the status computation matches the smart contract's
 * `isSupportThresholdReached()` logic.
 */

import {describe, expect, it} from "vitest"
import {computeProposalStatus} from "../status"
import {type ProposalWithVotes, RATIO_BASE} from "../types"

// =============================================================================
// Test Helpers
// =============================================================================

/**
 * Create a test proposal with sensible defaults.
 *
 * The V2 thresholds default so the legacy `threshold` field semantics still
 * hold for callers that only set `threshold`: Fast-mode proposals mirror
 * `threshold` into `flatSupportThreshold`, and Slow-mode proposals mirror it
 * into `partialPercentageSupportThreshold`. Callers can always override a
 * V2 field directly when a test needs it.
 */
function makeProposal(overrides: Partial<ProposalWithVotes> = {}): ProposalWithVotes {
	const now = BigInt(Math.floor(Date.now() / 1000))
	const votingMode = overrides.votingMode ?? "Fast"
	const threshold = overrides.threshold ?? 10n
	return {
		id: "test-proposal-id",
		spaceId: "test-space-id",
		name: "Test Proposal",
		proposedBy: "test-proposer-id",
		proposalVersion: 1,
		votingMode,
		startTime: now - 3600n, // Started 1 hour ago
		endTime: now + 3600n, // Ends in 1 hour
		quorum: 10n,
		threshold,
		flatSupportThreshold: votingMode === "Fast" ? threshold : 0n,
		partialPercentageSupportThreshold: votingMode === "Slow" ? threshold : 0n,
		universalPercentageSupportThreshold: 0n,
		executeBy: null,
		totalEditors: 0n,
		executedAt: null,
		yesCount: 0n,
		noCount: 0n,
		abstainCount: 0n,
		votes: [],
		actions: [],
		...overrides,
	}
}

// =============================================================================
// Executed Proposals
// =============================================================================

describe("computeProposalStatus - executed proposals", () => {
	it("returns ACCEPTED for executed proposals regardless of votes", () => {
		const proposal = makeProposal({
			executedAt: 1234567890n,
			yesCount: 0n,
			noCount: 100n,
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		expect(result.status).toBe("ACCEPTED")
		expect(result.isQuorumReached).toBe(true)
		expect(result.isThresholdReached).toBe(true)
	})
})

// =============================================================================
// Fast Path Tests
// =============================================================================

describe("computeProposalStatus - fast path", () => {
	it("returns EXECUTABLE when yes votes exceed threshold", () => {
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 10n,
			yesCount: 10n, // Exactly at threshold (10 > 10-1 = 10 > 9 = true)
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("EXECUTABLE")
		expect(result.isThresholdReached).toBe(true)
	})

	it("returns PROPOSED when yes votes are below threshold during voting", () => {
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 10n,
			yesCount: 9n, // Below threshold (9 > 9 = false)
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("PROPOSED")
		expect(result.isThresholdReached).toBe(false)
	})

	it("returns REJECTED when voting ends without reaching threshold", () => {
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 10n,
			yesCount: 5n,
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		expect(result.status).toBe("REJECTED")
		expect(result.isThresholdReached).toBe(false)
	})

	it("handles threshold of 0 (always passes with any yes vote)", () => {
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 0n,
			yesCount: 1n,
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		// With threshold 0, effectiveThreshold = 0, so 1 > 0 = true
		expect(result.status).toBe("EXECUTABLE")
		expect(result.isThresholdReached).toBe(true)
	})

	it("boundary: exactly one below threshold", () => {
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 10n,
			yesCount: 9n, // 9 > 9 = false
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("PROPOSED")
		expect(result.isThresholdReached).toBe(false)
	})

	it("fast path can pass at any time (does not wait for voting to end)", () => {
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 5n,
			yesCount: 5n,
		})

		// Even at the very start of voting
		const result = computeProposalStatus(proposal, proposal.startTime)

		expect(result.status).toBe("EXECUTABLE")
	})
})

// =============================================================================
// Slow Path Tests
// =============================================================================

describe("computeProposalStatus - slow path", () => {
	it("returns PROPOSED during voting even if threshold would be met", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n, // 50%
			quorum: 10n,
			yesCount: 100n,
			noCount: 1n,
		})

		// During voting (before endTime)
		const result = computeProposalStatus(proposal, proposal.endTime - 1n)

		expect(result.status).toBe("PROPOSED")
		// Threshold is still computed for UI display
		expect(result.isThresholdReached).toBe(true)
	})

	it("returns REJECTED when quorum is not reached after voting ends", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: 100n,
			yesCount: 30n,
			noCount: 10n,
			abstainCount: 5n, // Total: 45 < 100 quorum
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		expect(result.status).toBe("REJECTED")
		expect(result.isQuorumReached).toBe(false)
	})

	it("returns EXECUTABLE when quorum and threshold are met after voting ends", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n, // 50%
			quorum: 100n,
			yesCount: 60n,
			noCount: 30n,
			abstainCount: 20n, // Total: 110 >= 100 quorum
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		expect(result.status).toBe("EXECUTABLE")
		expect(result.isQuorumReached).toBe(true)
		expect(result.isThresholdReached).toBe(true)
	})

	it("returns REJECTED when quorum met but threshold not reached", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n, // 50%
			quorum: 100n,
			yesCount: 40n,
			noCount: 50n,
			abstainCount: 20n, // Total: 110 >= 100, but 40 yes vs 50 no
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		expect(result.status).toBe("REJECTED")
		expect(result.isQuorumReached).toBe(true)
		expect(result.isThresholdReached).toBe(false)
	})

	it("exact tie results in REJECTED (threshold not reached)", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n, // 50%
			quorum: 10n,
			yesCount: 50n,
			noCount: 50n,
			abstainCount: 10n,
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		// (RATIO_BASE - threshold) * yes > threshold * no
		// (10M - 5M) * 50 > 5M * 50
		// 250M > 250M = false (tie goes to rejection)
		expect(result.status).toBe("REJECTED")
		expect(result.isThresholdReached).toBe(false)
	})

	it("yes wins by 1 vote", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: 10n,
			yesCount: 51n,
			noCount: 50n,
			abstainCount: 10n,
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		// (10M - 5M) * 51 > 5M * 50
		// 255M > 250M = true
		expect(result.status).toBe("EXECUTABLE")
		expect(result.isThresholdReached).toBe(true)
	})

	it("abstain votes count toward quorum but not threshold", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: 100n,
			yesCount: 10n,
			noCount: 5n,
			abstainCount: 90n, // Quorum met via abstains
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		// Quorum: 10 + 5 + 90 = 105 >= 100 (met)
		// Threshold: (10M - 5M) * 10 > 5M * 5 => 50M > 25M = true
		expect(result.status).toBe("EXECUTABLE")
		expect(result.isQuorumReached).toBe(true)
		expect(result.isThresholdReached).toBe(true)
	})
})

// =============================================================================
// Contract Parity Tests
// =============================================================================

describe("computeProposalStatus - contract parity", () => {
	/**
	 * Test vectors extracted from contract behavior analysis.
	 * These ensure our implementation matches the smart contract exactly.
	 */

	it("matches contract: fast path threshold reached", () => {
		const now = 1707000000n
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 10n,
			quorum: 0n,
			yesCount: 10n, // Contract: yes > threshold - 1 => 10 > 9 => true
			noCount: 5n,
			abstainCount: 0n,
			startTime: now - 3600n,
			endTime: now + 3600n,
			executedAt: null,
		})

		const result = computeProposalStatus(proposal, now)
		expect(result.status).toBe("EXECUTABLE")
	})

	it("matches contract: fast path threshold NOT reached (boundary)", () => {
		const now = 1707000000n
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 10n,
			quorum: 0n,
			yesCount: 9n, // Contract: yes > threshold - 1 => 9 > 9 => false
			noCount: 5n,
			abstainCount: 0n,
			startTime: now - 3600n,
			endTime: now + 3600n,
			executedAt: null,
		})

		const result = computeProposalStatus(proposal, now)
		expect(result.status).toBe("PROPOSED")
	})

	it("matches contract: slow path quorum not reached", () => {
		const now = 1707000000n
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: 100n,
			yesCount: 30n,
			noCount: 10n,
			abstainCount: 5n, // Total: 45 < 100 quorum
			startTime: now - 7200n,
			endTime: now - 3600n, // Ended
			executedAt: null,
		})

		const result = computeProposalStatus(proposal, now)
		expect(result.status).toBe("REJECTED")
	})

	it("matches contract: slow path 50% threshold exact tie", () => {
		const now = 1707000000n
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: 10n,
			yesCount: 50n,
			noCount: 50n, // Exact tie
			abstainCount: 10n,
			startTime: now - 7200n,
			endTime: now - 3600n, // Ended
			executedAt: null,
		})

		const result = computeProposalStatus(proposal, now)
		// Contract: (10M - 5M) * 50 > 5M * 50 => 250M > 250M => false
		expect(result.status).toBe("REJECTED")
	})

	it("matches contract: slow path 50% threshold yes wins by 1", () => {
		const now = 1707000000n
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: 10n,
			yesCount: 51n,
			noCount: 50n,
			abstainCount: 10n,
			startTime: now - 7200n,
			endTime: now - 3600n, // Ended
			executedAt: null,
		})

		const result = computeProposalStatus(proposal, now)
		// Contract: (10M - 5M) * 51 > 5M * 50 => 255M > 250M => true
		expect(result.status).toBe("EXECUTABLE")
	})
})

// =============================================================================
// Edge Cases
// =============================================================================

describe("computeProposalStatus - edge cases", () => {
	it("handles zero votes", () => {
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: 1n,
			yesCount: 0n,
			noCount: 0n,
			abstainCount: 0n,
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		expect(result.status).toBe("REJECTED")
		expect(result.isQuorumReached).toBe(false)
	})

	it("handles very large vote counts (bigint safety)", () => {
		const largeCount = BigInt(Number.MAX_SAFE_INTEGER) + 1n

		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 5_000_000n,
			quorum: largeCount,
			yesCount: largeCount,
			noCount: 1n,
			abstainCount: 0n,
		})

		// Should not overflow - bigint handles this
		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		expect(result.status).toBe("EXECUTABLE")
		expect(result.isQuorumReached).toBe(true)
	})

	it("handles RATIO_BASE correctly in threshold formula", () => {
		// Verify RATIO_BASE is what we expect
		expect(RATIO_BASE).toBe(10_000_000n)

		// With 70% threshold (7_000_000), need yes * 3M > no * 7M
		// So need yes > no * (7/3) ≈ 2.33x the no votes
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 7_000_000n, // 70%
			quorum: 10n,
			yesCount: 70n,
			noCount: 30n,
			abstainCount: 0n,
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		// (10M - 7M) * 70 > 7M * 30
		// 3M * 70 > 7M * 30
		// 210M > 210M = false (exactly at threshold, not passing)
		expect(result.status).toBe("REJECTED")

		// With 71 yes votes
		const proposal2 = makeProposal({
			votingMode: "Slow",
			threshold: 7_000_000n,
			quorum: 10n,
			yesCount: 71n,
			noCount: 30n,
			abstainCount: 0n,
		})

		const result2 = computeProposalStatus(proposal2, proposal2.endTime + 1n)

		// 3M * 71 > 7M * 30
		// 213M > 210M = true
		expect(result2.status).toBe("EXECUTABLE")
	})
})

// =============================================================================
// V2: Fast path reads flatSupportThreshold (not legacy threshold)
// =============================================================================

describe("computeProposalStatus - V2 fast path uses flatSupportThreshold", () => {
	it("fast path executes when yesCount reaches flatSupportThreshold, ignoring legacy threshold", () => {
		// flat=3 is the real V2 threshold; legacy threshold is a stale/bogus value.
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 100n, // legacy (should be ignored by V2 fast path)
			flatSupportThreshold: 3n,
			yesCount: 5n,
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("EXECUTABLE")
		expect(result.isThresholdReached).toBe(true)
		expect(result.isEarlyExecutable).toBe(false)
	})

	it("fast path stays PROPOSED when yesCount is below flatSupportThreshold", () => {
		const proposal = makeProposal({
			votingMode: "Fast",
			threshold: 0n, // would pass under legacy threshold reading
			flatSupportThreshold: 10n,
			yesCount: 5n,
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("PROPOSED")
		expect(result.isEarlyExecutable).toBe(false)
	})
})

// =============================================================================
// V2: Slow-path late execution reads partialPercentageSupportThreshold
// =============================================================================

describe("computeProposalStatus - V2 slow late uses partialPercentageSupportThreshold", () => {
	it("slow late path uses partial threshold (not legacy)", () => {
		// Under legacy threshold=9M (90%), 5 yes vs 3 no would fail.
		// Under V2 partial=5M (50%), 5 yes vs 3 no passes.
		const proposal = makeProposal({
			votingMode: "Slow",
			threshold: 9_000_000n, // 90% — stale/bogus under V2
			partialPercentageSupportThreshold: 5_000_000n, // 50%
			quorum: 5n,
			yesCount: 5n,
			noCount: 3n,
			abstainCount: 0n,
		})

		const result = computeProposalStatus(proposal, proposal.endTime + 1n)

		// (RATIO_BASE - partial) * yes > partial * no
		// (10M - 5M) * 5 > 5M * 3 => 25M > 15M ✓
		expect(result.status).toBe("EXECUTABLE")
		expect(result.isThresholdReached).toBe(true)
		expect(result.isEarlyExecutable).toBe(false)
	})
})

// =============================================================================
// V2: Slow-path early execution (NEW) — universalPercentageSupportThreshold × totalEditors
// =============================================================================

describe("computeProposalStatus - V2 slow early execution", () => {
	it("becomes EXECUTABLE before voting ends when yesCount meets the universal/totalEditors ratio", () => {
		// ceil(75% × 4 editors) = ceil(30M/10M) = 3 yes-votes required.
		const proposal = makeProposal({
			votingMode: "Slow",
			universalPercentageSupportThreshold: 7_500_000n, // 75%
			totalEditors: 4n,
			partialPercentageSupportThreshold: 5_000_000n,
			quorum: 10n,
			yesCount: 4n, // above the ceiling
			noCount: 0n,
			abstainCount: 0n,
		})

		// Voting is still ongoing.
		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("EXECUTABLE")
		expect(result.isEarlyExecutable).toBe(true)
	})

	it("does not early-execute below the universal threshold — stays PROPOSED during voting", () => {
		// Requires 3 yes; only has 2.
		const proposal = makeProposal({
			votingMode: "Slow",
			universalPercentageSupportThreshold: 7_500_000n, // 75%
			totalEditors: 4n,
			partialPercentageSupportThreshold: 5_000_000n,
			quorum: 10n,
			yesCount: 2n,
			noCount: 0n,
			abstainCount: 0n,
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("PROPOSED")
		expect(result.isEarlyExecutable).toBe(false)
	})

	it("rounds the universal × totalEditors ratio up (ceiling)", () => {
		// 33% × 10 editors = 3.3 → ceil = 4 yes-votes required.
		const proposal = makeProposal({
			votingMode: "Slow",
			universalPercentageSupportThreshold: 3_333_333n, // ~33.33%
			totalEditors: 10n,
			partialPercentageSupportThreshold: 5_000_000n,
			quorum: 10n,
			yesCount: 3n, // below ceiling of 4
			noCount: 0n,
			abstainCount: 0n,
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)
		expect(result.status).toBe("PROPOSED")
		expect(result.isEarlyExecutable).toBe(false)

		const proposal2 = makeProposal({
			...proposal,
			yesCount: 4n,
		})
		const result2 = computeProposalStatus(proposal2, proposal2.startTime + 1n)
		expect(result2.status).toBe("EXECUTABLE")
		expect(result2.isEarlyExecutable).toBe(true)
	})

	it("does not apply early execution when totalEditors is 0 (no editors indexed yet)", () => {
		// Without a known editor count, we can't evaluate the formula safely.
		// Falls through to the normal slow-path behavior (must wait for voting end).
		const proposal = makeProposal({
			votingMode: "Slow",
			universalPercentageSupportThreshold: 7_500_000n,
			totalEditors: 0n,
			partialPercentageSupportThreshold: 5_000_000n,
			quorum: 10n,
			yesCount: 1_000_000n, // extremely high
			noCount: 0n,
			abstainCount: 0n,
		})

		const result = computeProposalStatus(proposal, proposal.startTime + 1n)

		expect(result.status).toBe("PROPOSED")
		expect(result.isEarlyExecutable).toBe(false)
	})
})

// =============================================================================
// V2: executeBy deadline
// =============================================================================

describe("computeProposalStatus - V2 executeBy deadline", () => {
	it("REJECTS an otherwise-executable fast proposal past executeBy", () => {
		const now = BigInt(Math.floor(Date.now() / 1000))
		const proposal = makeProposal({
			votingMode: "Fast",
			flatSupportThreshold: 3n,
			yesCount: 100n, // well past threshold
			executeBy: now - 10n, // deadline passed
			endTime: now + 3600n, // voting window still open
		})

		const result = computeProposalStatus(proposal, now)

		expect(result.status).toBe("REJECTED")
	})

	it("REJECTS an otherwise-early-executable slow proposal past executeBy", () => {
		const now = BigInt(Math.floor(Date.now() / 1000))
		const proposal = makeProposal({
			votingMode: "Slow",
			universalPercentageSupportThreshold: 5_000_000n,
			totalEditors: 2n, // ceil(50%×2)=1 yes required
			partialPercentageSupportThreshold: 5_000_000n,
			quorum: 1n,
			yesCount: 2n,
			executeBy: now - 10n,
			endTime: now + 3600n,
		})

		const result = computeProposalStatus(proposal, now)

		expect(result.status).toBe("REJECTED")
		expect(result.isEarlyExecutable).toBe(false)
	})

	it("falls through to normal V1 behavior when executeBy is null (legacy proposals)", () => {
		const now = BigInt(Math.floor(Date.now() / 1000))
		const proposal = makeProposal({
			votingMode: "Fast",
			flatSupportThreshold: 3n,
			yesCount: 5n,
			executeBy: null,
			endTime: now - 10n, // voting ended
		})

		const result = computeProposalStatus(proposal, now)

		expect(result.status).toBe("EXECUTABLE")
	})

	it("does not reject exactly at executeBy (boundary: now == executeBy still allowed)", () => {
		const now = BigInt(Math.floor(Date.now() / 1000))
		const proposal = makeProposal({
			votingMode: "Fast",
			flatSupportThreshold: 3n,
			yesCount: 5n,
			executeBy: now,
			endTime: now + 3600n,
		})

		const result = computeProposalStatus(proposal, now)

		// `now > executeBy` is strictly greater; at the boundary, still executable.
		expect(result.status).toBe("EXECUTABLE")
	})
})

// =============================================================================
// V2: isEarlyExecutable flag is false for all non-slow-early paths
// =============================================================================

describe("computeProposalStatus - isEarlyExecutable default", () => {
	it.each([
		["fast executable", {votingMode: "Fast" as const, flatSupportThreshold: 1n, yesCount: 5n}],
		["fast proposed", {votingMode: "Fast" as const, flatSupportThreshold: 100n, yesCount: 0n}],
	])("returns isEarlyExecutable=false for %s", (_label, overrides) => {
		const proposal = makeProposal(overrides)
		const result = computeProposalStatus(proposal, proposal.startTime + 1n)
		expect(result.isEarlyExecutable).toBe(false)
	})

	it("returns isEarlyExecutable=false for ACCEPTED (already executed) proposals", () => {
		const proposal = makeProposal({executedAt: 1234567890n})
		const result = computeProposalStatus(proposal, proposal.startTime + 1n)
		expect(result.status).toBe("ACCEPTED")
		expect(result.isEarlyExecutable).toBe(false)
	})
})
