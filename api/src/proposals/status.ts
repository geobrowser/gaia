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
 * 0. Verified permanently unexecutable (`unexecutableAt`) → REJECTED, checked
 *    right after execution and before any vote arithmetic. The migration left
 *    proposals in this database that were never created on chain; their stored
 *    votes resolve to EXECUTABLE, so without this they advertise a "Pending
 *    execution" that reverts `CanNotExecute()` forever.
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
 * 6. Past the `executeBy` deadline (`executeBy > 0 && now > executeBy`) →
 *    REJECTED for anything not already ACCEPTED, including an outcome the
 *    votes resolved to EXECUTABLE. The contract's `canExecuteProposal` checks
 *    `block.timestamp > executeBy` and returns false from then on, so a passed
 *    but unexecuted proposal can never be executed, and reporting it as
 *    EXECUTABLE (with `canExecute: true`) promises an action that always
 *    reverts. A zero or null `executeBy` means no window has been set yet (V2
 *    leaves it zero until the first vote) and never closes anything.
 *
 *    This used to spare EXECUTABLE outcomes, because migrated proposals carry
 *    a synthesized, long-expired `executeBy` and 58% of them would have read
 *    REJECTED. That population now reads ACCEPTED (the kg-indexer infers the
 *    fast-path `executed_at`) or REJECTED (`unexecutableAt`): on 2026-09-30
 *    only 2 proposals across all 1,425 spaces reported EXECUTABLE, and both
 *    were past `executeBy`. The exemption was only keeping dead proposals
 *    looking executable (GEO-2609).
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

	// Verified permanently unexecutable -> REJECTED. Checked before any vote
	// arithmetic because the votes are irrelevant: the DAO has no record of this
	// proposal, so nothing can ever apply its outcome. Ranks below `executedAt` so
	// a proposal that somehow both executed and got flagged still reads ACCEPTED —
	// execution is the stronger fact.
	if (proposal.unexecutableAt !== null) {
		return {
			status: "REJECTED",
			isQuorumReached: false,
			isThresholdReached: false,
			isEarlyExecutable: false,
		}
	}

	const result = computeVoteBasedStatus(proposal, nowSeconds)

	// Past the on-chain `executeBy` deadline nothing can execute any more, so
	// both a still-undecided and a passed-but-unexecuted proposal lapse to
	// REJECTED. The quorum/threshold flags keep describing the votes; only the
	// status (and so `canExecute`) reflects the closed window.
	if (result.status !== "REJECTED" && isExecutionWindowClosed(proposal.executeBy, nowSeconds)) {
		return {
			status: "REJECTED",
			isQuorumReached: result.isQuorumReached,
			isThresholdReached: result.status === "EXECUTABLE" ? result.isThresholdReached : false,
			isEarlyExecutable: false,
		}
	}

	return result
}

/**
 * True once the contract's execution window has closed: it mirrors
 * `canExecuteProposal`'s `block.timestamp > executeBy`. A null or zero
 * `executeBy` means the window has not been set yet (V2 leaves it at zero
 * until the first vote), so it is never closed. Must match
 * `sqlExecutionWindowClosed` in `queries.ts`.
 */
export function isExecutionWindowClosed(executeBy: bigint | null, nowSeconds: bigint): boolean {
	return executeBy !== null && executeBy > 0n && nowSeconds > executeBy
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
