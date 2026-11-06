/**
 * Proposal actions as natural-key records (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `governance` family),
 * behind GEODB_PROPOSAL_ACTIONS, which takes effect only with GEODB_RECORD_KEYS (proposalActionsKeyedEnabled). With
 * either off nothing here runs. A flag of its own: the backfill must run before the consumer first starts.
 *
 * Source: `<env>.space.governance`, messages whose `event-type` header is `PROPOSAL_CREATED` or `PROPOSAL_UPDATED`
 * (HermesProposalCreated / HermesProposalUpdated, hermes-schema/proto/governance.proto), the events kg-indexer folds
 * into gaia's `proposal_actions` (handlers/governance.rs map_proposal_message / map_proposal_action; storage.rs
 * insert_proposal_actions). Every other event type is ignored. One record per action, in the `governance` system
 * Space (RECORD_KINDS.proposalAction is pinned to that family):
 * - record entity: `recordEntityId(proposalAction, [id(proposal), int(block), int(index)])`;
 * - key columns (RECORD_COLUMNS.proposalAction): `add` + `set_active(true)`: a reference to the proposal, the block
 *   and the index;
 * - the other columns are `put`, under `recordStatementId(proposalAction, [record, property, value])`, and only the
 *   ones Postgres holds non-null: action_type (text), target (reference), content_uri (text), metadata and content_id
 *   (lowercase hex text), and for UpdateVotingSettings quorum, duration, partial, universal, flat, grace (numbers) and
 *   disable_fast_path (boolean). The legacy V1 columns `fast_threshold` (= flat) and `slow_threshold` (= partial) are
 *   copies kg-indexer writes for old API queries; they are not stored again, a reader derives them.
 *
 * DEVIATION FROM THE TABLE KEY. Postgres keys a row by (proposal_id, proposal_version, index), and an UPDATE event
 * does not carry its version: storage assigns `current_version + 1` when it inserts the version row (idempotent on
 * the unique (proposal_id, version_created_at_block)). A consumer with no read of Postgres cannot know the ordinal,
 * so the record is keyed by the block that stamps the version, `meta.block_number`
 * (`proposal_versions.version_created_at_block`, 0 when the event has no metadata), which an event does carry, is
 * unique per proposal, and for the CREATE is the version-1 block. The ordinal is not stored here: a reader that needs
 * it takes the rank of the block among the proposal's records, or a later `proposal_versions` consumer (the version
 * row has the same two keys) writes it. The backfill and the gate map a row's version to its block through
 * `proposal_versions`.
 *
 * Every event's intents are fixed by the event alone: no read before write, no journal, and a replay is a no-op.
 * Versions are immutable once written (kg-indexer inserts with ON CONFLICT DO NOTHING), so a replayed or reordered
 * event never changes a record another event wrote.
 *
 * Accepted and rejected where kg-indexer's map_proposal_message accepts and rejects: an id that is not 16 bytes, or a
 * message with no `settings`, is dead-lettered (kg-indexer errors on both); an action whose target is not 16 bytes, or
 * with no decoded action, is written as `Unknown` with no other column, as kg-indexer stores it. A wire type protobuf
 * would not decode, or a content_uri that is not UTF-8, is a decode failure (thrown, dead-lettered).
 *
 * Not mirrored, in either direction: kg-indexer drops the WHOLE block when any write in it fails permanently, so
 * geo-db can hold actions Postgres lacks. Treat any difference between `proposal_actions` and these records as
 * expected and investigate it, never as a bug in either (proposalActionsGate.ts reports it as geo-db-only).
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
import {bytesPart, idPart, intPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent, zigString} from "./translate.js"

const KIND = RECORD_KINDS.proposalAction.id
const FAMILY = RECORD_KINDS.proposalAction.family
const COL = RECORD_COLUMNS.proposalAction

export const PROPOSAL_CREATED = "PROPOSAL_CREATED"
export const PROPOSAL_UPDATED = "PROPOSAL_UPDATED"

/** The columns a row can carry beyond its key; each is present only when Postgres holds it non-null. */
export type ProposalActionColumns = {
	actionType: string
	/** 32 hex digits. */
	target?: string
	contentUri?: string
	/** Lowercase hex. */
	metadata?: string
	contentId?: string
	quorum?: bigint
	duration?: bigint
	partial?: bigint
	universal?: bigint
	flat?: bigint
	grace?: bigint
	disableFastPath?: boolean
}

export type ProposalAction = ProposalActionColumns & {
	proposal: string
	/** The block that stamps the action's version (u64 bit pattern of the i64 Postgres holds). */
	block: bigint
	index: number
}

export const hex = (b: Uint8Array) => Buffer.from(b).toString("hex")
export const asBytes = (v: bigint | Uint8Array | undefined, what: string): Uint8Array => {
	if (v === undefined) return new Uint8Array(0)
	if (!(v instanceof Uint8Array)) throw new Error(`proposal action: ${what} has an invalid wire type`)
	return v
}
export const id16 = (v: bigint | Uint8Array | undefined, what: string): string => {
	const b = asBytes(v, what)
	if (b.length !== 16) throw new Error(`proposal action: ${what} is not 16 bytes`)
	return hex(b)
}
export const num = (v: bigint | Uint8Array | undefined, what: string): bigint => {
	if (v === undefined) return 0n
	if (typeof v !== "bigint") throw new Error(`proposal action: ${what} has an invalid wire type`)
	return v
}
/** `*x as i64`, the cast kg-indexer's `Some(*x as i64)` makes. */
export const i64 = (v: bigint) => BigInt.asIntN(64, v)

/** The last value of each field number, as prost keeps it for a scalar or a oneof member. */
export function lastFields(b: Uint8Array): Map<number, bigint | Uint8Array> {
	const m = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(b)) m.set(n, v)
	return m
}

const TARGETED: Record<number, [string, "address" | "space" | "topic"]> = {
	10: ["AddMember", "address"],
	11: ["RemoveMember", "address"],
	12: ["AddEditor", "address"],
	13: ["RemoveEditor", "address"],
	14: ["UnflagEditor", "address"],
	19: ["SubspaceVerified", "space"],
	20: ["SubspaceUnverified", "space"],
	21: ["SubspaceRelated", "space"],
	22: ["SubspaceUnrelated", "space"],
	23: ["SubspaceTopicDeclared", "topic"],
	24: ["SubspaceTopicRemoved", "topic"],
	25: ["SetTopic", "topic"],
	26: ["UnsetTopic", "topic"],
}

/** One decoded `ProposalAction` message as kg-indexer's map_proposal_action maps it. */
export function parseAction(b: Uint8Array): ProposalActionColumns {
	// A oneof keeps its last member; fields 10..26 are the members.
	let member: number | null = null
	let body: Uint8Array = new Uint8Array(0)
	for (const [n, v] of parseFields(b)) {
		if (n >= 10 && n <= 26) {
			if (!(v instanceof Uint8Array)) throw new Error("proposal action: action member has an invalid wire type")
			member = n
			body = v
		}
	}
	if (member === null) return {actionType: "Unknown"}
	const t = TARGETED[member]
	if (t) {
		// Uuid::from_slice: anything but 16 bytes is Unknown.
		const target = asBytes(lastFields(body).get(1), "target")
		return target.length === 16 ? {actionType: t[0], target: hex(target)} : {actionType: "Unknown"}
	}
	const f = lastFields(body)
	if (member === 15) {
		const uri = asBytes(f.get(1), "content_uri")
		const meta = asBytes(f.get(2), "metadata")
		return {
			actionType: "Publish",
			contentUri: new TextDecoder("utf-8", {fatal: true}).decode(uri),
			...(meta.length > 0 ? {metadata: hex(meta)} : {}),
		}
	}
	if (member === 16 || member === 17) return {actionType: member === 16 ? "Flag" : "Unflag", contentId: hex(asBytes(f.get(1), "content_id"))}
	// 18, UpdateVotingSettings.
	return {
		actionType: "UpdateVotingSettings",
		partial: i64(num(f.get(5), "partial")),
		universal: i64(num(f.get(6), "universal")),
		flat: i64(num(f.get(7), "flat")),
		quorum: i64(num(f.get(8), "quorum")),
		duration: i64(num(f.get(9), "duration")),
		disableFastPath: num(f.get(10), "disable_fast_path") !== 0n,
		grace: i64(num(f.get(11), "grace")),
	}
}

/**
 * Every action of one PROPOSAL_CREATED or PROPOSAL_UPDATED message, keyed by the message's block. Throws where
 * kg-indexer's map_proposal_message errors (an id not 16 bytes, no settings) or protobuf would not decode.
 */
export function parseProposalActions(payload: Uint8Array): ProposalAction[] {
	const f = new Map<number, bigint | Uint8Array>()
	const actions: Uint8Array[] = []
	for (const [n, v] of parseFields(payload)) {
		if (n === 5) {
			if (!(v instanceof Uint8Array)) throw new Error("proposal action: actions has an invalid wire type")
			actions.push(v)
		} else f.set(n, v)
	}
	id16(f.get(1), "space_id")
	id16(f.get(2), "proposer_id")
	const proposal = id16(f.get(3), "proposal_id")
	if (!(f.get(6) instanceof Uint8Array)) throw new Error("proposal action: message has no settings")
	let block = 0n
	const meta = f.get(10)
	if (meta !== undefined) {
		if (!(meta instanceof Uint8Array)) throw new Error("proposal action: invalid metadata wire type")
		for (const [n, v] of parseFields(meta)) {
			if (n === 3) {
				if (typeof v !== "bigint") throw new Error("proposal action: invalid metadata block wire type")
				block = v
			}
		}
	}
	const stamped = BigInt.asUintN(64, i64(block))
	return actions.map((a, index) => ({...parseAction(a), proposal, block: stamped, index}))
}

/** The record entity of one action. */
export function proposalActionRecordId(a: Pick<ProposalAction, "proposal" | "block" | "index">): string {
	if (!Number.isInteger(a.index) || a.index < 0 || a.index > 0x7fffffff) throw new Error(`proposal action: index ${a.index}`)
	return recordEntityId(KIND, [idPart(a.proposal), intPart(BigInt.asUintN(64, a.block)), intPart(a.index)])
}

const bits = (v: bigint) => intPart(BigInt.asUintN(64, v))
const refStatementId = (record: string, property: string, target: string) => recordStatementId(KIND, [idPart(record), idPart(property), idPart(target)])

/** The intents for one action. Pure, and depends on the action alone. */
export function proposalActionIntents(a: ProposalAction): string[] {
	const record = proposalActionRecordId(a)
	const proposal = cid("entity", a.proposal)
	const out = [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(refStatementId(record, COL.proposal, proposal), record, COL.proposal, `.{ .reference = "${proposal}" }`),
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.block), bits(a.block)]), record, COL.block, `.{ .number = ${BigInt.asIntN(64, a.block)} }`),
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.index), intPart(a.index)]), record, COL.index, `.{ .number = ${a.index} }`),
	]
	const put = (property: string, key: ReturnType<typeof intPart>, value: string) =>
		out.push(putIntent({id: recordStatementId(KIND, [idPart(record), idPart(property), key]), entity: record, property, value}))
	const text = (property: string, v: string) => put(property, bytesPart(new TextEncoder().encode(v)), `.{ .text = ${zigString(v)} }`)
	const n = (property: string, v: bigint | undefined) => {
		if (v !== undefined) put(property, bits(v), `.{ .number = ${v} }`)
	}
	text(COL.actionType, a.actionType)
	if (a.target !== undefined) {
		const t = cid("entity", a.target)
		put(COL.target, idPart(t), `.{ .reference = "${t}" }`)
	}
	if (a.contentUri !== undefined) text(COL.contentUri, a.contentUri)
	if (a.metadata !== undefined) text(COL.metadata, a.metadata)
	if (a.contentId !== undefined) text(COL.contentId, a.contentId)
	n(COL.quorum, a.quorum)
	n(COL.duration, a.duration)
	n(COL.partial, a.partial)
	n(COL.universal, a.universal)
	n(COL.flat, a.flat)
	n(COL.grace, a.grace)
	if (a.disableFastPath !== undefined) put(COL.disableFastPath, intPart(a.disableFastPath ? 1 : 0), `.{ .boolean = ${a.disableFastPath} }`)
	return out
}

export type KeyedProposalActionsDeps = KeyedRecordDeps

/** The `governance` system Space holds every action; the apply's `space` is the proposal's, for dead-letter context. */
export async function applyProposalActions(
	deps: KeyedProposalActionsDeps,
	actions: ProposalAction[],
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> {
	if (actions.length === 0) return "ignored"
	return applyKeyedRecord(deps, FAMILY, actions.flatMap(proposalActionIntents), {...meta, space: (actions[0] as ProposalAction).proposal})
}

export const headerText = (h: unknown): string | undefined => {
	const v = Array.isArray(h) ? h[0] : h
	if (v === undefined || v === null) return undefined
	return Buffer.isBuffer(v) || v instanceof Uint8Array ? Buffer.from(v).toString("utf8") : String(v)
}

export async function handleProposalActionMessage(
	deps: KeyedProposalActionsDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	const type = headerText(message.headers?.["event-type"])
	if (type !== PROPOSAL_CREATED && type !== PROPOSAL_UPDATED) return "ignored"
	if (!message.value) return "ignored"
	let actions: ProposalAction[]
	try {
		actions = parseProposalActions(message.value)
	} catch (e) {
		return deadLetterDecode(deps, "space.governance", partition, message.offset, e)
	}
	return applyProposalActions(deps, actions, {partition, offset: message.offset})
}

/** Whether the keyed proposal-actions writer runs: GEODB_PROPOSAL_ACTIONS only takes effect with GEODB_RECORD_KEYS. */
export const proposalActionsKeyedEnabled = (c: {recordKeys: boolean; proposalActions: {enabled: boolean}}): boolean =>
	c.proposalActions.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const proposalActionsOffsetsDir = (dataDir: string) => `${dataDir}/feeder/proposal-actions-keyed`

/** A `proposal_actions` row joined to its version's block, as the backfill reads it (pgSource.readProposalActionRows). */
export type ProposalActionRow = {
	proposal_id: string
	index: number
	/** `proposal_versions.version_created_at_block`, text. */
	block: string
	action_type: string
	target_id: string | null
	content_uri: string | null
	/** Lowercase hex of the bytea, or null. */
	metadata: string | null
	content_id: string | null
	quorum: string | null
	duration: string | null
	partial_percentage_support_threshold: string | null
	universal_percentage_support_threshold: string | null
	flat_support_threshold: string | null
	disable_fast_path_access_for_new_members: boolean | null
	execution_grace_period: string | null
}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()
const I64 = /^-?\d+$/
const ACTION_TYPES = new Set([
	"AddMember", "RemoveMember", "AddEditor", "RemoveEditor", "UnflagEditor", "Publish", "Flag", "Unflag", "UpdateVotingSettings",
	"SubspaceVerified", "SubspaceUnverified", "SubspaceRelated", "SubspaceUnrelated", "SubspaceTopicDeclared", "SubspaceTopicRemoved",
	"SetTopic", "UnsetTopic", "Unknown",
])

const big = (k: string, v: string | null): bigint | undefined => {
	if (v === null) return undefined
	if (!I64.test(v) || BigInt.asIntN(64, BigInt(v)) !== BigInt(v)) throw new Error(`proposal_actions: ${k} ${JSON.stringify(v)}`)
	return BigInt(v)
}
const opt = <K extends string, V>(k: K, v: V | undefined | null) => (v === undefined || v === null ? {} : ({[k]: v} as Record<K, V>))

/** A Postgres row as the action an event wrote. Throws on a value no event could have written. */
export function proposalActionFromRow(r: ProposalActionRow): ProposalAction {
	if (!ACTION_TYPES.has(r.action_type)) throw new Error(`proposal_actions: action_type ${JSON.stringify(r.action_type)}`)
	if (!Number.isInteger(r.index) || r.index < 0) throw new Error(`proposal_actions: index ${r.index}`)
	const block = big("block", r.block)
	if (block === undefined) throw new Error("proposal_actions: no version block")
	return {
		proposal: compact(r.proposal_id),
		block: BigInt.asUintN(64, block),
		index: r.index,
		actionType: r.action_type,
		...opt("target", r.target_id === null ? null : compact(r.target_id)),
		...opt("contentUri", r.content_uri),
		...opt("metadata", r.metadata),
		...opt("contentId", r.content_id),
		...opt("quorum", big("quorum", r.quorum)),
		...opt("duration", big("duration", r.duration)),
		...opt("partial", big("partial", r.partial_percentage_support_threshold)),
		...opt("universal", big("universal", r.universal_percentage_support_threshold)),
		...opt("flat", big("flat", r.flat_support_threshold)),
		...opt("grace", big("grace", r.execution_grace_period)),
		...opt("disableFastPath", r.disable_fast_path_access_for_new_members),
	}
}

/**
 * Write a snapshot of `proposal_actions`. Rerunnable: a held row is written again to the same ids. A row Postgres
 * holds that no event could have written is dead-lettered (counted, the Job fails at the end) rather than invented.
 */
export const backfillProposalActions = (
	deps: KeyedProposalActionsDeps,
	rows: AsyncIterable<ProposalActionRow> | Iterable<ProposalActionRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.governance", fromRow: proposalActionFromRow, apply: (d, a) => applyProposalActions(d, [a], {partition: -1, offset: "backfill"})},
		progress,
	)
