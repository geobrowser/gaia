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
 * 2. Past `executeBy` deadline → REJECTED
 * 3. Fast path: `yesCount >= flatSupportThreshold` → EXECUTABLE
 * 4. Slow-path early execution (before voting ends): when
 *    `yesCount >= ceil(universalPercentageSupportThreshold × totalEditors / RATIO_BASE)`.
 * 5. Slow-path late execution (after voting ends): quorum + the classic
 *    `(RATIO_BASE - partial) × yes > partial × no` ratio.
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

	// Past the on-chain `executeBy` deadline → REJECTED, regardless of vote outcome.
	// Null `executeBy` means no deadline (legacy V1 rows).
	if (proposal.executeBy !== null && nowSeconds > proposal.executeBy) {
		return {
			status: "REJECTED",
			isQuorumReached: false,
			isThresholdReached: false,
			isEarlyExecutable: false,
		}
	}

	const isVotingEnded = nowSeconds > proposal.endTime
	const totalVotes = proposal.yesCount + proposal.noCount + proposal.abstainCount
	const isQuorumReached = totalVotes >= proposal.quorum

	if (proposal.votingMode === "Fast") {
		const isThresholdReached = proposal.yesCount >= proposal.flatSupportThreshold

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

/**
 * Helper to get current time in seconds as bigint.
 * Extracted for easy mocking in tests.
 */
export const getCurrentTimeSeconds = (): bigint => BigInt(Math.floor(Date.now() / 1000))
