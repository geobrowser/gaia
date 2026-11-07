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
import type {StatusSnapshot} from "./status"

export const PROPOSAL_STATUS_KIND = "cf398f45b2c74002bb13e5416fbc7027"
export const PROPOSAL_VOTE_KIND = "ca89a877762c4883a3640b5455a8f0e2"

export const PROPOSAL_VERSION_KIND = "6863abef6664450eb135a182c6ff1715"

export const PROPOSAL_VERSION_COLUMNS = {
	proposal: "c42be9e340d14566b2b9f084004c748b",
	block: "bccb84d7017849b0babfa7444789068f",
	votingMode: "1ec5fb72263443668b4880d09670cb56",
	startTime: "5a806f60759e4dd08a39c771d7586f53",
	endTime: "72d87497d7ed49db81e31675a31c5da7",
	quorum: "54000906476b41b7a0ca655f043b1a8d",
	partial: "e4e76529227940f08bd1ca1ab884fe31",
	universal: "eecb13aaa70c44509070df7550436103",
	flat: "2e641ff402734c6b80201641f3969fe2",
	executeBy: "ca75ecfb5ab24196998de1b3bb31eb6f",
} as const

export const PROPOSAL_ACTION_KIND = "22335b74e17a4c9e867d4ac2a32c3a4a"

export const PROPOSAL_ACTION_COLUMNS = {
	proposal: "235abc0092a248d69e2dbb5c71fd0209",
	block: "a93c92d0a2f442e9852057064567e012",
	index: "81b028b2bb7f4f9e85e5689a2f1826af",
	actionType: "002e8db762e0401386b53478b329013e",
	target: "afc49d0ddf9c4affaae9ff963338d7c4",
	contentUri: "a267fc081a4c457abfda85b8fc024d92",
	contentId: "268e38bcf4d247138d7dbd154873391c",
} as const

/** Key columns of a vote record, which the queries shadow reads to count a proposal's votes by version. */
export const PROPOSAL_VOTE_KEY_COLUMNS = {
	proposal: "c020ebd69bbb4bec8c83db30765aa91b",
	version: "da1d8b5c47ee499797e729ae90938086",
} as const

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

export type DecodedVersion = {block: bigint; snapshot: Omit<StatusSnapshot, "totalEditors">}

/**
 * A proposal version record's block (its key, and the order of versions) and the settings it carries. `threshold` is not
 * stored: it is the legacy copy, `flat` for a Fast version and `partial` for a Slow one. Null `execute_by` is no column
 * (the event's 0). A version missing a required column is ambiguous, not defaulted.
 */
export function decodeProposalVersion(rows: GeoCardRow[]): DecodedVersion | null {
	if (rows.length === 0) return null
	const need = (property: string): string => {
		const v = single(rows, property)
		if (v === null) throw new AmbiguousRecordError(`version column ${property} missing`)
		return v
	}
	const mode = need(PROPOSAL_VERSION_COLUMNS.votingMode)
	if (mode !== "Fast" && mode !== "Slow") throw new AmbiguousRecordError(`unknown voting mode ${mode}`)
	const flat = wholeNumber(need(PROPOSAL_VERSION_COLUMNS.flat))
	const partial = wholeNumber(need(PROPOSAL_VERSION_COLUMNS.partial))
	const executeBy = single(rows, PROPOSAL_VERSION_COLUMNS.executeBy)
	return {
		block: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.block)),
		snapshot: {
			votingMode: mode,
			startTime: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.startTime)),
			endTime: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.endTime)),
			quorum: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.quorum)),
			threshold: mode === "Fast" ? flat : partial,
			flatSupportThreshold: flat,
			partialPercentageSupportThreshold: partial,
			universalPercentageSupportThreshold: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.universal)),
			executeBy: executeBy === null ? null : wholeNumber(executeBy),
		},
	}
}

export type DecodedAction = {
	/** The proposal's geo-db entity ref (the key column), not its gaia id. */
	proposal: string
	block: bigint
	index: bigint
	actionType: string
	target: string | null
	contentUri: string | null
	contentId: string | null
}

/** A proposal action record, or null when the card is empty. A record missing a key or its type is ambiguous. */
export function decodeProposalAction(rows: GeoCardRow[]): DecodedAction | null {
	if (rows.length === 0) return null
	const need = (property: string): string => {
		const v = single(rows, property)
		if (v === null) throw new AmbiguousRecordError(`action column ${property} missing`)
		return v
	}
	return {
		proposal: need(PROPOSAL_ACTION_COLUMNS.proposal),
		block: wholeNumber(need(PROPOSAL_ACTION_COLUMNS.block)),
		index: wholeNumber(need(PROPOSAL_ACTION_COLUMNS.index)),
		actionType: need(PROPOSAL_ACTION_COLUMNS.actionType),
		target: single(rows, PROPOSAL_ACTION_COLUMNS.target),
		contentUri: single(rows, PROPOSAL_ACTION_COLUMNS.contentUri),
		contentId: single(rows, PROPOSAL_ACTION_COLUMNS.contentId),
	}
}

/** A vote record's proposal version and option, or null when the card is empty. */
export function decodeVoteRecord(rows: GeoCardRow[]): {version: bigint; option: 1 | 2 | 3} | null {
	const option = decodeProposalVote(rows)
	if (option === null) return null
	const version = single(rows, PROPOSAL_VOTE_KEY_COLUMNS.version)
	if (version === null) throw new AmbiguousRecordError("vote version missing")
	return {version: wholeNumber(version), option}
}
