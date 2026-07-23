/**
 * Proposal status computation matching the smart contract logic.
 *
 * This module provides a pure function for computing proposal status,
 * matching the contract's `isSupportThresholdReached()` / `canExecuteProposal()`
 * V2 implementation.
 */

import type {ProposalListItem, ProposalWithVotes, StatusComputationResult} from "./types"
import {RATIO_BASE} from "./types"

/**
 * Computes proposal status matching the V2 smart contract logic.
 *
 * This is a PURE function - time is injected to enable deterministic testing.
 *
 * Decision order:
 * 1. Already executed → ACCEPTED
 * 2. Voting window not started yet (`endTime == 0`) → PROPOSED. In V2 the
 *    window (`startDate`/`lastDate`/`executeBy`) stays zero until the first
 *    vote, and the contract's `canExecuteProposal` returns false while
 *    `lastDate == 0`. So a proposal with no votes yet is open, never
 *    executable or rejected on the zero window.
 * 3. Fast path: `yesCount > effective(flatSupportThreshold)` → EXECUTABLE,
 *    where `effective(x) = x == 0 ? 0 : x - 1` (matches the contract).
 * 4. Slow-path early execution (before voting ends): when
 *    `yesCount >= ceil(universalPercentageSupportThreshold × totalEditors / RATIO_BASE)`.
 * 5. Slow-path late execution (after voting ends): quorum + the classic
 *    `(RATIO_BASE - partial) × yes > partial × no` ratio.
 * 6. Past `executeBy` deadline → downgrades an otherwise-still-undecided
 *    (PROPOSED) outcome to REJECTED. Deliberately does NOT override an
 *    outcome that already resolved to EXECUTABLE (or REJECTED by vote) —
 *    `executeBy` means "this open proposal lapsed without a decision," not
 *    "ignore what the votes say." This matters for historical/migrated
 *    proposals, where `executeBy` is synthesized from long-past timestamps
 *    and will always appear expired by the time anyone looks at it; treating
 *    it as an unconditional override would make every migrated proposal that
 *    actually passed its vote display as REJECTED.
 *
 * Must stay byte-for-byte consistent with the SQL fragments in `queries.ts`
 * (`sqlIsExecutable` / `sqlIsProposed` / `sqlIsRejected`). The parity tests
 * in `__tests__/queries.test.ts` validate this — update both sides together.
 *
 * @param proposal - The proposal with aggregated vote counts (using bigint)
 * @param nowSeconds - Current time in seconds (inject for testability)
 * @returns Status computation result with status and intermediate flags
 */
export function computeProposalStatus(
	proposal: ProposalWithVotes | ProposalListItem,
	nowSeconds: bigint,
): StatusComputationResult {
	// Already executed -> ACCEPTED (regardless of votes or deadline)
	if (proposal.executedAt !== null) {
		return {
			status: "ACCEPTED",
			isQuorumReached: true,
			isThresholdReached: true,
			isEarlyExecutable: false,
		}
	}

	const result = computeVoteBasedStatus(proposal, nowSeconds)

	// Past the on-chain `executeBy` deadline: only downgrades a proposal that's
	// still genuinely undecided (PROPOSED) to REJECTED. See the deadline note
	// above for why this must not override an already-resolved outcome.
	if (result.status === "PROPOSED" && proposal.executeBy !== null && nowSeconds > proposal.executeBy) {
		return {
			status: "REJECTED",
			isQuorumReached: result.isQuorumReached,
			isThresholdReached: false,
			isEarlyExecutable: false,
		}
	}

	return result
}

function computeVoteBasedStatus(
	proposal: ProposalWithVotes | ProposalListItem,
	nowSeconds: bigint,
): StatusComputationResult {
	// Voting window not started yet: the V2 contract leaves start/last/executeBy
	// at zero until the first vote, and `canExecuteProposal` returns false while
	// `lastDate == 0`. Treat this as an open proposal — not executable, not ended.
	if (proposal.endTime === 0n) {
		return {
			status: "PROPOSED",
			isQuorumReached: false,
			isThresholdReached: false,
			isEarlyExecutable: false,
		}
	}

	const isVotingEnded = nowSeconds > proposal.endTime
	const totalVotes = proposal.yesCount + proposal.noCount + proposal.abstainCount
	const isQuorumReached = totalVotes >= proposal.quorum

	if (proposal.votingMode === "Fast") {
		const isThresholdReached = proposal.yesCount > effectiveThreshold(proposal.flatSupportThreshold)

		if (isThresholdReached) {
			return {
				status: "EXECUTABLE",
				isQuorumReached,
				isThresholdReached,
				isEarlyExecutable: false,
			}
		}
		return {
			status: isVotingEnded ? "REJECTED" : "PROPOSED",
			isQuorumReached,
			isThresholdReached,
			isEarlyExecutable: false,
		}
	}

	// Slow path — early execution (before voting ends).
	// Skip the check when we can't evaluate it safely: totalEditors = 0 means
	// the space has no indexed editors yet; universal = 0 means no configured
	// early-execution threshold.
	if (!isVotingEnded && proposal.totalEditors > 0n && proposal.universalPercentageSupportThreshold > 0n) {
		const required = ceilDiv(proposal.universalPercentageSupportThreshold * proposal.totalEditors, RATIO_BASE)
		if (proposal.yesCount >= required) {
			return {
				status: "EXECUTABLE",
				isQuorumReached,
				isThresholdReached: true,
				isEarlyExecutable: true,
			}
		}
	}

	// Slow path — late execution (after voting ends).
	const partial = proposal.partialPercentageSupportThreshold
	const isThresholdReached = (RATIO_BASE - partial) * proposal.yesCount > partial * proposal.noCount

	if (!isVotingEnded) {
		return {
			status: "PROPOSED",
			isQuorumReached,
			isThresholdReached,
			isEarlyExecutable: false,
		}
	}

	if (!isQuorumReached) {
		return {
			status: "REJECTED",
			isQuorumReached,
			isThresholdReached: false,
			isEarlyExecutable: false,
		}
	}

	return {
		status: isThresholdReached ? "EXECUTABLE" : "REJECTED",
		isQuorumReached,
		isThresholdReached,
		isEarlyExecutable: false,
	}
}

// Integer ceiling division for bigint. Assumes `divisor > 0n` and `dividend >= 0n`.
function ceilDiv(dividend: bigint, divisor: bigint): bigint {
	return (dividend + divisor - 1n) / divisor
}

// Mirrors the contract's `_computeEffectiveSupportThreshold`: a threshold of 0
// stays 0 (so a single yes vote clears it via strict `>`), otherwise `x - 1`.
function effectiveThreshold(threshold: bigint): bigint {
	return threshold === 0n ? 0n : threshold - 1n
}

/**
 * Helper to get current time in seconds as bigint.
 * Extracted for easy mocking in tests.
 */
export const getCurrentTimeSeconds = (): bigint => BigInt(Math.floor(Date.now() / 1000))
