/**
 * The governance records the twin reads, as the feeder pins them (geodb-feeder/src/recordKinds.ts), and the decoders
 * from a geo-db card to the inputs of the executable decision. The executor cannot import the feeder, so the ids are
 * copied; a test reads the feeder's file when it is in the checkout and fails if any copied id differs.
 */
import {idPart, recordEntityId} from "./recordKey.js"

export const PROPOSAL_STATUS_KIND = "cf398f45b2c74002bb13e5416fbc7027"
export const PROPOSAL_VERSION_KIND = "6863abef6664450eb135a182c6ff1715"
export const PROPOSAL_VOTE_KIND = "ca89a877762c4883a3640b5455a8f0e2"

export const PROPOSAL_VERSION_COLUMNS = {
	proposal: "c42be9e340d14566b2b9f084004c748b",
	block: "bccb84d7017849b0babfa7444789068f",
	votingMode: "1ec5fb72263443668b4880d09670cb56",
	endTime: "72d87497d7ed49db81e31675a31c5da7",
	quorum: "54000906476b41b7a0ca655f043b1a8d",
	partial: "e4e76529227940f08bd1ca1ab884fe31",
	universal: "eecb13aaa70c44509070df7550436103",
	executeBy: "ca75ecfb5ab24196998de1b3bb31eb6f",
	createdAt: "6c74f4f248d249368e1002b4c393a59e",
} as const

export const PROPOSAL_STATUS_COLUMNS = {
	executedAt: "ed3855a3c41b4f749651bf049dabac0b",
	unexecutableAt: "72179d56e3dc478d9c9ab2159e4b40ef",
} as const

export const PROPOSAL_VOTE_COLUMNS = {
	proposal: "c020ebd69bbb4bec8c83db30765aa91b",
	version: "da1d8b5c47ee499797e729ae90938086",
	vote: "328dc69e2c964982a35fe4d3aa79e31a",
} as const

/** The `editor` role property the membership dual write puts on a Space's own entity (feeder ROLE_PROPERTY.editor). */
export const EDITOR_PROPERTY = "79c3a498486641bc930f3b973de4b4d3"

/** Entity id of a proposal's status record, in the `governance` system Space. */
export const proposalStatusRecordId = (proposalId: string): string =>
	recordEntityId(PROPOSAL_STATUS_KIND, [idPart(proposalId)])

/** A card row: a property and the value geo-db printed for it. */
export type CardRow = {prop: string; raw: string}

/** A record holds two different values for a column that should hold one, or one that does not parse. */
export class AmbiguousRecordError extends Error {}

function single(rows: CardRow[], property: string): string | null {
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
export function decodeProposalStatus(rows: CardRow[]): {executedAt: bigint | null; unexecutableAt: bigint | null} {
	const executed = single(rows, PROPOSAL_STATUS_COLUMNS.executedAt)
	const unexecutable = single(rows, PROPOSAL_STATUS_COLUMNS.unexecutableAt)
	return {
		executedAt: executed === null ? null : instantSeconds(executed),
		unexecutableAt: unexecutable === null ? null : instantSeconds(unexecutable),
	}
}

export type DecodedVersion = {
	block: bigint
	votingMode: "Fast" | "Slow"
	endTime: bigint
	quorum: bigint
	partial: bigint
	universal: bigint
	executeBy: bigint | null
	createdAt: bigint
}

/**
 * A proposal version record's block (its key, and the order of versions) and the settings the decision reads. A null
 * `execute_by` is no column (the event's 0). A version missing a required column is ambiguous, not defaulted.
 */
export function decodeProposalVersion(rows: CardRow[]): DecodedVersion | null {
	if (rows.length === 0) return null
	const need = (property: string): string => {
		const v = single(rows, property)
		if (v === null) throw new AmbiguousRecordError(`version column ${property} missing`)
		return v
	}
	const mode = need(PROPOSAL_VERSION_COLUMNS.votingMode)
	if (mode !== "Fast" && mode !== "Slow") throw new AmbiguousRecordError(`unknown voting mode ${mode}`)
	const executeBy = single(rows, PROPOSAL_VERSION_COLUMNS.executeBy)
	return {
		block: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.block)),
		votingMode: mode,
		endTime: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.endTime)),
		quorum: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.quorum)),
		partial: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.partial)),
		universal: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.universal)),
		executeBy: executeBy === null ? null : wholeNumber(executeBy),
		createdAt: wholeNumber(need(PROPOSAL_VERSION_COLUMNS.createdAt)),
	}
}

/** A vote record's version ordinal and option (1 yes, 2 no, 3 abstain); null when the record has neither. */
export function decodeProposalVote(rows: CardRow[]): {version: bigint; option: 1 | 2 | 3} | null {
	const vote = single(rows, PROPOSAL_VOTE_COLUMNS.vote)
	const version = single(rows, PROPOSAL_VOTE_COLUMNS.version)
	if (vote === null || version === null) return null
	const n = wholeNumber(vote)
	if (n !== 1n && n !== 2n && n !== 3n) throw new AmbiguousRecordError(`unknown vote option ${vote}`)
	return {version: wholeNumber(version), option: Number(n) as 1 | 2 | 3}
}
