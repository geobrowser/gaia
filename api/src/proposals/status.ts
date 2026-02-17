/**
 * Proposal status computation matching the smart contract logic.
 *
 * This module provides a pure function for computing proposal status,
 * matching the contract's `isSupportThresholdReached()` implementation.
 */

import type {ProposalListItem, ProposalWithVotes, StatusComputationResult} from "./types"
import {RATIO_BASE} from "./types"

/**
 * Computes proposal status matching the smart contract's isSupportThresholdReached() logic.
 *
 * This is a PURE function - time is injected to enable deterministic testing.
 *
 * Contract logic:
 * - Fast path: flat threshold (yes > threshold - 1, equivalent to yes >= threshold)
 * - Slow path: percentage threshold with quorum, voting must end first
 *   Formula: (RATIO_BASE - threshold) * yes > threshold * no
 *
 * @param proposal - The proposal with aggregated vote counts (using bigint)
 * @param nowSeconds - Current time in seconds (inject for testability)
 * @returns Status computation result with status and intermediate flags
 */
export function computeProposalStatus(
	proposal: ProposalWithVotes | ProposalListItem,
	nowSeconds: bigint,
): StatusComputationResult {
	// Already executed -> ACCEPTED (regardless of votes)
	if (proposal.executedAt !== null) {
		return {
			status: "ACCEPTED",
			isQuorumReached: true, // Must have been reached to execute
			isThresholdReached: true,
		}
	}

	const isVotingEnded = nowSeconds > proposal.endTime
	const totalVotes = proposal.yesCount + proposal.noCount + proposal.abstainCount

	// Quorum check (applies to both paths, but only enforced in slow path)
	const isQuorumReached = totalVotes >= proposal.quorum

	if (proposal.votingMode === "Fast") {
		// Fast path: flat threshold (absolute yes votes needed)
		// Contract uses `yes > threshold - 1` which is equivalent to `yes >= threshold`
		// We subtract 1 to match the contract's strict inequality
		const fastThreshold = proposal.threshold === 0n ? 0n : proposal.threshold - 1n
		const isThresholdReached = proposal.yesCount > fastThreshold

		if (isThresholdReached) {
			return {status: "EXECUTABLE", isQuorumReached, isThresholdReached}
		}
		return {
			status: isVotingEnded ? "REJECTED" : "PROPOSED",
			isQuorumReached,
			isThresholdReached,
		}
	}

	// Slow path: percentage threshold with quorum check
	// Formula from contract: (RATIO_BASE - threshold) * yes > threshold * no
	// With RATIO_BASE = 10_000_000 and threshold = 5_000_000 (50%):
	// 5_000_000 * yes > 5_000_000 * no -> yes > no
	// Note: A tie (yes == no) results in REJECTED (threshold not reached)
	const threshold = proposal.threshold
	const isThresholdReached = (RATIO_BASE - threshold) * proposal.yesCount > threshold * proposal.noCount

	// Must wait for voting period to end before determining outcome
	if (!isVotingEnded) {
		// During voting, compute threshold for UI display but status is PROPOSED
		return {status: "PROPOSED", isQuorumReached, isThresholdReached}
	}

	// Voting ended - check quorum first
	if (!isQuorumReached) {
		return {status: "REJECTED", isQuorumReached, isThresholdReached: false}
	}

	return {
		status: isThresholdReached ? "EXECUTABLE" : "REJECTED",
		isQuorumReached,
		isThresholdReached,
	}
}

/**
 * Helper to get current time in seconds as bigint.
 * Extracted for easy mocking in tests.
 */
export const getCurrentTimeSeconds = (): bigint => BigInt(Math.floor(Date.now() / 1000))
