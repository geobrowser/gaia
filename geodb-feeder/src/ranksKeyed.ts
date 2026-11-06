/**
 * The ranking-indexer's three tables (`ranks.ranking_blocks`, `ranks.rankings`, `ranks.ranking_items`) as natural-key
 * records (geo-db plan 10.12 K1-K5), behind GEODB_RANKS, which takes effect only with GEODB_RECORD_KEYS
 * (ranksKeyedEnabled). With either off nothing here runs. A flag of its own: the backfill must run first.
 *
 * Source. ranking-indexer folds `knowledge.edits` into Postgres (ranking-indexer/src/detect.rs per edit, storage.rs
 * apply). This consumer reads the same topic on a group of its own and ports detect.rs (detectRanks) so the same edit
 * yields the same facts. Out of scope, deliberately: `ranking_scores` (computed by recompute.rs, not an edit fold),
 * the author address (resolved from a personal space during aggregation; only the backfill carries it) and the
 * public-graph RANK_POSITION projection.
 *
 * Keys are each table's primary key, so one row is one record and a replay is a no-op:
 * - ranking_block: [block, space]; ranking: [ranking, space]; ranking_item: [ranking, entity, space] (the item's
 *   `to_space`, not the edit's). All three kinds sit in the `votes` family (RECORD_KINDS), so no fourth system Space
 *   is founded and none is added to the serve cap.
 * - Key columns are references (`add` + `set_active`, keyColumn); every other column is a `put` whose statement id
 *   includes the value, so a changed value closes the old one and A -> B -> A ends on A.
 * - A column the source lacks is NOT written, so it is never cleared. Postgres is different where an edit re-creates
 *   a block or ranking that lacks a value (`ON CONFLICT DO UPDATE` writes NULL); geo-db keeps the older value there.
 *
 * Replace-whole-ranking, without a read before the write. storage.rs deletes every `ranking_items` row of a ranking
 * (across spaces) before inserting the items of an edit that carries at least one. geo-db cannot delete, so each item
 * carries a `generation` column, the block number of the edit that wrote it (0 for the backfill), and a reader takes,
 * for a ranking, only the items at its maximum generation. Older items stay in the record set but are stale. Stream
 * order is last-wins, as in Postgres (no max guard there either), so a replay from the start converges, and a
 * stream replay (generation = block > 0) supersedes the backfill snapshot (generation 0).
 *
 * Differences to carry, none of them a bug on either side:
 * - Two edits for one ranking in the SAME block tie, and their items union; Postgres keeps the last edit's.
 * - An edit whose metadata lacks the block number is generation 0, like the backfill (kg-style `unwrap_or(0)`).
 * - A RANK_BLOCK link writes the ranking record's block column even when the ranking was created in another edit;
 *   Postgres' `UPDATE` is a no-op until the ranking row exists, so geo-db can hold a block link Postgres lacks.
 * - Items without `to_space` are skipped, as detect.rs does.
 */
import {relationEntityId} from "@geoprotocol/grc-20"
import {parseHermesEdit} from "./hermes.js"
import type {Decode} from "./handle.js"
import {cid, hexId} from "./ids.js"
import {
	applyKeyedRecord,
	backfillKeyedRows,
	deadLetterDecode,
	type KeyedBackfillResult,
	type KeyedOutcome,
	type KeyedRecordDeps,
	keyColumn,
} from "./keyedRecord.js"
import {idPart, intPart, type RecordKeyPart, recordEntityId, recordStatementId, textPart} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {floatToDecimal, type GrcOp, type GrcPropertyValue, putIntent, zigString} from "./translate.js"

const K = {block: RECORD_KINDS.rankingBlock, ranking: RECORD_KINDS.ranking, item: RECORD_KINDS.rankingItem}
const FAMILY = RECORD_KINDS.ranking.family
const CB = RECORD_COLUMNS.rankingBlock
const CR = RECORD_COLUMNS.ranking
const CI = RECORD_COLUMNS.rankingItem

// sdk/src/core/ids.rs, as 32 hex digits.
const d = (u: string) => u.replaceAll("-", "")
const ID = {
	typeRelation: d("8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"),
	name: d("a126ca53-0c8e-48d5-b888-82c734c38935"),
	rankType: d("5c74731d-fabb-4dc8-b5c5-3346521c639a"),
	rankTypeProperty: d("48e01bc8-324e-48c2-a6c9-cab3f49290c6"),
	votesRelation: d("19a4cfff-45f2-4150-abf2-af0f43eb2eec"),
	weight: d("103701dd-cabe-4a8e-835b-10345327b647"),
	blockType: d("150db6de-fe23-44f0-805a-fa57502e2c32"),
	blockRelation: d("09c219c1-03d1-4d2a-a5c7-8edbf2d0182a"),
	startDate: d("eed03a04-0acd-4a9e-81e0-8272ed70a817"),
	endDate: d("b08b8f63-dc1e-4156-8b08-19946f2b011c"),
	startDatetime: d("2d696bf0-510f-403e-985b-8cd1e73feb9b"),
	endDatetime: d("c3445f6b-e2c0-4f25-b73a-5eb876c4f50c"),
	restriction: d("1e4caa2d-e331-4efa-8ac2-4e8d9d3e9fe9"),
	filter: d("14a46854-bfd1-4b18-8215-2785c2dab9f3"),
	frequency: d("c0ab9953-8eeb-4d7c-8ccb-1dd585ae4711"),
	rolling: d("d00ddc89-b23c-4a20-a63f-ebf545ddc139"),
}

export type BlockFacts = {
	block: string
	name: string | null
	filter: string | null
	/** Microseconds since the epoch. */
	startDate: bigint | null
	endDate: bigint | null
	restriction: string | null
	/** The rolling type's id, or null. */
	rankingType: string | null
	submissionFrequency: bigint | null
}
export type RankingFacts = {
	ranking: string
	rankType: string | null
	/** Whole seconds. */
	submittedAt: bigint | null
	updatedAtBlock: bigint | null
	updateIndex: bigint | null
}
export type ItemFacts = {ranking: string; entity: string; space: string; position: string | null; weight: number | null}
export type DetectedRanks = {blocks: BlockFacts[]; rankings: RankingFacts[]; items: ItemFacts[]; links: {ranking: string; block: string}[]}
export type EditMeta = {space: string; block: number | bigint; timestamp: number | bigint}

const idOf = (x: unknown): string => hexId(x as string | Uint8Array).toLowerCase()

const find = (values: readonly GrcPropertyValue[] | undefined, property: string, type: string) =>
	values?.find((pv) => idOf(pv.property) === property && pv.value?.type === type)?.value

const textOf = (values: readonly GrcPropertyValue[] | undefined, property: string): string | null => {
	const v = find(values, property, "text")
	return v === undefined ? null : String(v.value)
}

/** detect.rs int_value: an Integer that fits an i32. */
const intOf = (values: readonly GrcPropertyValue[] | undefined, property: string): bigint | null => {
	const v = find(values, property, "integer")
	if (v === undefined) return null
	const n = BigInt(v.value as bigint | number | string)
	return n >= -(2n ** 31n) && n < 2n ** 31n ? n : null
}

const RFC3339 = /^(\d{4}-\d{2}-\d{2})[Tt ](\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?([Zz]|[+-]\d{2}:\d{2})$/
const BARE = /^\d{4}-\d{2}-\d{2}$/

/** detect.rs parse_date: RFC3339, or a bare YYYY-MM-DD at midnight UTC. Microseconds since the epoch. */
export function parseDate(s: string): bigint | null {
	let m = RFC3339.exec(s)
	if (m) {
		const ms = Date.parse(`${m[1]}T${m[2]}${m[4]?.toUpperCase()}`)
		if (Number.isNaN(ms)) return null
		return BigInt(ms) * 1000n + BigInt((m[3] ?? "").padEnd(6, "0").slice(0, 6) || "0")
	}
	m = BARE.exec(s)
	if (m) {
		const ms = Date.parse(`${s}T00:00:00Z`)
		return Number.isNaN(ms) ? null : BigInt(ms) * 1000n
	}
	return null
}

const dateOf = (values: readonly GrcPropertyValue[] | undefined, property: string): bigint | null => {
	const v = values?.find((pv) => idOf(pv.property) === property && (pv.value?.type === "date" || pv.value?.type === "datetime"))?.value
	return v === undefined ? null : parseDate(String(v.value))
}

/** Port of detect.rs `detect`: the rank-relevant facts of one decoded edit. Later duplicates win. */
export function detectRanks(ops: readonly GrcOp[], meta: EditMeta): DetectedRanks {
	const entityValues = new Map<string, readonly GrcPropertyValue[] | undefined>()
	const typesOf = new Map<string, Set<string>>()
	const restrictionOf = new Map<string, string>()
	for (const op of ops) {
		if (op.type === "createEntity") entityValues.set(idOf(op.id), op.values)
		else if (op.type === "createRelation") {
			const t = idOf(op.relationType)
			if (t === ID.typeRelation) {
				const from = idOf(op.from)
				if (!typesOf.has(from)) typesOf.set(from, new Set())
				typesOf.get(from)?.add(idOf(op.to))
			} else if (t === ID.restriction) restrictionOf.set(idOf(op.from), idOf(op.to))
		}
	}
	const blocks = new Map<string, BlockFacts>()
	const rankings = new Map<string, RankingFacts>()
	const items = new Map<string, ItemFacts>()
	const links = new Map<string, {ranking: string; block: string}>()
	ops.forEach((op, index) => {
		if (op.type === "createEntity") {
			const id = idOf(op.id)
			const types = typesOf.get(id)
			if (types?.has(ID.rankType)) {
				rankings.set(id, {
					ranking: id,
					rankType: textOf(op.values, ID.rankTypeProperty),
					submittedAt: BigInt(meta.timestamp),
					updatedAtBlock: BigInt(meta.block),
					updateIndex: BigInt(index),
				})
			} else if (types?.has(ID.blockType)) {
				blocks.set(id, {
					block: id,
					name: textOf(op.values, ID.name),
					filter: textOf(op.values, ID.filter),
					startDate: dateOf(op.values, ID.startDatetime) ?? dateOf(op.values, ID.startDate),
					endDate: dateOf(op.values, ID.endDatetime) ?? dateOf(op.values, ID.endDate),
					restriction: restrictionOf.get(id) ?? null,
					rankingType: types.has(ID.rolling) ? ID.rolling : null,
					submissionFrequency: intOf(op.values, ID.frequency),
				})
			}
		} else if (op.type === "createRelation") {
			const t = idOf(op.relationType)
			const from = idOf(op.from)
			if (t === ID.votesRelation) {
				if (op.toSpace === undefined || op.toSpace === null) return
				const reified = op.entity !== undefined && op.entity !== null ? idOf(op.entity) : hexId(relationEntityId(op.id as never))
				const w = find(entityValues.get(reified), ID.weight, "float")
				const item: ItemFacts = {
					ranking: from,
					entity: idOf(op.to),
					space: idOf(op.toSpace),
					position: typeof op.position === "string" ? op.position : null,
					weight: w === undefined ? null : Number(w.value),
				}
				items.set(`${item.ranking}/${item.entity}/${item.space}`, item)
			} else if (t === ID.blockRelation) links.set(`${from}/${idOf(op.to)}`, {ranking: from, block: idOf(op.to)})
		}
	})
	return {blocks: [...blocks.values()], rankings: [...rankings.values()], items: [...items.values()], links: [...links.values()]}
}

/** The record entities. */
export const rankingBlockRecordId = (block: string, space: string): string => recordEntityId(K.block.id, [idPart(block), idPart(space)])
export const rankingRecordId = (ranking: string, space: string): string => recordEntityId(K.ranking.id, [idPart(ranking), idPart(space)])
export const rankingItemRecordId = (ranking: string, entity: string, space: string): string =>
	recordEntityId(K.item.id, [idPart(ranking), idPart(entity), idPart(space)])

const u64 = (v: bigint) => intPart(BigInt.asUintN(64, v))

/** Appends the creation, key columns and puts of one record; `put` ids include the value. */
class RecordBuilder {
	readonly out: string[]
	constructor(
		private readonly kind: string,
		readonly id: string,
	) {
		this.out = [`.{ .create_entity = "${id}" }`]
	}
	key(property: string, target: string): this {
		const ref = cid("entity", target)
		this.out.push(...keyColumn(recordStatementId(this.kind, [idPart(this.id), idPart(property), idPart(ref)]), this.id, property, `.{ .reference = "${ref}" }`))
		return this
	}
	private put(property: string, part: RecordKeyPart, value: string): this {
		this.out.push(putIntent({id: recordStatementId(this.kind, [idPart(this.id), idPart(property), part]), entity: this.id, property, value}))
		return this
	}
	text(property: string, v: string | null): this {
		return v === null ? this : this.put(property, textPart(v), `.{ .text = ${zigString(v)} }`)
	}
	number(property: string, v: bigint | null): this {
		return v === null ? this : this.put(property, u64(v), `.{ .number = ${v} }`)
	}
	instant(property: string, micros: bigint | null): this {
		return micros === null ? this : this.put(property, u64(micros), `.{ .instant = "${micros}" }`)
	}
	reference(property: string, target: string | null): this {
		if (target === null) return this
		const ref = cid("entity", target)
		return this.put(property, idPart(ref), `.{ .reference = "${ref}" }`)
	}
	decimal(property: string, v: number | null): this {
		const dec = v === null ? null : floatToDecimal(v)
		return dec === null ? this : this.put(property, textPart(`${dec[0]},${dec[1]}`), `.{ .decimal = "${dec[0]},${dec[1]}" }`)
	}
}

export type RankingRecord = RankingFacts & {block?: string | null; author?: string | null}

const blockIntents = (b: BlockFacts, space: string): string[] =>
	new RecordBuilder(K.block.id, rankingBlockRecordId(b.block, space))
		.key(CB.block, b.block)
		.key(CB.space, space)
		.text(CB.name, b.name)
		.text(CB.filter, b.filter)
		.instant(CB.startDate, b.startDate)
		.instant(CB.endDate, b.endDate)
		.reference(CB.restriction, b.restriction)
		.reference(CB.rankingType, b.rankingType)
		.number(CB.submissionFrequency, b.submissionFrequency).out

const rankingIntents = (r: RankingRecord, space: string): string[] =>
	new RecordBuilder(K.ranking.id, rankingRecordId(r.ranking, space))
		.key(CR.ranking, r.ranking)
		.key(CR.space, space)
		.reference(CR.block, r.block ?? null)
		.text(CR.rankType, r.rankType)
		.instant(CR.submittedAt, r.submittedAt === null ? null : r.submittedAt * 1_000_000n)
		.number(CR.updatedAtBlock, r.updatedAtBlock)
		.number(CR.updateIndex, r.updateIndex)
		.text(CR.author, r.author ?? null).out

const itemIntents = (i: ItemFacts, generation: bigint): string[] =>
	new RecordBuilder(K.item.id, rankingItemRecordId(i.ranking, i.entity, i.space))
		.key(CI.ranking, i.ranking)
		.key(CI.entity, i.entity)
		.key(CI.space, i.space)
		.text(CI.position, i.position)
		.decimal(CI.weight, i.weight)
		.number(CI.generation, generation).out

/** Records per intent group: one apply each, so one huge edit does not become one huge document. */
const GROUP = 500

/**
 * The intents of one edit's facts, in groups of at most GROUP records. `space` is the edit's Space (the blocks' and
 * rankings' key); each item's own space is its key. Items carry `generation`. A ranking linked to a block, but not
 * created in the edit, still gets its record, as the link alone is a fact (see the header).
 */
export function ranksIntentGroups(r: DetectedRanks, space: string, generation: bigint = r.rankings[0]?.updatedAtBlock ?? 0n): string[][] {
	const groups: string[][] = []
	const push = (records: string[][]) => {
		for (let i = 0; i < records.length; i += GROUP) groups.push(records.slice(i, i + GROUP).flat())
	}
	push(r.blocks.map((b) => blockIntents(b, space)))
	const rankings = new Map<string, RankingRecord>(r.rankings.map((x) => [x.ranking, x]))
	for (const l of r.links) rankings.set(l.ranking, {...(rankings.get(l.ranking) ?? {ranking: l.ranking, rankType: null, submittedAt: null, updatedAtBlock: null, updateIndex: null}), block: l.block})
	push([...rankings.values()].map((x) => rankingIntents(x, space)))
	push(r.items.map((i) => itemIntents(i, generation)))
	return groups
}

export type KeyedRanksDeps = KeyedRecordDeps & {decode: Decode}

async function applyGroups(deps: KeyedRecordDeps, groups: string[][], meta: {partition: number; offset: string; space: string}): Promise<KeyedOutcome> {
	let outcome: KeyedOutcome = "applied"
	// Each group is idempotent, so a retry after a thrown founding failure rewrites the earlier ones to the same ids.
	for (const g of groups) if ((await applyKeyedRecord(deps, FAMILY, g, meta)) === "dead_letter") outcome = "dead_letter"
	return outcome
}

export async function handleRanksMessage(
	deps: KeyedRanksDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	if (!message.value) return "ignored"
	let space: string
	let detected: DetectedRanks
	let block: bigint
	try {
		const he = parseHermesEdit(message.value)
		if (!he.payload || !he.spaceId) throw new Error("edit has no payload or space")
		if (he.spaceId.length !== 32) throw new Error("edit space_id is not 16 bytes")
		space = he.spaceId
		block = BigInt(he.blockNumber ?? 0)
		detected = detectRanks((await deps.decode(he.payload)).ops, {space, block, timestamp: he.createdAt ?? 0})
	} catch (e) {
		return deadLetterDecode(deps, "knowledge.edits", partition, message.offset, e)
	}
	if (!detected.blocks.length && !detected.rankings.length && !detected.items.length && !detected.links.length) return "ignored"
	return applyGroups(deps, ranksIntentGroups(detected, space, block), {partition, offset: message.offset, space})
}

/** Whether the keyed ranks writer runs: GEODB_RANKS only takes effect with GEODB_RECORD_KEYS. */
export const ranksKeyedEnabled = (c: {recordKeys: boolean; ranks: {enabled: boolean}}): boolean => c.ranks.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const ranksOffsetsDir = (dataDir: string) => `${dataDir}/feeder/ranks-keyed`

// ---- backfill rows (pgSource.readRankingBlockRows, readRankingRows, readRankingItemRows) ----

/** Timestamps arrive as microseconds since the epoch, text (the readers cast them in SQL). */
export type RankingBlockRow = {
	id: string
	space_id: string
	name: string | null
	filter: string | null
	start_date: string | null
	end_date: string | null
	restriction_id: string | null
	ranking_type: string | null
	submission_frequency: number | string | null
}
export type RankingRow = {
	id: string
	space_id: string
	block_id: string | null
	author_address: string | null
	rank_type: string | null
	submitted_at: string | null
	updated_at_block: string | null
	update_index: string | null
}
export type RankingItemRow = {ranking_id: string; entity_id: string; space_id: string; position: string | null; weight: number | string | null}

const hex = (v: string): string => {
	const h = v.replaceAll("-", "").toLowerCase()
	if (!/^[0-9a-f]{32}$/.test(h)) throw new Error(`ranks: not a uuid ${JSON.stringify(v)}`)
	return h
}
const hexOrNull = (v: string | null) => (v === null ? null : hex(v))
const INT = /^-?\d+$/
const big = (k: string, v: string | number | null): bigint | null => {
	if (v === null) return null
	const s = String(v)
	if (!INT.test(s) || BigInt.asIntN(64, BigInt(s)) !== BigInt(s)) throw new Error(`ranks: ${k} ${JSON.stringify(v)}`)
	return BigInt(s)
}

type Snapshot = {kind: "block"; space: string; facts: BlockFacts} | {kind: "ranking"; space: string; facts: RankingRecord} | {kind: "item"; facts: ItemFacts}

export function rankingBlockFromRow(r: RankingBlockRow): Snapshot {
	return {
		kind: "block",
		space: hex(r.space_id),
		facts: {
			block: hex(r.id),
			name: r.name,
			filter: r.filter,
			startDate: big("start_date", r.start_date),
			endDate: big("end_date", r.end_date),
			restriction: hexOrNull(r.restriction_id),
			rankingType: hexOrNull(r.ranking_type),
			submissionFrequency: big("submission_frequency", r.submission_frequency),
		},
	}
}
export function rankingFromRow(r: RankingRow): Snapshot {
	const submitted = big("submitted_at", r.submitted_at)
	return {
		kind: "ranking",
		space: hex(r.space_id),
		facts: {
			ranking: hex(r.id),
			block: hexOrNull(r.block_id),
			author: r.author_address,
			rankType: r.rank_type,
			// The source is microseconds; the facts hold whole seconds, and the intent scales them back.
			submittedAt: submitted === null ? null : submitted / 1_000_000n,
			updatedAtBlock: big("updated_at_block", r.updated_at_block),
			updateIndex: big("update_index", r.update_index),
		},
	}
}
export function rankingItemFromRow(r: RankingItemRow): Snapshot {
	const weight = r.weight === null ? null : Number(r.weight)
	if (weight !== null && !Number.isFinite(weight)) throw new Error(`ranks: weight ${JSON.stringify(r.weight)}`)
	return {kind: "item", facts: {ranking: hex(r.ranking_id), entity: hex(r.entity_id), space: hex(r.space_id), position: r.position, weight}}
}

const applySnapshot = (deps: KeyedRecordDeps, s: Snapshot): Promise<KeyedOutcome> => {
	const meta = {partition: -1, offset: "backfill"}
	if (s.kind === "block") return applyKeyedRecord(deps, FAMILY, blockIntents(s.facts, s.space), {...meta, space: s.space})
	if (s.kind === "ranking") return applyKeyedRecord(deps, FAMILY, rankingIntents(s.facts, s.space), {...meta, space: s.space})
	return applyKeyedRecord(deps, FAMILY, itemIntents(s.facts, 0n), {...meta, space: s.facts.space})
}

type Rows<R> = AsyncIterable<R> | Iterable<R>

/**
 * Write a snapshot of the three tables: blocks, then rankings, then items at generation 0, so the stream supersedes
 * them. Rerunnable. A row no writer could have written is dead-lettered (counted, the Job fails at the end).
 */
export async function backfillRanks(
	deps: KeyedRecordDeps,
	src: {blocks: Rows<RankingBlockRow>; rankings: Rows<RankingRow>; items: Rows<RankingItemRow>},
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> {
	const total: KeyedBackfillResult = {rows: 0, applied: 0, deadLetter: 0}
	const run = async <R>(rows: Rows<R>, fromRow: (r: R) => Snapshot) => {
		const r = await backfillKeyedRows(deps, rows, {topic: "ranks", fromRow, apply: applySnapshot}, progress && ((n) => progress(total.rows + n)))
		total.rows += r.rows
		total.applied += r.applied
		total.deadLetter += r.deadLetter
	}
	await run(src.blocks, rankingBlockFromRow)
	await run(src.rankings, rankingFromRow)
	await run(src.items, rankingItemFromRow)
	return total
}
