/**
 * Proposal status over geo-db records: the read-time function the PROPOSAL-STATUS record note names as the home of
 * status (feeder proposalStatusKeyed.ts). gaia stores no status; it is a pure function of stored inputs and the
 * clock, so a geo-db read is the same function fed from records instead of Postgres columns.
 *
 * `computeProposalStatus` stays the one definition. This builds its input from the three things geo-db holds today
 * (the status record's executed_at and unexecutable_at, and a tally of the proposal's vote records) and takes the
 * rest, the settings snapshot a proposal carries and its editor count, from `snapshot`. The snapshot is per proposal
 * version (quorum, thresholds, window and execute-by are copied into the version when it is created), so the shadow
 * reads it from the proposal's latest proposalVersion record (shadow.ts); only the editor count still comes from Postgres.
 * The Space's current `space_voting_settings` is NOT an input: a proposal keeps the settings it was created under.
 */
import {computeProposalStatus} from "../status"
import type {ProposalWithVotes, StatusComputationResult} from "../types"

/** What a proposal version fixes at creation, plus its Space's editor count. */
export type StatusSnapshot = Pick<
	ProposalWithVotes,
	| "votingMode"
	| "startTime"
	| "endTime"
	| "quorum"
	| "threshold"
	| "flatSupportThreshold"
	| "partialPercentageSupportThreshold"
	| "universalPercentageSupportThreshold"
	| "executeBy"
	| "totalEditors"
>

export type StatusRecords = {
	executedAt: bigint | null
	unexecutableAt: bigint | null
	yes: bigint
	no: bigint
	abstain: bigint
}

/** `computeProposalStatus` over records + snapshot + now. Pure. */
export function statusFromRecords(
	snapshot: StatusSnapshot,
	records: StatusRecords,
	nowSeconds: bigint,
): StatusComputationResult {
	return computeProposalStatus(
		{
			id: "",
			spaceId: "",
			name: null,
			proposedBy: "",
			proposalVersion: 0,
			...snapshot,
			executedAt: records.executedAt,
			unexecutableAt: records.unexecutableAt,
			yesCount: records.yes,
			noCount: records.no,
			abstainCount: records.abstain,
			votes: [],
			actions: [],
		},
		nowSeconds,
	)
}

/** Yes, no and abstain counts from vote options (1 yes, 2 no, 3 abstain). */
export function tallyVotes(options: Iterable<1 | 2 | 3>): {yes: bigint; no: bigint; abstain: bigint} {
	let yes = 0n
	let no = 0n
	let abstain = 0n
	for (const o of options) {
		if (o === 1) yes++
		else if (o === 2) no++
		else abstain++
	}
	return {yes, no, abstain}
}

export const sameResult = (a: StatusComputationResult, b: StatusComputationResult): boolean =>
	a.status === b.status &&
	a.isQuorumReached === b.isQuorumReached &&
	a.isThresholdReached === b.isThresholdReached &&
	a.isEarlyExecutable === b.isEarlyExecutable
