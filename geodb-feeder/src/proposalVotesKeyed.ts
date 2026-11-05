/**
 * Proposal votes as natural-key records (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `votes` family), behind
 * GEODB_PROPOSAL_VOTES, which takes effect only with GEODB_RECORD_KEYS (proposalVotesKeyedEnabled). With either off
 * nothing here runs. A flag of its own, not GEODB_VOTES: this family's backfill must run before its consumer first
 * starts, so turning it on is a step of its own even where GEODB_VOTES is already on.
 *
 * Source: `<env>.space.governance`, messages whose `event-type` header is `PROPOSAL_VOTED` (HermesProposalVoted,
 * hermes-schema/proto/governance.proto), the events kg-indexer folds into gaia's `proposal_votes`. Every other event
 * type on the topic is ignored, as kg-indexer routes by that header. One record per `proposal_votes` primary key, in
 * the `votes` system Space (systemSpaces.ts; RECORD_KINDS.proposalVote is pinned to that family):
 * - record entity: `recordEntityId(proposalVote, [id(proposal), int(u32(version)), id(voter)])`, the gaia ids as
 *   Postgres holds them (raw 16 bytes), so the record is a pure function of the row's key. The version is the
 *   event's u32 with 0 read as 1 (kg-indexer's normalize_proposal_version), stored as the i32 Postgres holds and keyed
 *   as its u32 bit pattern, which is the version itself for every version Postgres can hold from an event;
 * - key columns (RECORD_COLUMNS.proposalVote): `add` + `set_active(true)`; references to `cid("entity", ...)` for the
 *   proposal and the voter (a Space), a number for the version; and the owning Space, a reference written the same
 *   way. kg-indexer never updates `space_id` on conflict; it is a function of the proposal (`proposals.space_id`), so
 *   a correct producer never sends two for one key. K5: no target needs to exist in this Space;
 * - mutable columns: `put` of `vote` (1 yes, 2 no, 3 abstain), `created_at` (instant) and `created_at_block`, each
 *   under `recordStatementId(proposalVote, [record, property, int(value)])`. A put closes the cell's other active
 *   statements, so a changed vote replaces the old one, as kg-indexer's `ON CONFLICT DO UPDATE` does, and including
 *   the value in the id makes A -> B -> A end on A and a replayed event a no-op.
 *
 * Every event's intents are fixed by the event alone: no read before write, no journal. hermes keys PROPOSAL_VOTED by
 * the owning Space (emit.rs), and every key includes the proposal, which belongs to one Space, so one key's events
 * arrive in order on one partition and a replay converges on the last event per key, as Postgres does.
 *
 * Accepted and rejected exactly where kg-indexer's handle_proposal_voted accepts and rejects: an id that is not 16
 * bytes is dead-lettered; none (0) or an unknown option is written as abstain; missing metadata is written as time 0,
 * block 0. Not mirrored: kg-indexer drops a whole block when any write in it fails permanently (an FK violation, for
 * one: `proposal_votes_version_fk` needs the proposal version's row), and this writer has no such dependency, so
 * geo-db can hold a vote Postgres lacks. The backfill cannot see those; the stream replays them while retained.
 *
 * Tallies (`proposals.yes_count` and the rest) are derived by kg-indexer's tally worker and are not written here.
 */
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {
	applyKeyedRecord,
	backfillKeyedRows,
	deadLetterDecode,
	type KeyedBackfillResult,
	type KeyedOutcome,
	type KeyedRecordDeps,
	keyColumn,
} from "./keyedRecord.js"
import {idPart, intPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent} from "./translate.js"

const KIND = RECORD_KINDS.proposalVote.id
const FAMILY = RECORD_KINDS.proposalVote.family
const COL = RECORD_COLUMNS.proposalVote

/** The `event-type` header value kg-indexer decodes as HermesProposalVoted. */
export const PROPOSAL_VOTED = "PROPOSAL_VOTED"

/** `voteOption` as a number: 1 Yes, 2 No, 3 Abstain. */
export type ProposalVoteOption = 1 | 2 | 3

export type ProposalVote = {
	proposal: string
	/** The version as Postgres holds it (i32). */
	version: number
	voter: string
	space: string
	vote: ProposalVoteOption
	/** Block time in whole seconds (i64, as Postgres's text holds it). */
	createdAtSeconds: bigint
	createdAtBlock: bigint
}

const hex16 = (v: bigint | Uint8Array | undefined, what: string): string => {
	// proto3: an absent bytes field is empty, and Uuid::from_slice refuses anything but 16 bytes.
	const b = v === undefined ? new Uint8Array(0) : v
	if (!(b instanceof Uint8Array) || b.length !== 16) throw new Error(`proposal vote: ${what} is not 16 bytes`)
	return Buffer.from(b).toString("hex")
}

/** kg-indexer's normalize_proposal_version, then `raw as i32`. */
export const normalizeProposalVersion = (raw: bigint): number => {
	const u32 = BigInt.asUintN(32, raw)
	return Number(BigInt.asIntN(32, u32 === 0n ? 1n : u32))
}

/** The version as a key part: its u32 bit pattern, injective on i32 and the version itself when >= 0. */
export const proposalVersionKey = (version: number): bigint => {
	if (!Number.isInteger(version) || version < -(2 ** 31) || version >= 2 ** 31) throw new Error(`proposal vote: version ${version} is not an i32`)
	return BigInt.asUintN(32, BigInt(version))
}

/** Decode a HermesProposalVoted with kg-indexer's handle_proposal_voted rules. Throws where it errors. */
export function parseProposalVoted(b: Uint8Array): ProposalVote {
	const f = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(b)) f.set(n, v)
	let createdAt = 0n
	let block = 0n
	const meta = f.get(5)
	if (meta instanceof Uint8Array) {
		for (const [n, v] of parseFields(meta)) {
			if (n === 1 && typeof v === "bigint") createdAt = v
			else if (n === 3 && typeof v === "bigint") block = v
		}
	}
	const raw = f.get(4) ?? 0n
	// proto3 enum: prost keeps an unknown value, and try_from(None | unknown) is Abstain.
	const opt = typeof raw === "bigint" ? BigInt.asIntN(32, raw) : 0n
	const vote: ProposalVoteOption = opt === 1n ? 1 : opt === 2n ? 2 : 3
	const ver = f.get(6) ?? 0n
	if (typeof ver !== "bigint") throw new Error("proposal vote: invalid proposal_version")
	return {
		proposal: hex16(f.get(3), "proposal_id"),
		version: normalizeProposalVersion(ver),
		voter: hex16(f.get(1), "voter_id"),
		space: hex16(f.get(2), "space_id"),
		vote,
		// `m.created_at as i64`, `m.block_number as i64`
		createdAtSeconds: BigInt.asIntN(64, createdAt),
		createdAtBlock: BigInt.asIntN(64, block),
	}
}

/** The record entity of one `proposal_votes` key. */
export function proposalVoteRecordId(v: Pick<ProposalVote, "proposal" | "version" | "voter">): string {
	return recordEntityId(KIND, [idPart(v.proposal), intPart(proposalVersionKey(v.version)), idPart(v.voter)])
}

// A signed value's statement-id key part is its u64 bit pattern (record keys hold unsigned ints).
const valueStatementId = (record: string, property: string, value: bigint | number) =>
	recordStatementId(KIND, [idPart(record), idPart(property), intPart(BigInt.asUintN(64, BigInt(value)))])
const refStatementId = (record: string, property: string, target: string) =>
	recordStatementId(KIND, [idPart(record), idPart(property), idPart(target)])

/** The intents for one proposal vote. Pure, and depends on the vote alone. */
export function proposalVoteIntents(v: ProposalVote): string[] {
	const record = proposalVoteRecordId(v)
	const refs: [string, string][] = [
		[COL.proposal, cid("entity", v.proposal)],
		[COL.voter, cid("entity", v.voter)],
		[COL.space, cid("entity", v.space)],
	]
	const put = (property: string, key: bigint | number, value: string) =>
		putIntent({id: valueStatementId(record, property, key), entity: record, property, value})
	return [
		`.{ .create_entity = "${record}" }`,
		...refs.flatMap(([p, t]) => keyColumn(refStatementId(record, p, t), record, p, `.{ .reference = "${t}" }`)),
		...keyColumn(valueStatementId(record, COL.version, proposalVersionKey(v.version)), record, COL.version, `.{ .number = ${v.version} }`),
		put(COL.vote, v.vote, `.{ .number = ${v.vote} }`),
		put(COL.createdAt, v.createdAtSeconds, `.{ .instant = "${v.createdAtSeconds * 1_000_000n}" }`),
		put(COL.createdAtBlock, v.createdAtBlock, `.{ .number = ${v.createdAtBlock} }`),
	]
}

export type KeyedProposalVotesDeps = KeyedRecordDeps

export async function applyProposalVote(
	deps: KeyedProposalVotesDeps,
	v: ProposalVote,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> {
	// Every proposal vote lives in the one votes Space; a founding failure is thrown, never dead-lettered.
	return applyKeyedRecord(deps, FAMILY, proposalVoteIntents(v), {...meta, space: v.space})
}

const headerText = (h: unknown): string | undefined => {
	const v = Array.isArray(h) ? h[0] : h
	if (v === undefined || v === null) return undefined
	return Buffer.isBuffer(v) || v instanceof Uint8Array ? Buffer.from(v).toString("utf8") : String(v)
}

export async function handleProposalVoteMessage(
	deps: KeyedProposalVotesDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	// kg-indexer routes `space.governance` by this header; only PROPOSAL_VOTED is a proposal vote.
	if (headerText(message.headers?.["event-type"]) !== PROPOSAL_VOTED) return "ignored"
	if (!message.value) return "ignored"
	let v: ProposalVote
	try {
		v = parseProposalVoted(message.value)
	} catch (e) {
		return deadLetterDecode(deps, "space.governance", partition, message.offset, e)
	}
	return applyProposalVote(deps, v, {partition, offset: message.offset})
}

/** Whether the keyed proposal-votes writer runs: GEODB_PROPOSAL_VOTES only takes effect with GEODB_RECORD_KEYS. */
export const proposalVotesKeyedEnabled = (c: {recordKeys: boolean; proposalVotes: {enabled: boolean}}): boolean =>
	c.proposalVotes.enabled && c.recordKeys

/**
 * The proposal-votes consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate), as
 * the user-votes backfill gates on its own: once the consumer has applied events a snapshot row could put back a
 * vote the stream has since changed.
 */
export const proposalVotesOffsetsDir = (dataDir: string) => `${dataDir}/feeder/proposal-votes-keyed`

/** A `proposal_votes` row as the backfill reads it (pgSource.readProposalVoteRows). */
export type ProposalVoteRow = {
	proposal_id: string
	proposal_version: number
	voter_id: string
	space_id: string
	vote: string
	created_at: string
	created_at_block: string
}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()
const OPTIONS: Record<string, ProposalVoteOption> = {Yes: 1, No: 2, Abstain: 3}
const I64 = /^-?\d+$/

/** A Postgres row as the event that set it. Throws on a value no event could have written. */
export function proposalVoteFromRow(r: ProposalVoteRow): ProposalVote {
	const vote = OPTIONS[r.vote]
	if (vote === undefined) throw new Error(`proposal_votes: vote ${JSON.stringify(r.vote)}`)
	// normalize_proposal_version never yields 0; a negative i32 is a u32 above i32::MAX, which an event can carry.
	if (!Number.isInteger(r.proposal_version) || r.proposal_version === 0) throw new Error(`proposal_votes: proposal_version ${r.proposal_version}`)
	for (const [k, v] of [["created_at", r.created_at], ["created_at_block", r.created_at_block]] as const) {
		if (!I64.test(v) || BigInt.asIntN(64, BigInt(v)) !== BigInt(v)) throw new Error(`proposal_votes: ${k} ${JSON.stringify(v)}`)
	}
	return {
		proposal: compact(r.proposal_id),
		version: r.proposal_version,
		voter: compact(r.voter_id),
		space: compact(r.space_id),
		vote,
		createdAtSeconds: BigInt(r.created_at),
		createdAtBlock: BigInt(r.created_at_block),
	}
}

/**
 * Write a snapshot of `proposal_votes`. Rerunnable: a held row is written again to the same ids. A row Postgres holds
 * that no event could have written is dead-lettered (counted, the Job fails at the end) rather than invented.
 */
export const backfillProposalVotes = (
	deps: KeyedProposalVotesDeps,
	rows: AsyncIterable<ProposalVoteRow> | Iterable<ProposalVoteRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.governance", fromRow: proposalVoteFromRow, apply: (d, v) => applyProposalVote(d, v, {partition: -1, offset: "backfill"})},
		progress,
	)
