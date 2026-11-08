/**
 * The entity score overlay's shadow (GEODB_RANKING_SCORES_SHADOW, off by default; plan 10.21 section 9).
 *
 * `public.entity_ranking_scores` is read by the api only inside SQL (the RANKING_SCORE order-by, entities_in_best_order,
 * for_you_candidates, the Entity computed columns), so no request has a served row to compare. geo-db holds an
 * ENGAGED-ONLY overlay of it in derived collection 21 (62-byte value, scoring-service vote-indexer `entity_scores_mirror`):
 * a row exists for an entity with a nonzero engagement term, and an absent key means the entity is un-engaged, whose row geo-db
 * builds from the card and the ranking config (geo-db `derived.entity_scores.get`). This samples both directions, in the
 * background, never touching a request:
 *
 *  - overlay -> Postgres: the first `sample` rows of collection 21 (score descending), each looked up by primary key;
 *  - Postgres -> overlay: `sample` rows from a random primary-key position (wrapping), each read from collection 21. Almost all
 *    are un-engaged (about 9.5k engaged of 48.9M), so this direction mostly proves "absent key for an un-engaged row".
 *
 * `gaia_api_ranking_scores_shadow_total{result}`:
 *  - match: engaged on both sides, same score and the same five counts;
 *  - mismatch: engaged on both sides but a different score or count;
 *  - absent_ok: Postgres row un-engaged and no overlay row (the default's case);
 *  - missing: Postgres row engaged and no overlay row (the mirror lags, or never ran);
 *  - stale: an overlay row where Postgres has no row or an un-engaged one (the mirror's delete has not run);
 *  - unsupported: an overlay row of another length or version (the 48-byte layout before the overlay writer);
 *  - failed: a read failed.
 *
 * The DEFAULT row's value (an un-engaged entity's score from its card and config) is compared by a third pass, when the reader
 * can serve it (geo-db wire frame revision 11, DerivedScores): `sampleIds` un-engaged rows from a random position, their cards
 * (distinct property count, non-system outgoing relation count, `created_at`, heaviest Types weight) and the ranking config read
 * from Postgres, sent to geo-db in one request, and the served quality, intrinsic, type weight and ranking scores held to the
 * Postgres row's. `gaia_api_ranking_scores_default_shadow_total{result}`:
 *  - match: all four scores agree;
 *  - mismatch: a score differs. A Postgres row is a snapshot of the entity's last refresh, so an entity whose card changed since
 *    (nothing re-scores on a card edit) is a benign mismatch: read the rate, not one count;
 *  - engaged: geo-db served an overlay row for an entity Postgres holds un-engaged (the same disagreement `stale` counts);
 *  - no_card: the sampled row has no entities row, so there is no card to build a default from;
 *  - failed: a read failed, or the node is below frame revision 11 and refused the request.
 *
 * Blind spots: the overlay direction reads only the highest-scored page, and a sample is not a scan. Logs counts only, never an
 * entity.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {GeoDbError} from "./client"
import type {
	DerivedReader,
	DerivedRow,
	DerivedScanReader,
	DerivedScoresReader,
	ScoreCard,
	ScoreConfig,
	ScoreRow,
} from "./derived"

export const COLLECTION = 21
export const OVERLAY_LEN = 62
export const OVERLAY_VERSION = 1
export const RESULTS = ["match", "mismatch", "absent_ok", "missing", "stale", "unsupported", "failed"] as const
export type Result = (typeof RESULTS)[number]

export const DEFAULT_RESULTS = ["match", "mismatch", "engaged", "no_card", "failed"] as const
export type DefaultResult = (typeof DEFAULT_RESULTS)[number]

export type RankingScoresShadowConfig = {
	enabled: boolean
	intervalMs: number
	sample: number
	/** Un-engaged rows compared against geo-db's computed default each pass (geo-db takes 64 at most). */
	defaultSample: number
}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readRankingScoresShadowConfig(env: Record<string, string | undefined>): RankingScoresShadowConfig {
	const flag = env.GEODB_RANKING_SCORES_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GEODB_RANKING_SCORES_SHADOW_INTERVAL_MS, 60_000, 86_400_000),
		sample: wholeNumber(env.GEODB_RANKING_SCORES_SHADOW_SAMPLE, 20, 200),
		defaultSample: wholeNumber(env.GEODB_RANKING_SCORES_SHADOW_DEFAULT_SAMPLE, 20, 64),
	}
}

/** Scores are f64 on both sides; Postgres's numeric text round trip may differ in the last digits. */
const TOLERANCE = 1e-9

/** The Postgres columns the overlay is the negation of (plan 10.21 section 9.1), as text. */
export type PgRow = {
	rankingScore: number
	positive: number
	negative: number
	stancePositive: number
	stanceNegative: number
	commenterCount: number
	/** The watch, age-credit and trending terms: the three the overlay value carries beside the counts. */
	watchScore: number
	ageCredit: number
	trendingScore: number
	/** True when any engagement term is nonzero: the overlay's predicate. */
	engaged: boolean
}

const num = (v: unknown): number => (v === null || v === undefined ? 0 : Number(v))

/** A Postgres row as the overlay sees it, or null when a column is not a number. */
export function toPgRow(row: Record<string, unknown>): PgRow | null {
	const rankingScore = Number(row.ranking_score)
	if (!Number.isFinite(rankingScore)) return null
	const counts: [number, number, number, number, number] = [
		num(row.positive),
		num(row.negative),
		num(row.stance_positive),
		num(row.stance_negative),
		num(row.commenter_count),
	]
	const terms = [
		row.participation_score,
		row.comment_score,
		row.topic_score,
		row.watch_score,
		row.age_credit,
		row.trending_score,
	].map(num)
	if ([...counts, ...terms].some((n) => !Number.isFinite(n))) return null
	return {
		rankingScore,
		positive: counts[0],
		negative: counts[1],
		stancePositive: counts[2],
		stanceNegative: counts[3],
		commenterCount: counts[4],
		watchScore: terms[3] as number,
		ageCredit: terms[4] as number,
		trendingScore: terms[5] as number,
		engaged: counts.some((n) => n !== 0) || terms.some((n) => n !== 0) || (row.trending_source ?? null) !== null,
	}
}

/** The five counts of a 62-byte overlay value, or null for another length or version. */
export function overlayCounts(value: Buffer): number[] | null {
	if (value.length !== OVERLAY_LEN || value[0] !== OVERLAY_VERSION || (value[1] ?? 9) > 2) return null
	return [2, 6, 10, 14, 18].map((at) => value.readUInt32LE(at))
}

/** What a Postgres row (null when it has none) and geo-db's overlay row (null when absent) say about each other. Pure. */
export function compareRow(pg: PgRow | null, held: DerivedRow | null): Exclude<Result, "failed"> {
	if (held === null) return pg !== null && pg.engaged ? "missing" : "absent_ok"
	const counts = overlayCounts(held.value)
	if (counts === null) return "unsupported"
	if (pg === null || !pg.engaged) return "stale"
	const sameScore = Math.abs(pg.rankingScore - held.score) <= TOLERANCE * Math.max(1, Math.abs(pg.rankingScore))
	const want = [pg.positive, pg.negative, pg.stancePositive, pg.stanceNegative, pg.commenterCount]
	return sameScore && want.every((n, i) => n === counts[i]) ? "match" : "mismatch"
}

export const MISMATCH_DELTAS = ["none", "lt_0.01", "lt_1", "lt_100", "ge_100"] as const
export type MismatchDelta = (typeof MISMATCH_DELTAS)[number]

const FIELD_NAMES = [
	"score",
	"positive",
	"negative",
	"stance_positive",
	"stance_negative",
	"commenter",
	"watch",
	"age_credit",
	"trending",
] as const

/**
 * Why a `mismatch` row mismatches: which fields differ (joined with `+`, or `many` past three) and the size of the score gap in
 * buckets. It reads the three terms the overlay value carries beside the counts (watch, age credit, trending), which `compareRow`
 * does not hold, so a row can show a differing term here yet still count as `match`; only rows `compareRow` calls `mismatch`
 * are labelled. Returns null for another layout. No ids. Pure.
 */
export function mismatchReason(pg: PgRow, held: DerivedRow): {fields: string; delta: MismatchDelta} | null {
	const counts = overlayCounts(held.value)
	if (counts === null) return null
	const v = held.value
	const same = (a: number, b: number) => Math.abs(a - b) <= TOLERANCE * Math.max(1, Math.abs(a))
	const diff = [
		!same(pg.rankingScore, held.score),
		pg.positive !== counts[0],
		pg.negative !== counts[1],
		pg.stancePositive !== counts[2],
		pg.stanceNegative !== counts[3],
		pg.commenterCount !== counts[4],
		!same(pg.watchScore, v.readDoubleLE(22)),
		!same(pg.ageCredit, v.readDoubleLE(30)),
		!same(pg.trendingScore, v.readDoubleLE(38)),
	]
	const names = FIELD_NAMES.filter((_, i) => diff[i])
	const gap = Math.abs(pg.rankingScore - held.score)
	const delta: MismatchDelta = !diff[0]
		? "none"
		: gap < 0.01
			? "lt_0.01"
			: gap < 1
				? "lt_1"
				: gap < 100
					? "lt_100"
					: "ge_100"
	return {fields: names.length === 0 ? "none" : names.length > 3 ? "many" : names.join("+"), delta}
}

/** The four scores a default row is held to, from a Postgres row, as text. */
export type PgScores = {quality: number; intrinsic: number; typeWeight: number; ranking: number}

/** Whether geo-db's served row has the Postgres row's four scores, within tolerance. Pure. */
export function compareDefault(pg: PgScores, served: ScoreRow): "match" | "mismatch" | "engaged" {
	if (served.engaged) return "engaged"
	const same = (a: number, b: number) => Math.abs(a - b) <= TOLERANCE * Math.max(1, Math.abs(a))
	return same(pg.quality, served.quality) &&
		same(pg.intrinsic, served.intrinsic) &&
		same(pg.typeWeight, served.typeWeight) &&
		same(pg.ranking, served.ranking)
		? "match"
		: "mismatch"
}

const hex = (uuid: string) => uuid.replace(/-/g, "").toLowerCase()
const isId = (v: unknown): v is string =>
	typeof v === "string" && /^[0-9a-f]{8}-?([0-9a-f]{4}-?){3}[0-9a-f]{12}$/i.test(v)
const uuidOfHex = (h: string) =>
	`${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`

let active = false
const results = new Map<Result, number>()
const bump = (r: Result): void => void results.set(r, (results.get(r) ?? 0) + 1)
const mismatchReasons = new Map<string, number>()
const defaults = new Map<DefaultResult, number>()
const bumpDefault = (r: DefaultResult): void => void defaults.set(r, (defaults.get(r) ?? 0) + 1)

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderRankingScoresShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_ranking_scores_shadow_total Sampled entity_ranking_scores rows against geo-db's engaged-only overlay in derived collection 21, by result: match, mismatch, absent_ok (un-engaged and no overlay row), missing (engaged, no overlay row), stale (overlay row for an un-engaged or absent Postgres row), unsupported (another layout) or failed.",
		"# TYPE gaia_api_ranking_scores_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_ranking_scores_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_ranking_scores_shadow_mismatch_total Rows counted mismatch, by the fields that differ (score, positive, negative, stance_positive, stance_negative, commenter, watch, age_credit, trending; joined with +, many past three) and the score gap bucket. Counts, not distinct rows: the overlay direction re-reads the top page every pass.",
		"# TYPE gaia_api_ranking_scores_shadow_mismatch_total counter",
	)
	for (const [key, n] of [...mismatchReasons].sort())
		lines.push(`gaia_api_ranking_scores_shadow_mismatch_total{${key}} ${n}`)
	lines.push(
		"# HELP gaia_api_ranking_scores_default_shadow_total Sampled un-engaged entity_ranking_scores rows against the default row geo-db builds from the entity's card and the ranking config (DerivedScores), by result: match, mismatch (a score differs; a stale Postgres snapshot is benign), engaged (geo-db holds an overlay row), no_card (no entities row) or failed.",
		"# TYPE gaia_api_ranking_scores_default_shadow_total counter",
	)
	for (const r of DEFAULT_RESULTS)
		lines.push(`gaia_api_ranking_scores_default_shadow_total{result="${r}"} ${defaults.get(r) ?? 0}`)
	return lines.join("\n")
}

export function __resetRankingScoresShadowMetricsForTests(): void {
	active = false
	results.clear()
	mismatchReasons.clear()
	defaults.clear()
}

const COLUMNS = sql.raw(
	"ranking_score::text AS ranking_score, positive::text AS positive, negative::text AS negative, stance_positive::text AS stance_positive, stance_negative::text AS stance_negative, commenter_count::text AS commenter_count, participation_score::text AS participation_score, comment_score::text AS comment_score, topic_score::text AS topic_score, watch_score::text AS watch_score, age_credit::text AS age_credit, trending_score::text AS trending_score, trending_source",
)

function rowsOf(result: unknown): Record<string, unknown>[] {
	return ((result as {rows?: Record<string, unknown>[]}).rows ?? []) as Record<string, unknown>[]
}

/** The Postgres row of an entity, or null when it has none. Throws on a database failure. */
export async function readPgRow(db: Database, entityId: string): Promise<PgRow | null> {
	const rows = rowsOf(
		await db.execute(sql`SELECT ${COLUMNS} FROM entity_ranking_scores WHERE entity_id = ${entityId}::uuid LIMIT 1`),
	)
	const first = rows[0]
	return first === undefined ? null : toPgRow(first)
}

/** Up to `limit` entity ids from the primary-key position `pivot`, wrapping to the start when short. */
export async function sampleIds(db: Database, pivot: string, limit: number): Promise<string[]> {
	const read = async (from: string | null, n: number) =>
		rowsOf(
			await db.execute(
				from === null
					? sql`SELECT entity_id::text AS entity_id FROM entity_ranking_scores ORDER BY entity_id LIMIT ${n}`
					: sql`SELECT entity_id::text AS entity_id FROM entity_ranking_scores WHERE entity_id >= ${from}::uuid ORDER BY entity_id LIMIT ${n}`,
			),
		)
			.map((r) => r.entity_id)
			.filter(isId)
			.map(hex)
	const first = await read(pivot, limit)
	if (first.length >= limit) return first
	const seen = new Set(first)
	return first.concat((await read(null, limit)).filter((k) => !seen.has(k)).slice(0, limit - first.length))
}

function idList(ids: string[]) {
	return sql.join(
		ids.map((id) => sql`${id}::uuid`),
		sql`, `,
	)
}

const TYPES_PROPERTY = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"

/** The Postgres rows of up to 64 entities, un-engaged ones only, with the scores a default is held to. Throws on a database failure. */
export async function readUnengagedRows(db: Database, ids: string[]): Promise<Map<string, PgScores>> {
	const rows = rowsOf(
		await db.execute(
			sql`SELECT entity_id::text AS entity_id, quality_score::text AS quality_score, intrinsic_score::text AS intrinsic_score, type_weight::text AS type_weight, ${COLUMNS} FROM entity_ranking_scores WHERE entity_id IN (${idList(ids)})`,
		),
	)
	const out = new Map<string, PgScores>()
	for (const row of rows) {
		const pg = toPgRow(row)
		const quality = Number(row.quality_score)
		const intrinsic = Number(row.intrinsic_score)
		const typeWeight = Number(row.type_weight)
		if (
			!isId(row.entity_id) ||
			pg === null ||
			pg.engaged ||
			![quality, intrinsic, typeWeight].every(Number.isFinite)
		)
			continue
		out.set(hex(row.entity_id), {quality, intrinsic, typeWeight, ranking: pg.rankingScore})
	}
	return out
}

/** The cards of the entities Postgres holds (an entity with no row is absent from the result). Throws on a database failure. */
export async function readCards(db: Database, ids: string[]): Promise<Map<string, ScoreCard>> {
	const rows = rowsOf(
		await db.execute(
			sql`SELECT e.id::text AS id,
				CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint::text END AS created_epoch,
				(SELECT count(DISTINCT v.property_id) FROM "values" v WHERE v.entity_id = e.id)::text AS property_count,
				(SELECT count(*) FROM relations r WHERE r.from_entity_id = e.id AND r.is_system = false)::text AS relation_count,
				(SELECT MAX(w.weight) FROM relations r JOIN entity_type_weights w ON w.type_id = r.to_entity_id WHERE r.from_entity_id = e.id AND r.type_id = ${TYPES_PROPERTY}::uuid)::text AS type_weight_max
				FROM entities e WHERE e.id IN (${idList(ids)})`,
		),
	)
	const out = new Map<string, ScoreCard>()
	for (const row of rows) {
		if (!isId(row.id)) continue
		const propertyCount = Number(row.property_count)
		const relationCount = Number(row.relation_count)
		const createdEpoch =
			row.created_epoch === null || row.created_epoch === undefined ? null : Number(row.created_epoch)
		const typeWeightMax =
			row.type_weight_max === null || row.type_weight_max === undefined ? null : Number(row.type_weight_max)
		if (
			!Number.isInteger(propertyCount) ||
			!Number.isInteger(relationCount) ||
			(createdEpoch !== null && !Number.isSafeInteger(createdEpoch)) ||
			(typeWeightMax !== null && !Number.isFinite(typeWeightMax))
		)
			continue
		out.set(hex(row.id), {entityHex: hex(row.id), propertyCount, relationCount, createdEpoch, typeWeightMax})
	}
	return out
}

/** The ranking config row as geo-db takes it, or null when the table has none. Throws on a database failure. */
export async function readScoreConfig(db: Database): Promise<ScoreConfig | null> {
	const row = rowsOf(
		await db.execute(
			sql`SELECT tau_seconds::text AS tau, wilson_z::text AS z, prior_positive::text AS pp, prior_negative::text AS pn, quality_floor::text AS floor, intrinsic_cap::text AS cap, intrinsic_property_target::text AS pt, intrinsic_relation_target::text AS rt, participation_weight::text AS pw, participation_cap::text AS pc, comment_weight::text AS cw, comment_cap::text AS cc FROM entity_ranking_config WHERE id`,
		),
	)[0]
	if (row === undefined) return null
	const config: ScoreConfig = {
		tauSeconds: Number(row.tau),
		wilsonZ: Number(row.z),
		priorPositive: Number(row.pp),
		priorNegative: Number(row.pn),
		qualityFloor: Number(row.floor),
		intrinsicCap: Number(row.cap),
		intrinsicPropertyTarget: Number(row.pt),
		intrinsicRelationTarget: Number(row.rt),
		participationWeight: Number(row.pw),
		participationCap: Number(row.pc),
		commentWeight: Number(row.cw),
		commentCap: Number(row.cc),
	}
	return Object.values(config).every(Number.isFinite) ? config : null
}

export type RankingScoresShadow = {
	/** One sampling pass. Resolves when done; never throws. */
	runOnce(): Promise<void>
	/** Start the periodic pass; returns the stop function. The timer does not keep the process alive. */
	start(): () => void
}

export function createRankingScoresShadow(deps: {
	config: RankingScoresShadowConfig
	db: Database
	reader: DerivedReader & Partial<Pick<DerivedScanReader, "scan"> & Pick<DerivedScoresReader, "scores">>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
}): RankingScoresShadow {
	const {config, db, reader, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	active = true
	let running = false
	let logged = 0

	const record = (outcome: Result, pg?: PgRow | null, held?: DerivedRow) => {
		bump(outcome)
		if (outcome === "mismatch" && pg && held) {
			const why = mismatchReason(pg, held)
			if (why) {
				const key = `fields="${why.fields}",delta="${why.delta}"`
				mismatchReasons.set(key, (mismatchReasons.get(key) ?? 0) + 1)
			}
		}
		// Counts only: never which entity.
		if ((outcome === "mismatch" || outcome === "stale") && logged < 20) {
			logged++
			log.warn("ranking scores shadow disagreement", {result: outcome})
		}
	}
	const fail = (error: unknown) => {
		bump("failed")
		if (!(error instanceof GeoDbError)) log.warn("ranking scores shadow failed", {error: String(error)})
	}

	async function viaOverlay(): Promise<void> {
		if (!reader.scan) return
		let page: Awaited<ReturnType<NonNullable<typeof reader.scan>>>
		try {
			page = await reader.scan(COLLECTION, "0", config.sample)
		} catch (error) {
			return fail(error)
		}
		for (const r of page.rows) {
			try {
				const held: DerivedRow = {group: "0".repeat(32), score: r.score, value: Buffer.from(r.valueHex, "hex")}
				const pg = await readPgRow(db, uuidOfHex(r.keyHex))
				record(compareRow(pg, held), pg, held)
			} catch (error) {
				fail(error)
			}
		}
	}

	async function viaPostgres(): Promise<void> {
		let ids: string[]
		try {
			ids = await sampleIds(db, pivotOf(), config.sample)
		} catch (error) {
			return fail(error)
		}
		for (const id of ids) {
			try {
				const pg = await readPgRow(db, uuidOfHex(id))
				const row = await reader.get(COLLECTION, id)
				record(compareRow(pg, row), pg, row ?? undefined)
			} catch (error) {
				fail(error)
			}
		}
	}

	/** Un-engaged rows from beyond the top page, against the default geo-db builds from their cards. */
	async function viaDefault(): Promise<void> {
		if (!reader.scores) return
		try {
			const sampled = await sampleIds(db, pivotOf(), config.defaultSample)
			const pg =
				sampled.length === 0 ? new Map<string, PgScores>() : await readUnengagedRows(db, sampled.map(uuidOfHex))
			const wanted = [...pg.keys()]
			if (wanted.length === 0) return
			const [cards, scoreConfig] = await Promise.all([readCards(db, wanted.map(uuidOfHex)), readScoreConfig(db)])
			if (scoreConfig === null) throw new Error("entity_ranking_config has no usable row")
			const withCard = wanted.filter((id) => cards.has(id))
			for (let i = 0; i < wanted.length - withCard.length; i++) bumpDefault("no_card")
			if (withCard.length === 0) return
			const served = await reader.scores(
				scoreConfig,
				withCard.map((id) => cards.get(id) as ScoreCard),
			)
			withCard.forEach((id, i) => {
				const row = served[i]
				const expected = pg.get(id)
				if (row === undefined || expected === undefined) return bumpDefault("failed")
				bumpDefault(compareDefault(expected, row))
			})
		} catch (error) {
			bumpDefault("failed")
			if (!(error instanceof GeoDbError)) log.warn("ranking scores default shadow failed", {error: String(error)})
		}
	}

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		try {
			await viaOverlay()
			await viaPostgres()
			await viaDefault()
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
