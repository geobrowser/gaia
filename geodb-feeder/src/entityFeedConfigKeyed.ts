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
 * deleted from Postgres, or a note changed to null, is cleared by the next backfill run: a null note or reason is
 * `unset`, and a key that the previous run held and this run's complete read did not return has its key column and
 * fields `unset` (the record entity stays, empty, and a returning row writes it live again). "Held" is a manifest of
 * keys per table under the offsets directory (HeldKeys), written only after a run read all of Postgres; geo-db cannot
 * be enumerated by key from the Job. The first run after this lands has no manifest, so a row already deleted before
 * it is found only by the gate (entityFeedConfigGate.ts), which counts it as stale.
 */
import {existsSync, mkdirSync, readFileSync, renameSync, writeFileSync} from "node:fs"
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

/** Closes every active statement of one cell; a no-op on a cell that holds none. */
const unset = (record: string, property: string): string => `.{ .unset = .{ .entity = "${record}", .property = "${property}" } }`

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
		...(w.note === null ? [unset(record, WC.note)] : [textPut(W, record, WC.note, w.note)]),
	]
}

export function entityTypeExclusionIntents(x: EntityTypeExclusion): string[] {
	const record = entityTypeExclusionRecordId(x.type)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyIntents(X, record, XC.typeId, dashed(x.type)),
		...(x.note === null ? [unset(record, XC.note)] : [textPut(X, record, XC.note, x.note)]),
	]
}

export function entityFeedBlockIntents(b: EntityFeedBlock): string[] {
	const record = entityFeedBlockRecordId(b.entity)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyIntents(B, record, BC.entityId, dashed(b.entity)),
		...(b.reason === null ? [unset(record, BC.reason)] : [textPut(B, record, BC.reason, b.reason)]),
	]
}

/** The keys (dashed uuids) each table held after the last complete run: what a later run may clear. */
export type HeldKeys = {
	load(table: string): Set<string>
	save(table: string, keys: Set<string>): void
}
export type KeyedEntityFeedConfigDeps = KeyedRecordDeps & {held?: HeldKeys}

export const memoryHeldKeys = (): HeldKeys => {
	const m = new Map<string, Set<string>>()
	return {load: (t) => new Set(m.get(t) ?? []), save: (t, k) => void m.set(t, new Set(k))}
}

/** One JSON file a table under `dir`, replaced by rename so a crash leaves the old manifest. */
export function fileHeldKeys(dir: string): HeldKeys {
	const path = (t: string) => `${dir}/held-${t}.json`
	return {
		load: (t) => (existsSync(path(t)) ? new Set(JSON.parse(readFileSync(path(t), "utf8")) as string[]) : new Set()),
		save(t, keys) {
			mkdirSync(dir, {recursive: true})
			writeFileSync(`${path(t)}.tmp`, JSON.stringify([...keys].sort()))
			renameSync(`${path(t)}.tmp`, path(t))
		},
	}
}

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

type Clearing = {table: string; family: Parameters<typeof applyKeyedRecord>[1]; cells: (key: string) => [string, string[]]}
const clearing: Clearing[] = [
	{table: "entity_type_weights", family: W.family, cells: (k) => [entityTypeWeightRecordId(k), [WC.typeId, WC.weight, WC.note]]},
	{table: "entity_type_exclusions", family: X.family, cells: (k) => [entityTypeExclusionRecordId(k), [XC.typeId, XC.note]]},
	{table: "entity_feed_blocklist", family: B.family, cells: (k) => [entityFeedBlockRecordId(k), [BC.entityId, BC.reason]]},
]

/** The dashed key of a row's first column, or null when it is not a uuid (such a row dead-letters, and is not a deletion). */
const rowKey = (r: unknown, col: string): string | null => {
	const v = (r as Record<string, unknown>)[col]
	return typeof v === "string" && UUID.test(v) ? dashed(v) : null
}

/** Pass the rows through, noting every key Postgres returned. */
async function* noting<R>(rows: Rows<R>, col: string, into: Set<string>): AsyncGenerator<R> {
	for await (const r of rows) {
		const k = rowKey(r, col)
		if (k) into.add(k)
		yield r
	}
}

/** Unset every cell of the record a deleted row held: the key column too, so the gate and the readers see it gone. */
async function clearRecord(deps: KeyedEntityFeedConfigDeps, c: Clearing, key: string): Promise<KeyedOutcome> {
	const [record, props] = c.cells(key)
	return applyKeyedRecord(deps, c.family, props.map((p) => unset(record, p)), {...meta, space: c.table})
}

/**
 * All three tables, weights first; the totals are summed. With `deps.held`, a key the last complete run held and this
 * complete read did not return is cleared, then the manifest is replaced. A read that throws part way skips both.
 */
export async function backfillEntityFeedConfig(
	deps: KeyedEntityFeedConfigDeps,
	rows: {weights: Rows<EntityTypeWeightRow>; exclusions: Rows<EntityTypeExclusionRow>; blocks: Rows<EntityFeedBlockRow>},
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> {
	const seen = [new Set<string>(), new Set<string>(), new Set<string>()]
	const a = await backfillEntityTypeWeights(deps, noting(rows.weights, "type_id", seen[0] as Set<string>), progress)
	const b = await backfillEntityTypeExclusions(deps, noting(rows.exclusions, "type_id", seen[1] as Set<string>), progress)
	const c = await backfillEntityFeedBlocks(deps, noting(rows.blocks, "entity_id", seen[2] as Set<string>), progress)
	const dead = {n: 0}
	if (deps.held) {
		for (const [i, cl] of clearing.entries()) {
			const now = seen[i] as Set<string>
			for (const key of deps.held.load(cl.table)) {
				if (now.has(key)) continue
				if ((await clearRecord(deps, cl, key)) === "dead_letter") dead.n++
				await deps.pace?.()
			}
			deps.held.save(cl.table, now)
		}
	}
	return {rows: a.rows + b.rows + c.rows, applied: a.applied + b.applied + c.applied, deadLetter: a.deadLetter + b.deadLetter + c.deadLetter + dead.n}
}
