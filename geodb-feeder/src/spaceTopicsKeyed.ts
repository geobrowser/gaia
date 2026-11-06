/**
 * Space topics as natural-key records (geo-db plan 10.12 K1-K5; the `governance` family), behind GEODB_SPACE_TOPICS,
 * which takes effect only with GEODB_RECORD_KEYS (spaceTopicsKeyedEnabled). With either off nothing here runs. A flag of
 * its own: the backfill must run before the consumer first starts.
 *
 * Source: `<env>.space.topics`, messages whose `event-type` header is TOPIC_DECLARED (or absent, which kg-indexer reads
 * as declared) or TOPIC_REMOVED (HermesTopicDeclared / HermesTopicRemoved, space_id field 1 and topic_id field 2, both
 * 16 bytes), the events kg-indexer folds into `spaces.topic_id` (storage.rs update_space_topic and clear_space_topic).
 * Every other event type is ignored. One record per Space, in the `governance` system Space:
 * - record entity: `recordEntityId(spaceTopic, [id(space)])`;
 * - key column (RECORD_COLUMNS.spaceTopic.space): `add` + `set_active(true)`, a reference to the Space's entity;
 * - mutable column `topic`: a `put` of a reference to the topic entity under
 *   `recordStatementId(spaceTopic, [record, topicProperty, id(topic)])`. A put closes the cell's other active
 *   statements, so declaring a new topic replaces the old one (`UPDATE spaces SET topic_id`), and the value in the id
 *   makes a replay a no-op and A -> B -> A end on A.
 * - TOPIC_REMOVED is `set_active(that statement, false)`. kg-indexer clears the column only when the removed topic is
 *   the current one; here the statement of a topic that is not current is already closed, so the same guard falls out
 *   with no read.
 *
 * `spaces.address` is its own consumer (spaceAddressesKeyed.ts). A topic event for a
 * Space Postgres lacks is written anyway (kg-indexer's UPDATE would match no row); the gate reports such keys, not as a
 * failure. A space_id or topic_id that is not 16 bytes is dead-lettered, as kg-indexer's Uuid::from_slice errors.
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
import {idPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent} from "./translate.js"

const KIND = RECORD_KINDS.spaceTopic.id
const FAMILY = RECORD_KINDS.spaceTopic.family
const COL = RECORD_COLUMNS.spaceTopic

export const TOPIC_DECLARED = "TOPIC_DECLARED"
export const TOPIC_REMOVED = "TOPIC_REMOVED"

export type SpaceTopicEvent = {kind: "declared" | "removed"; space: string; topic: string}

/** Decode HermesTopicDeclared / HermesTopicRemoved. Throws where kg-indexer errors (an id that is not 16 bytes). */
export function parseTopicEvent(kind: "declared" | "removed", b: Uint8Array): SpaceTopicEvent {
	const f = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(b)) f.set(n, v)
	const id = (n: number, what: string): string => {
		const v = f.get(n) ?? new Uint8Array(0)
		if (!(v instanceof Uint8Array) || v.length !== 16) throw new Error(`space topic: ${what} is not 16 bytes`)
		return Buffer.from(v).toString("hex")
	}
	return {kind, space: id(1, "space_id"), topic: id(2, "topic_id")}
}

/** The record entity of one Space's topic. */
export const spaceTopicRecordId = (space: string): string => recordEntityId(KIND, [idPart(space)])

const topicStatementId = (record: string, topic: string) => recordStatementId(KIND, [idPart(record), idPart(COL.topic), idPart(topic)])

/** The intents for one event. Pure, and depends on the event alone. */
export function spaceTopicIntents(e: SpaceTopicEvent): string[] {
	const record = spaceTopicRecordId(e.space)
	const topicRef = cid("entity", e.topic)
	const sid = topicStatementId(record, topicRef)
	if (e.kind === "removed") return [`.{ .set_active = .{ .statement = "${sid}", .active = false } }`]
	const space = cid("entity", e.space)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.space), idPart(space)]), record, COL.space, `.{ .reference = "${space}" }`),
		putIntent({id: sid, entity: record, property: COL.topic, value: `.{ .reference = "${topicRef}" }`}),
	]
}

export type KeyedSpaceTopicsDeps = KeyedRecordDeps

export async function applySpaceTopic(
	deps: KeyedSpaceTopicsDeps,
	e: SpaceTopicEvent,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> {
	return applyKeyedRecord(deps, FAMILY, spaceTopicIntents(e), {...meta, space: e.space})
}

const headerText = (h: unknown): string | undefined => {
	const v = Array.isArray(h) ? h[0] : h
	if (v === undefined || v === null) return undefined
	return Buffer.isBuffer(v) || v instanceof Uint8Array ? Buffer.from(v).toString("utf8") : String(v)
}

export async function handleSpaceTopicMessage(
	deps: KeyedSpaceTopicsDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	const t = headerText(message.headers?.["event-type"])
	// kg-indexer: a missing header means declared.
	const kind = t === undefined || t === TOPIC_DECLARED ? "declared" : t === TOPIC_REMOVED ? "removed" : null
	if (kind === null || !message.value) return "ignored"
	let e: SpaceTopicEvent
	try {
		e = parseTopicEvent(kind, message.value)
	} catch (err) {
		return deadLetterDecode(deps, "space.topics", partition, message.offset, err)
	}
	return applySpaceTopic(deps, e, {partition, offset: message.offset})
}

/** Whether the keyed topic writer runs: GEODB_SPACE_TOPICS only takes effect with GEODB_RECORD_KEYS. */
export const spaceTopicsKeyedEnabled = (c: {recordKeys: boolean; spaceTopics: {enabled: boolean}}): boolean => c.spaceTopics.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const spaceTopicsOffsetsDir = (dataDir: string) => `${dataDir}/feeder/space-topics-keyed`

/** A `spaces` row with a topic, as the backfill reads it (pgSource.readSpaceTopicRows). */
export type SpaceTopicRow = {space_id: string; topic_id: string}

const UUID = /^[0-9a-f]{32}$/

/** A Postgres row as the declared event that set it. Throws on a value no event could have written. */
export function spaceTopicFromRow(r: SpaceTopicRow): SpaceTopicEvent {
	const space = String(r.space_id).replaceAll("-", "").toLowerCase()
	const topic = String(r.topic_id).replaceAll("-", "").toLowerCase()
	if (!UUID.test(space) || !UUID.test(topic)) throw new Error(`spaces: topic row ${JSON.stringify(r)}`)
	return {kind: "declared", space, topic}
}

/** Write a snapshot of `spaces.topic_id`. Rerunnable: a held row is written again to the same ids. */
export const backfillSpaceTopics = (
	deps: KeyedSpaceTopicsDeps,
	rows: AsyncIterable<SpaceTopicRow> | Iterable<SpaceTopicRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.topics", fromRow: spaceTopicFromRow, apply: (d, e) => applySpaceTopic(d, e, {partition: -1, offset: "backfill"})},
		progress,
	)
