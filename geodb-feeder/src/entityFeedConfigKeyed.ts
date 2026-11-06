/**
 * The operator-edited entity-feed tables as natural-key records (geo-db plan 10.22 section 6; the `config` family),
 * behind GEODB_ENTITY_FEED_CONFIG, which takes effect only with GEODB_RECORD_KEYS (entityFeedConfigKeyedEnabled).
 * With either off nothing here runs.
 *
 * - `entity_type_weights`: one record per `type_id`, mutable `weight` (decimal) and `note`.
 * - `entity_type_exclusions`: one record per excluded `type_id`, mutable `note`.
 * - `entity_feed_blocklist`: one record per blocked `entity_id`, mutable `reason`.
 *
 * Plan 10.22 section 6 sketched the last two as edge rows; they are records here (a presence record with one key column)
 * so they reuse the one proven keyed-record writer, gate and backfill Job. The plan's edge shape stays open for the
 * moment a reader needs a traversal.
 *
 * A mirror, not a move. Postgres stays the source and its readers (the plpgsql ranking and feed functions of
 * api/drizzle/0073-0075) are unchanged. Only operator SQL writes these tables, so there is no consumer: the operator
 * edit path is "edit the row in Postgres, rerun the backfill Job" (`entity-feed-config-backfill-main.ts`), which is
 * the only writer, rerunnable, and writes the same ids again. `updated_at` / `created_at` are not mirrored. A row
 * deleted from Postgres, or a note changed to null, is not cleared in geo-db; the gate (entityFeedConfigGate.ts) reports it.
 */
import {applyKeyedRecord, backfillKeyedRows, type KeyedBackfillResult, type KeyedOutcome, type KeyedRecordDeps, keyColumn} from "./keyedRecord.js"
import {idPart, intPart, recordEntityId, recordStatementId, textPart} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {floatToDecimal, putIntent, zigString} from "./translate.js"

const W = RECORD_KINDS.entityTypeWeight
const X = RECORD_KINDS.entityTypeExclusion
const B = RECORD_KINDS.entityFeedBlock
const WC = RECORD_COLUMNS.entityTypeWeight
const XC = RECORD_COLUMNS.entityTypeExclusion
const BC = RECORD_COLUMNS.entityFeedBlock

const UUID = /^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$/i
const dashed = (u: string): string => {
	const c = u.replaceAll("-", "").toLowerCase()
	return `${c.slice(0, 8)}-${c.slice(8, 12)}-${c.slice(12, 16)}-${c.slice(16, 20)}-${c.slice(20)}`
}

export type EntityTypeWeight = {type: string; weight: number; note: string | null}
export type EntityTypeExclusion = {type: string; note: string | null}
export type EntityFeedBlock = {entity: string; reason: string | null}

export const entityTypeWeightRecordId = (type: string): string => recordEntityId(W.id, [idPart(type)])
export const entityTypeExclusionRecordId = (type: string): string => recordEntityId(X.id, [idPart(type)])
export const entityFeedBlockRecordId = (entity: string): string => recordEntityId(B.id, [idPart(entity)])

type Kind = {id: string}
const keyIntents = (kind: Kind, record: string, property: string, uuid: string) =>
	keyColumn(
		recordStatementId(kind.id, [idPart(record), idPart(property), textPart(uuid)]),
		record,
		property,
		`.{ .text = ${zigString(uuid)} }`,
	)
const textPut = (kind: Kind, record: string, property: string, v: string) =>
	putIntent({
		id: recordStatementId(kind.id, [idPart(record), idPart(property), textPart(v)]),
		entity: record,
		property,
		value: `.{ .text = ${zigString(v)} }`,
	})

export function entityTypeWeightIntents(w: EntityTypeWeight): string[] {
	const record = entityTypeWeightRecordId(w.type)
	const d = floatToDecimal(w.weight)
	if (!d) throw new Error(`entity_type_weights ${w.type}: weight ${w.weight}`)
	const dec = `${d[0]},${d[1]}`
	return [
		`.{ .create_entity = "${record}" }`,
		...keyIntents(W, record, WC.typeId, dashed(w.type)),
		putIntent({
			id: recordStatementId(W.id, [idPart(record), idPart(WC.weight), textPart(dec)]),
			entity: record,
			property: WC.weight,
			value: `.{ .decimal = "${dec}" }`,
		}),
		...(w.note === null ? [] : [textPut(W, record, WC.note, w.note)]),
	]
}

export function entityTypeExclusionIntents(x: EntityTypeExclusion): string[] {
	const record = entityTypeExclusionRecordId(x.type)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyIntents(X, record, XC.typeId, dashed(x.type)),
		...(x.note === null ? [] : [textPut(X, record, XC.note, x.note)]),
	]
}

export function entityFeedBlockIntents(b: EntityFeedBlock): string[] {
	const record = entityFeedBlockRecordId(b.entity)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyIntents(B, record, BC.entityId, dashed(b.entity)),
		...(b.reason === null ? [] : [textPut(B, record, BC.reason, b.reason)]),
	]
}

export type KeyedEntityFeedConfigDeps = KeyedRecordDeps

const meta = {partition: -1, offset: "backfill"}
export const applyEntityTypeWeight = (deps: KeyedEntityFeedConfigDeps, w: EntityTypeWeight): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, W.family, entityTypeWeightIntents(w), {...meta, space: "entity_type_weights"})
export const applyEntityTypeExclusion = (deps: KeyedEntityFeedConfigDeps, x: EntityTypeExclusion): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, X.family, entityTypeExclusionIntents(x), {...meta, space: "entity_type_exclusions"})
export const applyEntityFeedBlock = (deps: KeyedEntityFeedConfigDeps, b: EntityFeedBlock): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, B.family, entityFeedBlockIntents(b), {...meta, space: "entity_feed_blocklist"})

/** Whether the keyed writer runs: GEODB_ENTITY_FEED_CONFIG only takes effect with GEODB_RECORD_KEYS. */
export const entityFeedConfigKeyedEnabled = (c: {recordKeys: boolean; entityFeedConfig: {enabled: boolean}}): boolean =>
	c.entityFeedConfig.enabled && c.recordKeys

/** The backfill gate's offsets directory under DATA_DIR (there is no consumer, so it stays empty). */
export const entityFeedConfigOffsetsDir = (dataDir: string) => `${dataDir}/feeder/entity-feed-config-keyed`

/** Rows as the backfill reads them (pgSource): uuids and the numeric as text. */
export type EntityTypeWeightRow = {type_id: string; weight: string; note: string | null}
export type EntityTypeExclusionRow = {type_id: string; note: string | null}
export type EntityFeedBlockRow = {entity_id: string; reason: string | null}

const uuid = (table: string, col: string, v: unknown): string => {
	if (typeof v !== "string" || !UUID.test(v)) throw new Error(`${table}: ${col} ${JSON.stringify(v)}`)
	return dashed(v)
}
const optText = (table: string, col: string, v: unknown): string | null => {
	if (v !== null && typeof v !== "string") throw new Error(`${table}: ${col} ${JSON.stringify(v)}`)
	return v
}

/** Throws on what the table's check (0.1 < weight <= 10) would have refused. */
export function entityTypeWeightFromRow(r: EntityTypeWeightRow): EntityTypeWeight {
	const type = uuid("entity_type_weights", "type_id", r.type_id)
	const text = r.weight
	const weight = typeof text === "string" && text.trim() !== "" ? Number(text) : Number.NaN
	if (!Number.isFinite(weight) || !(weight > 0.1) || weight > 10) throw new Error(`entity_type_weights ${type}: weight ${JSON.stringify(text)}`)
	return {type, weight, note: optText("entity_type_weights", "note", r.note)}
}
export const entityTypeExclusionFromRow = (r: EntityTypeExclusionRow): EntityTypeExclusion => ({
	type: uuid("entity_type_exclusions", "type_id", r.type_id),
	note: optText("entity_type_exclusions", "note", r.note),
})
export const entityFeedBlockFromRow = (r: EntityFeedBlockRow): EntityFeedBlock => ({
	entity: uuid("entity_feed_blocklist", "entity_id", r.entity_id),
	reason: optText("entity_feed_blocklist", "reason", r.reason),
})

type Rows<R> = AsyncIterable<R> | Iterable<R>
export const backfillEntityTypeWeights = (deps: KeyedEntityFeedConfigDeps, rows: Rows<EntityTypeWeightRow>, progress?: (done: number) => void) =>
	backfillKeyedRows(deps, rows, {topic: "entity_type_weights", fromRow: entityTypeWeightFromRow, apply: applyEntityTypeWeight}, progress)
export const backfillEntityTypeExclusions = (deps: KeyedEntityFeedConfigDeps, rows: Rows<EntityTypeExclusionRow>, progress?: (done: number) => void) =>
	backfillKeyedRows(deps, rows, {topic: "entity_type_exclusions", fromRow: entityTypeExclusionFromRow, apply: applyEntityTypeExclusion}, progress)
export const backfillEntityFeedBlocks = (deps: KeyedEntityFeedConfigDeps, rows: Rows<EntityFeedBlockRow>, progress?: (done: number) => void) =>
	backfillKeyedRows(deps, rows, {topic: "entity_feed_blocklist", fromRow: entityFeedBlockFromRow, apply: applyEntityFeedBlock}, progress)

/** All three tables, weights first; the totals are summed. */
export async function backfillEntityFeedConfig(
	deps: KeyedEntityFeedConfigDeps,
	rows: {weights: Rows<EntityTypeWeightRow>; exclusions: Rows<EntityTypeExclusionRow>; blocks: Rows<EntityFeedBlockRow>},
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> {
	const a = await backfillEntityTypeWeights(deps, rows.weights, progress)
	const b = await backfillEntityTypeExclusions(deps, rows.exclusions, progress)
	const c = await backfillEntityFeedBlocks(deps, rows.blocks, progress)
	return {rows: a.rows + b.rows + c.rows, applied: a.applied + b.applied + c.applied, deadLetter: a.deadLetter + b.deadLetter + c.deadLetter}
}
