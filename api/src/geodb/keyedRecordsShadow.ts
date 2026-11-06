/**
 * The keyed-record shadow (GEODB_KEYED_RECORDS_SHADOW, off by default) for `user_votes`, `ranks.rankings` and
 * `ranks.ranking_blocks`.
 *
 * The api never serves these tables through a path with a geo-db twin: `user_votes` only feeds SQL joins and filters
 * inside PostGraphile and pair-fit, and the `ranks` schema is read by the ranking-indexer, not the api. So no request
 * carries a served row to compare. The feeder writes each row as a natural-key record in the `votes` system Space
 * (geodb-feeder/src/votesKeyed.ts, ranksKeyed.ts). This samples the tables instead: every `intervalMs`, in the
 * background, it reads up to `sample` rows of each table from Postgres starting at a random primary-key position
 * (wrapping to the start), derives the row's record id, reads the record's card from geo-db and compares each column the
 * feeder writes. It never touches a request.
 *
 * `gaia_api_keyed_records_shadow_total{table,result}`: match, mismatch (a column differs, or a column holds two values),
 * missing (geo-db has no record for a row Postgres has: the feeder lags, was never backfilled or dead-lettered the
 * row), unsupported (the votes Space is not in the space map) or failed (a read failed). Blind spot: only rows
 * Postgres has are sampled, so a record geo-db keeps after Postgres dropped the row is not seen.
 *
 * Not compared: a relation vote's object column (geo-db holds a relation as a statement, whose id this copy does not
 * derive), and `ranking_blocks.restriction_id` / `ranking_type` / dates (a block's list of them is rewritten in place).
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {entityRef, idPart, intPart, recordEntityId} from "../proposals/governance/recordKey"
import {AmbiguousRecordError, instantSeconds, wholeNumber} from "../proposals/governance/records"
import type {SystemSpaceLookup} from "../proposals/governance/shadow"
import type {GeoCardRow} from "./card"
import {type GeoDbCardClient, GeoDbError} from "./client"

export type KeyedRecordsShadowConfig = {
	enabled: boolean
	intervalMs: number
	sample: number
}

const wholeNumberEnv = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readKeyedRecordsShadowConfig(env: Record<string, string | undefined>): KeyedRecordsShadowConfig {
	const flag = env.GEODB_KEYED_RECORDS_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumberEnv(env.GEODB_KEYED_RECORDS_SHADOW_INTERVAL_MS, 60_000, 86_400_000),
		sample: wholeNumberEnv(env.GEODB_KEYED_RECORDS_SHADOW_SAMPLE, 20, 200),
	}
}

/** The record kinds and columns as the feeder pins them (geodb-feeder/src/recordKinds.ts); a test holds the copies to it. */
export const USER_VOTE_KIND = "1623234736184c0d9a0b69e2ffbc124d"
export const RANKING_KIND = "ff8283aec584455a80198d6d80f0f4fc"
export const RANKING_BLOCK_KIND = "ce9b2464e65c450a975d3a427baaf4a2"

export const USER_VOTE_COLUMNS = {
	user: "4bd6d3068a204ff6a62bb084263045bc",
	object: "46e83cb03e0c46a4a874d7deeb5889dd",
	objectType: "e2f5df668d07476ba3f1721c885bd53e",
	space: "59217d4ba84c4fdcbdc3ff6fb3cb83fa",
	kind: "4f65ca0db5874a519d8068671a0222c5",
	direction: "8f3588d7a11c4e7eb86b61015670b491",
	votedAt: "6e088d8604924b05b09e3ebeceebf5db",
} as const

export const RANKING_COLUMNS = {
	ranking: "5afd9abadca04814bb03eda81d951d22",
	space: "c84de538157b4562be0352add400ee2f",
	block: "e702f42b27a94659a38f3ee7f8aef0b8",
	rankType: "b943c785e30e4fa5a2f4c8886b4104de",
	submittedAt: "883ddfb02814461881f7ed2fdf0c6be5",
	updatedAtBlock: "e2a7000398d848a7a5e541f63fd6c1a1",
	updateIndex: "529e8bb7b260483eb78cd68aa221a8a5",
} as const

export const RANKING_BLOCK_COLUMNS = {
	block: "30a0bcf61226497ea1a9fbdb05be369e",
	space: "723da53041a149e8bee03243676aec1a",
	name: "01c6cd80b4944f1eac79a2e069618dea",
	filter: "34f4cb764c2d4c88a40ad7553254fcae",
	submissionFrequency: "ead65e7abfe64037aaef51050f4f2cc7",
} as const

export const TABLES = ["user_votes", "rankings", "ranking_blocks"] as const
export type KeyedTable = (typeof TABLES)[number]
export const RESULTS = ["match", "mismatch", "missing", "unsupported", "failed"] as const

/** One column the feeder writes, as the record must hold it. A null value means the record must hold no such column. */
export type Expected =
	| {prop: string; kind: "ref" | "text"; value: string | null}
	| {prop: string; kind: "num" | "instant"; value: bigint | null}

/** A sampled Postgres row: the record's id and Space family, and every column the record must hold. */
export type SampledRecord = {entityId: string; expected: Expected[]}

type Row = Record<string, unknown>

const str = (v: unknown): string | null => (typeof v === "string" ? v : null)
const isId = (v: unknown): v is string =>
	typeof v === "string" && /^[0-9a-f]{8}-?([0-9a-f]{4}-?){3}[0-9a-f]{12}$/i.test(v)
const big = (v: unknown): bigint | null => {
	if (typeof v === "bigint") return v
	if (typeof v === "number" && Number.isInteger(v)) return BigInt(v)
	if (typeof v === "string" && /^-?\d+$/.test(v)) return BigInt(v)
	return null
}

/** The record a Postgres `user_votes` row stands for, or null when a value is not one an event could have written. */
export function userVoteRecord(r: Row): SampledRecord | null {
	const objectType = big(r.object_type)
	const kind = big(r.vote_kind)
	const direction = big(r.vote_type)
	const votedAt = big(r.voted_at_seconds)
	if (!isId(r.user_id) || !isId(r.object_id) || !isId(r.space_id)) return null
	if (objectType === null || kind === null || direction === null || votedAt === null) return null
	if (objectType !== 0n && objectType !== 1n) return null
	if (kind < -32768n || kind > 32767n) return null
	const C = USER_VOTE_COLUMNS
	return {
		entityId: recordEntityId(USER_VOTE_KIND, [
			idPart(r.user_id),
			idPart(r.object_id),
			intPart(objectType),
			idPart(r.space_id),
			intPart(BigInt.asUintN(16, kind)),
		]),
		expected: [
			{prop: C.user, kind: "ref", value: entityRef(r.user_id)},
			// A relation is a statement in geo-db; its id is not derived here.
			...(objectType === 0n ? [{prop: C.object, kind: "ref" as const, value: entityRef(r.object_id)}] : []),
			{prop: C.space, kind: "ref", value: entityRef(r.space_id)},
			{prop: C.objectType, kind: "num", value: objectType},
			{prop: C.kind, kind: "num", value: kind},
			{prop: C.direction, kind: "num", value: direction},
			{prop: C.votedAt, kind: "instant", value: votedAt},
		],
	}
}

/** The record a Postgres `ranks.rankings` row stands for. `submitted_at_seconds` is whole seconds, null when unset. */
export function rankingRecord(r: Row): SampledRecord | null {
	if (!isId(r.id) || !isId(r.space_id)) return null
	const updatedAtBlock = big(r.updated_at_block)
	const updateIndex = big(r.update_index)
	if (updatedAtBlock === null || updateIndex === null) return null
	const submitted = r.submitted_at_seconds === null ? null : big(r.submitted_at_seconds)
	if (r.submitted_at_seconds !== null && submitted === null) return null
	const C = RANKING_COLUMNS
	return {
		entityId: recordEntityId(RANKING_KIND, [idPart(r.id), idPart(r.space_id)]),
		expected: [
			{prop: C.ranking, kind: "ref", value: entityRef(r.id)},
			{prop: C.space, kind: "ref", value: entityRef(r.space_id)},
			{prop: C.block, kind: "ref", value: isId(r.block_id) ? entityRef(r.block_id) : null},
			{prop: C.rankType, kind: "text", value: str(r.rank_type)},
			{prop: C.submittedAt, kind: "instant", value: submitted},
			{prop: C.updatedAtBlock, kind: "num", value: updatedAtBlock},
			{prop: C.updateIndex, kind: "num", value: updateIndex},
		],
	}
}

/** The record a Postgres `ranks.ranking_blocks` row stands for. */
export function rankingBlockRecord(r: Row): SampledRecord | null {
	if (!isId(r.id) || !isId(r.space_id)) return null
	const frequency = r.submission_frequency === null ? null : big(r.submission_frequency)
	if (r.submission_frequency !== null && frequency === null) return null
	const C = RANKING_BLOCK_COLUMNS
	return {
		entityId: recordEntityId(RANKING_BLOCK_KIND, [idPart(r.id), idPart(r.space_id)]),
		expected: [
			{prop: C.block, kind: "ref", value: entityRef(r.id)},
			{prop: C.space, kind: "ref", value: entityRef(r.space_id)},
			{prop: C.name, kind: "text", value: str(r.name)},
			{prop: C.filter, kind: "text", value: str(r.filter)},
			{prop: C.submissionFrequency, kind: "num", value: frequency},
		],
	}
}

/**
 * What a record's card says about a sampled row. `missing` when the card has no rows at all. A column Postgres has
 * must hold exactly that value (and one value); a column Postgres holds as null must not be on the record. Pure.
 */
export function compareRecord(sampled: SampledRecord, card: GeoCardRow[]): "match" | "mismatch" | "missing" {
	if (card.length === 0) return "missing"
	try {
		for (const e of sampled.expected) {
			const held = [...new Set(card.filter((r) => r.prop === e.prop).map((r) => r.raw))]
			if (held.length > 1) return "mismatch"
			const raw = held[0]
			if (e.value === null) {
				if (raw !== undefined) return "mismatch"
				continue
			}
			if (raw === undefined) return "mismatch"
			const same =
				e.kind === "num"
					? wholeNumber(raw) === e.value
					: e.kind === "instant"
						? instantSeconds(raw) === e.value
						: raw === e.value
			if (!same) return "mismatch"
		}
	} catch (error) {
		if (error instanceof AmbiguousRecordError) return "mismatch"
		throw error
	}
	return "match"
}

let active = false
const results = new Map<string, number>()
const bump = (table: KeyedTable, result: (typeof RESULTS)[number]): void => {
	const k = `${table}\t${result}`
	results.set(k, (results.get(k) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderKeyedRecordsShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_keyed_records_shadow_total Sampled user_votes / ranks.rankings / ranks.ranking_blocks rows from Postgres against geo-db's keyed records, by table and result: match, mismatch, missing (no record), unsupported (no votes Space) or failed (a read failed).",
		"# TYPE gaia_api_keyed_records_shadow_total counter",
	]
	for (const t of TABLES)
		for (const r of RESULTS)
			lines.push(
				`gaia_api_keyed_records_shadow_total{table="${t}",result="${r}"} ${results.get(`${t}\t${r}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetKeyedRecordsShadowMetricsForTests(): void {
	active = false
	results.clear()
}

function rowsOf(result: unknown): Row[] {
	return ((result as {rows?: Row[]}).rows ?? []) as Row[]
}

const BUILD: Record<KeyedTable, (r: Row) => SampledRecord | null> = {
	user_votes: userVoteRecord,
	rankings: rankingRecord,
	ranking_blocks: rankingBlockRecord,
}

/** Up to `limit` rows of `table` from the primary-key position `pivot` (a UUID), wrapping to the start when short. */
export async function sampleRecords(
	db: Database,
	table: KeyedTable,
	pivot: string,
	limit: number,
): Promise<SampledRecord[]> {
	const read = async (from: string | null, n: number): Promise<SampledRecord[]> => {
		let result: unknown
		if (table === "user_votes") {
			const cols = sql`user_id::text, object_id::text, object_type::int, space_id::text, vote_kind::int, vote_type::int, extract(epoch from voted_at)::bigint::text AS voted_at_seconds`
			result = await db.execute(
				from === null
					? sql`SELECT ${cols} FROM user_votes ORDER BY user_id, object_id, object_type, space_id, vote_kind LIMIT ${n}`
					: sql`SELECT ${cols} FROM user_votes WHERE user_id >= ${from}::uuid ORDER BY user_id, object_id, object_type, space_id, vote_kind LIMIT ${n}`,
			)
		} else if (table === "rankings") {
			const cols = sql`id::text, space_id::text, block_id::text, rank_type, updated_at_block::text, update_index::text, extract(epoch from submitted_at)::bigint::text AS submitted_at_seconds`
			result = await db.execute(
				from === null
					? sql`SELECT ${cols} FROM ranks.rankings ORDER BY id, space_id LIMIT ${n}`
					: sql`SELECT ${cols} FROM ranks.rankings WHERE id >= ${from}::uuid ORDER BY id, space_id LIMIT ${n}`,
			)
		} else {
			const cols = sql`id::text, space_id::text, name, filter, submission_frequency::text`
			result = await db.execute(
				from === null
					? sql`SELECT ${cols} FROM ranks.ranking_blocks ORDER BY id, space_id LIMIT ${n}`
					: sql`SELECT ${cols} FROM ranks.ranking_blocks WHERE id >= ${from}::uuid ORDER BY id, space_id LIMIT ${n}`,
			)
		}
		return rowsOf(result)
			.map(BUILD[table])
			.filter((r): r is SampledRecord => r !== null)
	}
	const first = await read(pivot, limit)
	if (first.length >= limit) return first
	// The wrap re-reads from the start, which a small table (or a pivot near the front) overlaps: keep each record once.
	const seen = new Set(first.map((r) => r.entityId))
	return first.concat((await read(null, limit)).filter((r) => !seen.has(r.entityId)).slice(0, limit - first.length))
}

export type KeyedRecordsShadow = {
	/** One sampling pass over the tables. Resolves when done; never throws. */
	runOnce(): Promise<void>
	/** Start the periodic pass; returns the stop function. The timer does not keep the process alive. */
	start(): () => void
}

export function createKeyedRecordsShadow(deps: {
	config: KeyedRecordsShadowConfig
	db: Database
	geoDb: Pick<GeoDbCardClient, "readCard">
	systemSpaceOf: SystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
}): KeyedRecordsShadow {
	const {config, db, geoDb, systemSpaceOf, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	active = true
	let running = false
	let logged = 0

	async function pass(table: KeyedTable): Promise<void> {
		let geoSpaceId: string | null
		try {
			geoSpaceId = await systemSpaceOf("votes")
		} catch (error) {
			bump(table, "failed")
			log.warn("keyed records shadow space lookup failed", {table, error: String(error)})
			return
		}
		if (!geoSpaceId) {
			bump(table, "unsupported")
			return
		}
		let sampled: SampledRecord[]
		try {
			sampled = await sampleRecords(db, table, pivotOf(), config.sample)
		} catch (error) {
			bump(table, "failed")
			log.warn("keyed records shadow sample failed", {table, error: String(error)})
			return
		}
		for (const record of sampled) {
			try {
				const outcome = compareRecord(record, await geoDb.readCard({geoSpaceId, entityId: record.entityId}))
				bump(table, outcome)
				// Counts only: never which row.
				if (outcome === "mismatch" && logged < 20) {
					logged++
					log.warn("keyed records shadow mismatch", {table})
				}
			} catch (error) {
				bump(table, "failed")
				if (!(error instanceof GeoDbError))
					log.warn("keyed records shadow failed", {table, error: String(error)})
			}
		}
	}

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		try {
			for (const t of TABLES) await pass(t)
		} finally {
			running = false
		}
	}

	return {
		runOnce,
		start() {
			if (!config.enabled) return () => {}
			const timer = setInterval(() => void runOnce().catch(() => {}), config.intervalMs)
			timer.unref?.()
			return () => clearInterval(timer)
		},
	}
}
