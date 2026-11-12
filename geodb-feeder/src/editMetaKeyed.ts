/**
 * Per-edit metadata as natural-key records (geo-db plan 10.22 section 22, KG-EDIT-BLOCK-META-HOME), behind
 * GEODB_EDIT_META, which takes effect only with GEODB_RECORD_KEYS (editMetaKeyedEnabled). With either off nothing here runs.
 *
 * Source: `<env>.knowledge.edits` (HermesEdit, hermes-schema/proto/knowledge.proto). kg-indexer writes one
 * `edit_versions` row per edit that carries a `meta` sub-message (kg-indexer main.rs `if let Some(meta)`), with
 * `block_number`/`sequence` from that meta and `name` from the edit. The block is in the payload, not in a Kafka
 * header (kg-indexer reads only `event-type` and `event-id`). This writer keeps the same three columns:
 * - record entity: `recordEntityId(editMeta, [id(editId)])`, one record per edit id, in the `governance` Space;
 * - key column: the edit id as text (RECORD_COLUMNS.editMeta.editId);
 * - mutable columns, each a `put` with the value in the statement id (a replay is a no-op): `block`, `sequence`, and
 *   `name` when the edit has one. The name is cut to 256 bytes on a char boundary, as kg-indexer cuts it.
 *
 * An edit with no `meta` is ignored (no edit_versions row in kg-indexer either). An edit with no id cannot be keyed and
 * is dead-lettered. The envelope is all that is read: the payload is never decoded.
 */
import {parseHermesEdit} from "./hermes.js"
import {applyKeyedRecord, deadLetterDecode, type KeyedOutcome, type KeyedRecordDeps, keyColumn} from "./keyedRecord.js"
import {idPart, intPart, recordEntityId, recordStatementId, textPart} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent, zigString} from "./translate.js"

const KIND = RECORD_KINDS.editMeta.id
const FAMILY = RECORD_KINDS.editMeta.family
const COL = RECORD_COLUMNS.editMeta

/** kg-indexer's cut on `edit_versions.name` (extract_edit_metadata). */
export const EDIT_NAME_MAX_BYTES = 256

export type EditMeta = {editId: string; block: number; sequence: number; name: string | null}

export type KeyedEditMetaDeps = KeyedRecordDeps

/** `name` cut to at most 256 bytes without splitting a character; an empty name is no name. */
export function cutEditName(name: string | undefined): string | null {
	if (!name) return null
	let out = ""
	let bytes = 0
	for (const ch of name) {
		const n = Buffer.byteLength(ch, "utf8")
		if (bytes + n > EDIT_NAME_MAX_BYTES) break
		out += ch
		bytes += n
	}
	return out === "" ? null : out
}

/** The record entity of one edit. */
export const editMetaRecordId = (editId: string): string => recordEntityId(KIND, [idPart(editId)])

const numPut = (record: string, property: string, v: number) =>
	putIntent({
		id: recordStatementId(KIND, [idPart(record), idPart(property), intPart(v)]),
		entity: record,
		property,
		value: `.{ .number = ${v} }`,
	})

/** The intents for one edit. Pure, and depends on the edit's envelope alone. */
export function editMetaIntents(m: EditMeta): string[] {
	const record = editMetaRecordId(m.editId)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(
			recordStatementId(KIND, [idPart(record), idPart(COL.editId), textPart(m.editId)]),
			record,
			COL.editId,
			`.{ .text = ${zigString(m.editId)} }`,
		),
		numPut(record, COL.block, m.block),
		numPut(record, COL.sequence, m.sequence),
		...(m.name === null
			? []
			: [
					putIntent({
						id: recordStatementId(KIND, [idPart(record), idPart(COL.name), textPart(m.name)]),
						entity: record,
						property: COL.name,
						value: `.{ .text = ${zigString(m.name)} }`,
					}),
				]),
	]
}

export async function handleEditMetaMessage(
	deps: KeyedEditMetaDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	if (!message.value) return "ignored"
	let m: EditMeta
	try {
		const he = parseHermesEdit(message.value)
		if (!he.hasMeta) return "ignored"
		if (!he.id || he.id.length !== 32) throw new Error("edit has no 16-byte id")
		m = {editId: he.id, block: he.blockNumber ?? 0, sequence: he.sequence ?? 0, name: cutEditName(he.name)}
	} catch (e) {
		return deadLetterDecode(deps, "knowledge.edits", partition, message.offset, e)
	}
	// Every record lives in the one governance Space; a founding failure is thrown, never dead-lettered.
	return applyKeyedRecord(deps, FAMILY, editMetaIntents(m), {partition, offset: message.offset, space: m.editId})
}

/** Whether the keyed edit-meta writer runs: GEODB_EDIT_META only takes effect with GEODB_RECORD_KEYS. */
export const editMetaKeyedEnabled = (c: {recordKeys: boolean; editMeta: {enabled: boolean}}): boolean => c.editMeta.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR. */
export const editMetaOffsetsDir = (dataDir: string) => `${dataDir}/feeder/edit-meta-keyed`
