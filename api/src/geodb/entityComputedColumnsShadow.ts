/**
 * The Entity computed-column shadow (GEODB_ENTITY_COLUMNS_SHADOW, off by default; zero-Postgres critical path, ZPL-1).
 *
 * Fourteen of the 17 GraphQL `Entity` computed columns (`entities_agrees` ... `entities_upvotes`) read one row of
 * `public.entity_ranking_scores`. geo-db serves that row (overlay in derived collection 21 when engaged, else the default built
 * from the card and the ranking config: `DerivedScores`, wire revision 11). Each pass samples entity ids from the score table,
 * calls the real SQL functions on them, maps geo-db's served rows back to the same columns with the pure twin
 * (`rankedReads/entityColumns.ts`, pinned to the SQL by a golden grid) and compares column by column. Never touches a request.
 *
 * `gaia_api_entity_columns_shadow_total{column,result}`:
 *  - match: the same value (numbers within 1e-9 relative, text and dashed uuids exactly, NULL on both sides);
 *  - mismatch: a different value. A score row is a snapshot of the entity's last refresh and nothing re-scores on a card edit,
 *    so an un-engaged entity whose card moved is a benign mismatch: read the rate, not one count;
 *  - failed: a read failed, or the node is below frame revision 11 and refused the request.
 *
 * Not shadowed: `last_listed_on`, `listed_on_agenda_id` and `trending_topic_listed_on`. They are live reads over the trending
 * listings, whose twin (`rankedReads/trending.ts`) is already held to the SQL by GEODB_RANKED_READS_SHADOW; geo-db has no
 * Postgres-free source for the rows yet. Entities with no `entity_ranking_scores` row are not sampled either (the SQL returns
 * NULL, geo-db serves a default: a known, deliberate difference). Logs counts only, never an entity.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {GeoDbError} from "./client"
import type {DerivedScoresReader, ScoreCard, ScoreRow} from "./derived"
import {type ColumnValue, SCORE_COLUMNS, type ScoreColumn, sameColumn, scoreColumns} from "./rankedReads/entityColumns"
import {readCards, readScoreConfig, sampleIds} from "./rankingScoresShadow"

export const RESULTS = ["match", "mismatch", "failed"] as const
export type Result = (typeof RESULTS)[number]

export type EntityColumnsShadowConfig = {enabled: boolean; intervalMs: number; sample: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readEntityColumnsShadowConfig(env: Record<string, string | undefined>): EntityColumnsShadowConfig {
	const flag = env.GEODB_ENTITY_COLUMNS_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GEODB_ENTITY_COLUMNS_SHADOW_INTERVAL_MS, 60_000, 86_400_000),
		// geo-db takes 64 cards per DerivedScores request.
		sample: wholeNumber(env.GEODB_ENTITY_COLUMNS_SHADOW_SAMPLE, 20, 64),
	}
}

/** Per column, whether the Postgres text values and the twin's served values agree. Pure. */
export function compareColumns(
	pg: Record<ScoreColumn, string | null>,
	served: ScoreRow,
): Record<ScoreColumn, "match" | "mismatch"> {
	const twin = scoreColumns(served) as Record<ScoreColumn, ColumnValue>
	return Object.fromEntries(
		SCORE_COLUMNS.map((c) => [c, sameColumn(c, pg[c], twin[c]) ? "match" : "mismatch"]),
	) as Record<ScoreColumn, "match" | "mismatch">
}

const hex = (uuid: string) => uuid.replace(/-/g, "").toLowerCase()
const uuidOfHex = (h: string) =>
	`${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`

let active = false
const counts = new Map<string, number>()
const bump = (column: ScoreColumn, r: Result): void => {
	const key = `${column}\u0000${r}`
	counts.set(key, (counts.get(key) ?? 0) + 1)
}

/** Empty until mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderEntityColumnsShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_entity_columns_shadow_total Sampled Entity computed columns (the entities_* SQL functions over entity_ranking_scores) against the row geo-db serves (DerivedScores), by column and result: match, mismatch (a stale Postgres snapshot is benign) or failed.",
		"# TYPE gaia_api_entity_columns_shadow_total counter",
	]
	for (const c of SCORE_COLUMNS)
		for (const r of RESULTS)
			lines.push(
				`gaia_api_entity_columns_shadow_total{column="${c}",result="${r}"} ${counts.get(`${c}\u0000${r}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetEntityColumnsShadowMetricsForTests(): void {
	active = false
	counts.clear()
}

const FUNCTION_COLUMNS = sql.raw(SCORE_COLUMNS.map((c) => `public.entities_${c}(e)::text AS ${c}`).join(", "))

/** The SQL functions' output for up to 64 entities, as text, keyed by entity hex. Throws on a database failure. */
export async function readPgColumns(
	db: Database,
	ids: string[],
): Promise<Map<string, Record<ScoreColumn, string | null>>> {
	const list = sql.join(
		ids.map((id) => sql`${id}::uuid`),
		sql`, `,
	)
	const result = (await db.execute(
		sql`SELECT e.id::text AS id, ${FUNCTION_COLUMNS} FROM entities e WHERE e.id IN (${list})`,
	)) as {rows?: Record<string, unknown>[]}
	const out = new Map<string, Record<ScoreColumn, string | null>>()
	for (const row of result.rows ?? []) {
		if (typeof row.id !== "string") continue
		out.set(
			hex(row.id),
			Object.fromEntries(
				SCORE_COLUMNS.map((c) => [c, row[c] === null || row[c] === undefined ? null : String(row[c])]),
			) as Record<ScoreColumn, string | null>,
		)
	}
	return out
}

export type EntityColumnsShadow = {
	/** One sampling pass. Resolves when done; never throws. */
	runOnce(): Promise<void>
	/** Start the periodic pass; returns the stop function. The timer does not keep the process alive. */
	start(): () => void
}

export function createEntityColumnsShadow(deps: {
	config: EntityColumnsShadowConfig
	db: Database
	reader: Partial<DerivedScoresReader>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
}): EntityColumnsShadow {
	const {config, db, reader, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	active = true
	let running = false
	let logged = 0

	const failAll = (error: unknown) => {
		for (const c of SCORE_COLUMNS) bump(c, "failed")
		if (!(error instanceof GeoDbError)) log.warn("entity columns shadow failed", {error: String(error)})
	}

	async function runOnce(): Promise<void> {
		if (running || !reader.scores) return
		running = true
		try {
			const ids = await sampleIds(db, pivotOf(), config.sample)
			if (ids.length === 0) return
			const uuids = ids.map(uuidOfHex)
			const [pg, cards, scoreConfig] = await Promise.all([
				readPgColumns(db, uuids),
				readCards(db, uuids),
				readScoreConfig(db),
			])
			if (scoreConfig === null) throw new Error("entity_ranking_config has no usable row")
			const wanted = ids.filter((id) => pg.has(id) && cards.has(id))
			if (wanted.length === 0) return
			const served = await reader.scores(
				scoreConfig,
				wanted.map((id) => cards.get(id) as ScoreCard),
			)
			wanted.forEach((id, i) => {
				const row = served[i]
				const expected = pg.get(id)
				if (row === undefined || expected === undefined)
					return failAll(new GeoDbError("client_failed", "no served row"))
				const verdicts = compareColumns(expected, row)
				let differs = false
				for (const c of SCORE_COLUMNS) {
					bump(c, verdicts[c])
					differs ||= verdicts[c] === "mismatch"
				}
				// Counts only: never which entity.
				if (differs && logged < 20) {
					logged++
					log.warn("entity columns shadow disagreement", {})
				}
			})
		} catch (error) {
			failAll(error)
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
