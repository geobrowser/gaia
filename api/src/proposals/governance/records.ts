/**
 * The governance records the shadow reads, as the feeder pins them (geodb-feeder/src/recordKinds.ts), and the
 * decoders from a geo-db card to the stored inputs of a proposal's status.
 *
 * The api cannot import the feeder, so the ids are copied; `__tests__/governanceRecords.test.ts` reads the feeder's
 * file and fails if any copied id differs from it. `proposalStatus` is pinned by the PROPOSAL-STATUS change; the
 * test checks it once that file carries it.
 */
import type {GeoCardRow} from "../../geodb/card"
import {idPart, intPart, recordEntityId} from "./recordKey"

export const PROPOSAL_STATUS_KIND = "cf398f45b2c74002bb13e5416fbc7027"
export const PROPOSAL_VOTE_KIND = "ca89a877762c4883a3640b5455a8f0e2"

export const PROPOSAL_STATUS_COLUMNS = {
	executedAt: "ed3855a3c41b4f749651bf049dabac0b",
	unexecutableAt: "72179d56e3dc478d9c9ab2159e4b40ef",
} as const

export const PROPOSAL_VOTE_COLUMNS = {
	vote: "328dc69e2c964982a35fe4d3aa79e31a",
} as const

/** Entity id of a proposal's status record, in the `governance` system Space. */
export const proposalStatusRecordId = (proposalId: string): string =>
	recordEntityId(PROPOSAL_STATUS_KIND, [idPart(proposalId)])

/** Entity id of one voter's vote on one proposal version, in the `votes` system Space. */
export const proposalVoteRecordId = (proposalId: string, version: number, voterId: string): string =>
	recordEntityId(PROPOSAL_VOTE_KIND, [
		idPart(proposalId),
		intPart(BigInt.asUintN(32, BigInt(version))),
		idPart(voterId),
	])

/** A record holds two different values for a column that should hold one. */
export class AmbiguousRecordError extends Error {}

function single(rows: GeoCardRow[], property: string): string | null {
	const values = new Set(rows.filter((r) => r.prop === property).map((r) => r.raw))
	if (values.size > 1) throw new AmbiguousRecordError(`column ${property} holds ${values.size} values`)
	return [...values][0] ?? null
}

/** `instant:<sec>.<fraction>` as whole seconds. */
export function instantSeconds(raw: string): bigint {
	const m = /^instant:(-?\d+)(?:\.\d+)?$/.exec(raw)
	if (!m) throw new AmbiguousRecordError(`not an instant: ${raw}`)
	return BigInt(m[1] as string)
}

/** A whole number as geo-db prints it (`1`, or `1.0`). */
export function wholeNumber(raw: string): bigint {
	const m = /^(-?\d+)(?:\.0+)?$/.exec(raw)
	if (!m) throw new AmbiguousRecordError(`not a whole number: ${raw}`)
	return BigInt(m[1] as string)
}

/** executed_at and unexecutable_at from a status record's card. No row for a column means the source never had it. */
export function decodeProposalStatus(rows: GeoCardRow[]): {executedAt: bigint | null; unexecutableAt: bigint | null} {
	const executed = single(rows, PROPOSAL_STATUS_COLUMNS.executedAt)
	const unexecutable = single(rows, PROPOSAL_STATUS_COLUMNS.unexecutableAt)
	return {
		executedAt: executed === null ? null : instantSeconds(executed),
		unexecutableAt: unexecutable === null ? null : instantSeconds(unexecutable),
	}
}

/** A vote record's option (1 yes, 2 no, 3 abstain), or null when the record does not exist. */
export function decodeProposalVote(rows: GeoCardRow[]): 1 | 2 | 3 | null {
	const raw = single(rows, PROPOSAL_VOTE_COLUMNS.vote)
	if (raw === null) return null
	const n = wholeNumber(raw)
	if (n !== 1n && n !== 2n && n !== 3n) throw new AmbiguousRecordError(`unknown vote option ${raw}`)
	return Number(n) as 1 | 2 | 3
}
