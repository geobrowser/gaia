/**
 * Proposal versions as natural-key records (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `governance` family),
 * behind GEODB_PROPOSAL_VERSIONS, which takes effect only with GEODB_RECORD_KEYS (proposalVersionsKeyedEnabled). With
 * either off nothing here runs. A flag of its own: the backfill must run before the consumer first starts.
 *
 * Source: `<env>.space.governance`, messages whose `event-type` header is `PROPOSAL_CREATED` or `PROPOSAL_UPDATED` (and `PROPOSAL_SETTINGS_UPDATED`, below)
 * (HermesProposalCreated / HermesProposalUpdated, hermes-schema/proto/governance.proto), the events kg-indexer folds
 * into gaia's `proposal_versions` (handlers/governance.rs map_proposal_message; storage.rs
 * insert_proposal_version_initial and insert_new_proposal_version). It is the same message the proposal-actions
 * consumer reads (proposalActionsKeyed.ts), on its own group. One record per version, in the `governance` system
 * Space (RECORD_KINDS.proposalVersion is pinned to that family):
 * - record entity: `recordEntityId(proposalVersion, [id(proposal), int(block)])`;
 * - key columns (RECORD_COLUMNS.proposalVersion): `add` + `set_active(true)`: a reference to the proposal and the block;
 * - the other columns are `put`, under `recordStatementId(proposalVersion, [record, property, value])`: voting_mode
 *   (text, Fast or Slow), start_time, end_time, quorum, partial, universal, flat (numbers), version_created_at
 *   (number), and only when present execute_by (number; the event's 0 is "no deadline", stored as no column) and name
 *   (text; absent when the event has no actions).
 *
 * KEY, AND THE ORDINAL. Postgres keys a row by (proposal_id, proposal_version), the ordinal storage assigns as
 * `current_version + 1` when it inserts the row (idempotent on the unique (proposal_id, version_created_at_block)). An
 * event does not carry the ordinal, so, as proposalActionsKeyed.ts does for the actions of the version, the record is
 * keyed by the block that stamps the version (`meta.block_number`, 0 when the event has no metadata). The ordinal is
 * NOT stored: no value an event alone fixes can hold it, and a backfill that wrote it from Postgres would make a
 * record differ by its writer. A reader takes the rank of the block among the proposal's version records; the gate
 * (proposalVersionsGate.ts) checks that rank against Postgres's ordinal.
 *
 * COLUMNS NOT STORED. `proposal_version` (the ordinal, above). `threshold`: kg-indexer's legacy copy, `flat` for a Fast
 * version and `partial` for a Slow one (legacy_threshold), so a reader derives it. `yes_count`, `no_count` and
 * `abstain_count`: written by kg-indexer's tally worker from `proposal_votes`, not by the event; geo-db derives them from
 * the proposal-vote records (proposalVotesKeyed.ts, proposalTallyGate.ts).
 *
 * ESCALATION. A fast-to-slow escalation (`PROPOSAL_SETTINGS_UPDATED`) rewrites the settings of the proposal's CURRENT
 * version in place in Postgres, an event that does not say which version. So these records hold each version's settings
 * as its CREATE or UPDATE event set them, and the escalation is its own immutable record (proposalEscalationKeyed.ts,
 * written by this consumer's `PROPOSAL_SETTINGS_UPDATED` branch). A reader resolves it onto the version at the greatest
 * block at or before the escalation (resolveEscalations); the gate does the same before comparing. A backfill of
 * Postgres writes escalated values into the version records, and a replayed escalation record overlays the same values.
 * There is no escalation backfill: Postgres keeps no history of the event.
 *
 * Every event's intents are fixed by the event alone: no read before write, no journal, and a replay is a no-op.
 * Versions are immutable once written (kg-indexer inserts with ON CONFLICT DO NOTHING).
 *
 * Accepted and rejected where kg-indexer's map_proposal_message accepts and rejects: an id that is not 16 bytes, or a
 * message with no `settings`, is dead-lettered. A voting mode that is not Fast reads as Slow, as kg-indexer maps it.
 * A wire type protobuf would not decode, or a content_uri or publish name that is not UTF-8, is a decode failure
 * (thrown, dead-lettered).
 *
 * Not mirrored, in either direction: kg-indexer drops the WHOLE block when any write in it fails permanently, so geo-db
 * can hold versions Postgres lacks. Treat any difference between `proposal_versions` and these records as expected and
 * investigate it, never as a bug in either (proposalVersionsGate.ts reports it as geo-db-only).
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
import {applyProposalEscalation, PROPOSAL_SETTINGS_UPDATED, parseProposalEscalation} from "./proposalEscalationKeyed.js"
import {headerText, i64, id16, lastFields, num} from "./proposalActionsKeyed.js"
import {bytesPart, idPart, intPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent, zigString} from "./translate.js"

const KIND = RECORD_KINDS.proposalVersion.id
const FAMILY = RECORD_KINDS.proposalVersion.family
const COL = RECORD_COLUMNS.proposalVersion

export const PROPOSAL_CREATED = "PROPOSAL_CREATED"
export const PROPOSAL_UPDATED = "PROPOSAL_UPDATED"

export type VotingMode = "Fast" | "Slow"

export type ProposalVersion = {
	proposal: string
	/** The block that stamps the version (u64 bit pattern of the i64 Postgres holds). */
	block: bigint
	votingMode: VotingMode
	/** The five numbers below and createdAt are i64, the cast kg-indexer's `x as i64` makes. */
	startTime: bigint
	endTime: bigint
	quorum: bigint
	partial: bigint
	universal: bigint
	flat: bigint
	/** Absent when the event's `execute_by` is 0. */
	executeBy?: bigint
	/** Absent when the event has no actions. */
	name?: string
	createdAt: bigint
}

/** `ProposalAction` oneof member numbers to the label kg-indexer's derive_proposal_name gives them. */
const LABELS: Record<number, string> = {
	10: "Add Member",
	11: "Remove Member",
	12: "Add Editor",
	13: "Remove Editor",
	14: "Unflag Editor",
	16: "Flag",
	17: "Unflag",
	18: "Update Voting Settings",
	19: "Add Verified Space",
	20: "Remove Verified Space",
	21: "Add Related Space",
	22: "Remove Related Space",
	23: "Add subtopic",
	24: "Remove subtopic",
	25: "Set Topic",
	26: "Unset Topic",
}
const MAX_NAME_BYTES = 500

const utf8 = new TextDecoder("utf-8", {fatal: true})

/** kg-indexer's derive_proposal_name: labels joined with ", ", a Publish by its name; at most 500 bytes. */
export function deriveProposalName(actions: Uint8Array[]): string | undefined {
	if (actions.length === 0) return undefined
	const names = actions.map((a) => {
		let member: number | null = null
		let body: Uint8Array = new Uint8Array(0)
		for (const [n, v] of parseFields(a)) {
			if (n >= 10 && n <= 26) {
				if (!(v instanceof Uint8Array)) throw new Error("proposal version: action member has an invalid wire type")
				member = n
				body = v
			}
		}
		if (member === null) return "Unknown Action"
		if (member !== 15) return LABELS[member] as string
		const name = lastFields(body).get(3)
		if (name !== undefined && !(name instanceof Uint8Array)) throw new Error("proposal version: publish name has an invalid wire type")
		// prost decodes every string of the message, so a content_uri that is not UTF-8 fails the whole message.
		const uri = lastFields(body).get(1)
		if (uri instanceof Uint8Array) utf8.decode(uri)
		const text = name === undefined ? "" : utf8.decode(name)
		return text === "" ? "Publish" : text
	})
	const joined = Buffer.from(names.join(", "), "utf8")
	if (joined.length <= MAX_NAME_BYTES) return joined.toString("utf8")
	// Truncate to a char boundary at or before 497 bytes, then at the last ", " when there is one (is_char_boundary).
	let end = MAX_NAME_BYTES - 3
	while (end > 0 && ((joined[end] as number) & 0xc0) === 0x80) end--
	const safe = joined.subarray(0, end)
	const comma = safe.lastIndexOf(", ")
	return `${(comma >= 0 ? safe.subarray(0, comma) : safe).toString("utf8")}...`
}

/**
 * The version one PROPOSAL_CREATED or PROPOSAL_UPDATED message carries, keyed by the message's block. Throws where
 * kg-indexer's map_proposal_message errors (an id not 16 bytes, no settings) or protobuf would not decode.
 */
export function parseProposalVersion(payload: Uint8Array): ProposalVersion {
	const f = new Map<number, bigint | Uint8Array>()
	const actions: Uint8Array[] = []
	for (const [n, v] of parseFields(payload)) {
		if (n === 5) {
			if (!(v instanceof Uint8Array)) throw new Error("proposal version: actions has an invalid wire type")
			actions.push(v)
		} else f.set(n, v)
	}
	id16(f.get(1), "space_id")
	id16(f.get(2), "proposer_id")
	const proposal = id16(f.get(3), "proposal_id")
	const settingsBytes = f.get(6)
	if (!(settingsBytes instanceof Uint8Array)) throw new Error("proposal version: message has no settings")
	// ProtoVotingMode::try_from: 0 is Fast, anything else (1, or a value the enum lacks) is Slow.
	const votingMode: VotingMode = num(f.get(4), "voting_mode") === 0n ? "Fast" : "Slow"
	const s = lastFields(settingsBytes)
	let block = 0n
	let createdAt = 0n
	const meta = f.get(10)
	if (meta !== undefined) {
		if (!(meta instanceof Uint8Array)) throw new Error("proposal version: invalid metadata wire type")
		for (const [n, v] of parseFields(meta)) {
			if (n === 1 || n === 3) {
				if (typeof v !== "bigint") throw new Error("proposal version: invalid metadata wire type")
				if (n === 1) createdAt = v
				else block = v
			}
		}
	}
	const executeBy = num(s.get(14), "execute_by")
	const name = deriveProposalName(actions)
	return {
		proposal,
		block: BigInt.asUintN(64, i64(block)),
		votingMode,
		startTime: i64(num(s.get(12), "start_date")),
		endTime: i64(num(s.get(13), "last_date")),
		quorum: i64(num(s.get(11), "quorum")),
		partial: i64(num(s.get(8), "partial")),
		universal: i64(num(s.get(9), "universal")),
		flat: i64(num(s.get(10), "flat")),
		...(executeBy !== 0n ? {executeBy: i64(executeBy)} : {}),
		...(name !== undefined ? {name} : {}),
		createdAt: i64(createdAt),
	}
}

/** The record entity of one version. */
export function proposalVersionRecordId(v: Pick<ProposalVersion, "proposal" | "block">): string {
	return recordEntityId(KIND, [idPart(v.proposal), intPart(BigInt.asUintN(64, v.block))])
}

const bits = (v: bigint) => intPart(BigInt.asUintN(64, v))

/** The intents for one version. Pure, and depends on the version alone. */
export function proposalVersionIntents(v: ProposalVersion): string[] {
	const record = proposalVersionRecordId(v)
	const proposal = cid("entity", v.proposal)
	const out = [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.proposal), idPart(proposal)]), record, COL.proposal, `.{ .reference = "${proposal}" }`),
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.block), bits(v.block)]), record, COL.block, `.{ .number = ${BigInt.asIntN(64, v.block)} }`),
	]
	const put = (property: string, key: ReturnType<typeof intPart>, value: string) =>
		out.push(putIntent({id: recordStatementId(KIND, [idPart(record), idPart(property), key]), entity: record, property, value}))
	const text = (property: string, t: string) => put(property, bytesPart(new TextEncoder().encode(t)), `.{ .text = ${zigString(t)} }`)
	const n = (property: string, x: bigint | undefined) => {
		if (x !== undefined) put(property, bits(x), `.{ .number = ${x} }`)
	}
	text(COL.votingMode, v.votingMode)
	n(COL.startTime, v.startTime)
	n(COL.endTime, v.endTime)
	n(COL.quorum, v.quorum)
	n(COL.partial, v.partial)
	n(COL.universal, v.universal)
	n(COL.flat, v.flat)
	n(COL.executeBy, v.executeBy)
	if (v.name !== undefined) text(COL.name, v.name)
	n(COL.createdAt, v.createdAt)
	return out
}

export type KeyedProposalVersionsDeps = KeyedRecordDeps

/** The `governance` system Space holds every version; the apply's `space` is the proposal's, for dead-letter context. */
export const applyProposalVersion = (deps: KeyedProposalVersionsDeps, v: ProposalVersion, meta: {partition: number; offset: string}): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, FAMILY, proposalVersionIntents(v), {...meta, space: v.proposal})

export async function handleProposalVersionMessage(
	deps: KeyedProposalVersionsDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	const type = headerText(message.headers?.["event-type"])
	if (type !== PROPOSAL_CREATED && type !== PROPOSAL_UPDATED && type !== PROPOSAL_SETTINGS_UPDATED) return "ignored"
	if (!message.value) return "ignored"
	if (type === PROPOSAL_SETTINGS_UPDATED) {
		let escalation: ReturnType<typeof parseProposalEscalation>
		try {
			escalation = parseProposalEscalation(message.value)
		} catch (e) {
			return deadLetterDecode(deps, "space.governance", partition, message.offset, e)
		}
		return applyProposalEscalation(deps, escalation, {partition, offset: message.offset})
	}
	let version: ProposalVersion
	try {
		version = parseProposalVersion(message.value)
	} catch (e) {
		return deadLetterDecode(deps, "space.governance", partition, message.offset, e)
	}
	return applyProposalVersion(deps, version, {partition, offset: message.offset})
}

/** Whether the keyed proposal-versions writer runs: GEODB_PROPOSAL_VERSIONS only takes effect with GEODB_RECORD_KEYS. */
export const proposalVersionsKeyedEnabled = (c: {recordKeys: boolean; proposalVersions: {enabled: boolean}}): boolean =>
	c.proposalVersions.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const proposalVersionsOffsetsDir = (dataDir: string) => `${dataDir}/feeder/proposal-versions-keyed`

/** A `proposal_versions` row as the backfill reads it (pgSource.readProposalVersionRows). */
export type ProposalVersionRow = {
	proposal_id: string
	/** `version_created_at_block`, text. */
	block: string
	voting_mode: string
	start_time: string
	end_time: string
	quorum: string
	partial_percentage_support_threshold: string
	universal_percentage_support_threshold: string
	flat_support_threshold: string
	execute_by: string | null
	name: string | null
	/** `version_created_at`, text. */
	version_created_at: string
}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()
const I64 = /^-?\d+$/

const big = (k: string, v: string): bigint => {
	if (!I64.test(v) || BigInt.asIntN(64, BigInt(v)) !== BigInt(v)) throw new Error(`proposal_versions: ${k} ${JSON.stringify(v)}`)
	return BigInt(v)
}

/** A Postgres row as the version an event wrote. Throws on a value no event could have written. */
export function proposalVersionFromRow(r: ProposalVersionRow): ProposalVersion {
	if (r.voting_mode !== "Fast" && r.voting_mode !== "Slow") throw new Error(`proposal_versions: voting_mode ${JSON.stringify(r.voting_mode)}`)
	return {
		proposal: compact(r.proposal_id),
		block: BigInt.asUintN(64, big("block", r.block)),
		votingMode: r.voting_mode,
		startTime: big("start_time", r.start_time),
		endTime: big("end_time", r.end_time),
		quorum: big("quorum", r.quorum),
		partial: big("partial", r.partial_percentage_support_threshold),
		universal: big("universal", r.universal_percentage_support_threshold),
		flat: big("flat", r.flat_support_threshold),
		...(r.execute_by !== null ? {executeBy: big("execute_by", r.execute_by)} : {}),
		...(r.name !== null ? {name: r.name} : {}),
		createdAt: big("version_created_at", r.version_created_at),
	}
}

/**
 * Write a snapshot of `proposal_versions`. Rerunnable: a held row is written again to the same ids. A row Postgres holds
 * that no event could have written is dead-lettered (counted, the Job fails at the end) rather than invented.
 */
export const backfillProposalVersions = (
	deps: KeyedProposalVersionsDeps,
	rows: AsyncIterable<ProposalVersionRow> | Iterable<ProposalVersionRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.governance", fromRow: proposalVersionFromRow, apply: (d, v) => applyProposalVersion(d, v, {partition: -1, offset: "backfill"})},
		progress,
	)
