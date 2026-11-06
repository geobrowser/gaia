/**
 * Subspaces and subspace topics as natural-key edges (geo-db plan 10.12; plan 10.11 GAP-7, the `governance` family),
 * behind GEODB_SUBSPACES, which takes effect only with GEODB_RECORD_KEYS (subspacesKeyedEnabled). With either off
 * nothing here runs. A flag of its own: the backfill must run before the consumer first starts.
 *
 * Source: `<env>.space.trust.extensions`, messages whose `event-type` header is `TRUST_EXTENSION`
 * (HermesSpaceTrustExtension, hermes-schema/proto/space.proto), which kg-indexer folds into `subspaces` and
 * `subspace_topics` (handlers/subspaces.rs). Six variants: verified / related (Space to Space), subtopic (Space to
 * topic), and a removal of each. One edge per Postgres primary key, in the `governance` system Space, the same
 * model as membershipKeyed.ts:
 * - subject: the parent Space's own entity, `cid("entity", source_space_id)`;
 * - property: RECORD_COLUMNS.subspace.verified or .related for a `subspaces` row (its `type` is part of the primary
 *   key, so it is part of the edge's identity), RECORD_COLUMNS.subspaceTopic.topic for a `subspace_topics` row;
 * - value: a reference to `cid("entity", target)`, the child Space or the topic entity;
 * - statement id: `recordStatementId(kind, [subject, property, target])`, kind `subspace` or `subspaceTopic`.
 *
 * An insert is `create_entity(subject)`, `add(sid, edge)`, `set_active(sid, true)`; a removal (kg-indexer deletes the
 * row) is `set_active(sid, false)`. Each is idempotent and a retract of an id the store never saw is admitted (K1), so
 * every event's intents are fixed by the event alone and a replay converges on the last event per key. A reader
 * takes the ACTIVE edges as Postgres's rows.
 *
 * Accepted and rejected where handle_trust_extension does: a source or target id that is not 16 bytes is a decode
 * failure (dead-lettered); a message with no recognised variant is ignored (kg-indexer skips it).
 *
 * Not mirrored, in either direction: kg-indexer drops the WHOLE block when any write in it fails permanently (a
 * `subspaces_*_spaces_id_fk` violation is one cause), so geo-db can hold an edge Postgres lacks. Treat any difference
 * as expected and investigate it, never as a bug in either side (subspacesGate.ts reports it as geo-db-only).
 * Ordering: events for one (parent, child, type) must be applied in order; hermes keys them by source Space.
 */
import {cid} from "./ids.js"
import {parseFields} from "./hermes.js"
import {
	applyKeyedRecord,
	backfillKeyedRows,
	deadLetterDecode,
	type KeyedBackfillResult,
	type KeyedOutcome,
	type KeyedRecordDeps,
} from "./keyedRecord.js"
import {idPart, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"

export const TRUST_EXTENSION = "TRUST_EXTENSION"
const FAMILY = RECORD_KINDS.subspace.family

export type SubspaceType = "verified" | "related" | "topic"
export type SubspaceEvent = {kind: "add" | "remove"; space: string; target: string; type: SubspaceType}

export type KeyedSubspacesDeps = KeyedRecordDeps

const hex = (v: Uint8Array) => Buffer.from(v).toString("hex")

/** Oneof member field number -> (add or remove, edge type). Proto field 5 is `meta`, not a member. */
const VARIANTS: Record<number, {kind: "add" | "remove"; type: SubspaceType}> = {
	2: {kind: "add", type: "verified"},
	3: {kind: "add", type: "related"},
	4: {kind: "add", type: "topic"},
	6: {kind: "remove", type: "verified"},
	7: {kind: "remove", type: "related"},
	8: {kind: "remove", type: "topic"},
}

/**
 * HermesSpaceTrustExtension: source_space_id = 1 and a oneof whose members each hold one 16-byte target in field 1.
 * Null when no member is present; throws on a source or target that is not 16 bytes or a member that is not a message.
 */
export function parseTrustExtension(payload: Uint8Array): SubspaceEvent | null {
	let space: string | undefined
	let found: SubspaceEvent | null = null
	let pending: {kind: "add" | "remove"; type: SubspaceType; target: string | undefined} | null = null
	for (const [f, v] of parseFields(payload)) {
		if (f === 1) {
			if (!(v instanceof Uint8Array)) throw new Error("trust extension source is not bytes")
			space = hex(v)
		} else if (VARIANTS[f]) {
			if (!(v instanceof Uint8Array)) throw new Error(`trust extension member ${f} is not a message`)
			let target: string | undefined
			for (const [g, w] of parseFields(v)) if (g === 1 && w instanceof Uint8Array) target = hex(w)
			pending = {...(VARIANTS[f] as {kind: "add" | "remove"; type: SubspaceType}), target}
		}
	}
	if (space?.length !== 32) throw new Error("trust extension without a 16-byte source space")
	if (pending) {
		if (pending.target?.length !== 32) throw new Error("trust extension without a 16-byte target")
		found = {kind: pending.kind, space, target: pending.target, type: pending.type}
	}
	return found
}

const kindOf = (type: SubspaceType) => (type === "topic" ? RECORD_KINDS.subspaceTopic.id : RECORD_KINDS.subspace.id)
const propertyOf = (type: SubspaceType) => (type === "topic" ? RECORD_COLUMNS.subspaceTopic.topic : RECORD_COLUMNS.subspace[type])

export const subspaceSubject = (space: string): string => cid("entity", space)

/** The statement id of one edge: (parent Space entity, type's property, target entity). */
export function subspaceStatementId(space: string, target: string, type: SubspaceType): string {
	return recordStatementId(kindOf(type), [idPart(subspaceSubject(space)), idPart(propertyOf(type)), idPart(cid("entity", target))])
}

/** The intents for one event. Pure, and depends on the event alone. */
export function subspaceIntents(ev: SubspaceEvent): string[] {
	const sid = subspaceStatementId(ev.space, ev.target, ev.type)
	if (ev.kind === "remove") return [`.{ .set_active = .{ .statement = "${sid}", .active = false } }`]
	const subject = subspaceSubject(ev.space)
	return [
		`.{ .create_entity = "${subject}" }`,
		`.{ .add = .{ .id = "${sid}", .entity = "${subject}", .property = "${propertyOf(ev.type)}", .value = .{ .reference = "${cid("entity", ev.target)}" } } }`,
		`.{ .set_active = .{ .statement = "${sid}", .active = true } }`,
	]
}

export const applySubspaceEvent = (deps: KeyedSubspacesDeps, ev: SubspaceEvent, meta: {partition: number; offset: string}): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, FAMILY, subspaceIntents(ev), {...meta, space: ev.space})

const headerText = (h: unknown): string | undefined => {
	const v = Array.isArray(h) ? h[0] : h
	if (v === undefined || v === null) return undefined
	return Buffer.isBuffer(v) || v instanceof Uint8Array ? Buffer.from(v).toString("utf8") : String(v)
}

export async function handleSubspaceMessage(
	deps: KeyedSubspacesDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	if (headerText(message.headers?.["event-type"]) !== TRUST_EXTENSION) return "ignored"
	if (!message.value) return "ignored"
	let ev: SubspaceEvent | null
	try {
		ev = parseTrustExtension(message.value)
	} catch (e) {
		return deadLetterDecode(deps, "space.trust.extensions", partition, message.offset, e)
	}
	if (!ev) return "ignored"
	return applySubspaceEvent(deps, ev, {partition, offset: message.offset})
}

/** Whether the keyed subspaces writer runs: GEODB_SUBSPACES only takes effect with GEODB_RECORD_KEYS. */
export const subspacesKeyedEnabled = (c: {recordKeys: boolean; subspaces: {enabled: boolean}}): boolean => c.subspaces.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const subspacesOffsetsDir = (dataDir: string) => `${dataDir}/feeder/subspaces-keyed`

/** A `subspaces` or `subspace_topics` row as the backfill reads it (pgSource.readSubspaceRows). */
export type SubspaceRow = {parent_space_id: string; target_id: string; type: string}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()

/** The event a row stands for: an add. A type no event could have written, or a malformed id, throws (dead-lettered). */
export function subspaceFromRow(r: SubspaceRow): SubspaceEvent {
	if (r.type !== "verified" && r.type !== "related" && r.type !== "topic") throw new Error(`unknown subspace type ${r.type}`)
	const space = compact(r.parent_space_id)
	const target = compact(r.target_id)
	if (space.length !== 32 || target.length !== 32) throw new Error("subspace row without 16-byte ids")
	return {kind: "add", space, target, type: r.type}
}

/**
 * Write a snapshot as adds. Rerunnable; it never retracts, so a removal the stream carries after the snapshot is
 * applied by the stream (the Job refuses once the consumer has offsets, keyedBackfillGate).
 */
export const backfillSubspaces = (
	deps: KeyedSubspacesDeps,
	rows: AsyncIterable<SubspaceRow> | Iterable<SubspaceRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.trust.extensions", fromRow: subspaceFromRow, apply: (d, e) => applySubspaceEvent(d, e, {partition: -1, offset: "backfill"})},
		progress,
	)
