/**
 * The ordered entity reads' shadow (GEODB_ENTITIES_BEST_ORDER_SHADOW, off by default; zero-Postgres critical path, ZPL-2).
 *
 * `entities_in_best_order` and `entities_ordered_by_score` return a PAGE, so what has to agree is the ORDER of the ids, ties
 * included, not a row here and there. Each pass, in the background and never touching a request, builds a page both ways and
 * compares the id lists position by position:
 *
 *  - best_order: `sample` entity ids with a score row (from a random primary-key position), put through the real
 *    `public.entities_in_best_order`; the twin (`rankedReads/bestOrder.ts`) orders the same ids from the ranking scores geo-db
 *    serves (`DerivedScores`: overlay collection 21 if engaged, else the default from the card and the ranking config);
 *  - global_score: the first `sample` entities of `global_scores` by the function's own query (score DESC, id ASC), against
 *    the first `sample` rows of derived collection 2 (a scan is score DESC, key ASC) ordered by the twin;
 *  - local_score: the same for one space of `local_scores` (a random row's space) against collection 3.
 *
 * `gaia_api_entities_best_order_shadow_total{read,result}`: match (same ids in the same positions), mismatch (any difference:
 * order, a missing or extra id) or failed (a geo-db read failed; a node below frame revision 11 refuses best_order). A Postgres
 * reference query that fails is not geo-db's fault: reference_timeout (statement timeout, SQLSTATE 57014) or reference_failed.
 *
 * Reading the numbers: best_order compares geo-db's SERVED scores with the stored `entity_ranking_scores` snapshot, so an
 * entity whose card moved since its last refresh (nothing re-scores on a card edit) can reorder a list; read the rate, not one
 * count. The score reads hold f64 scores to Postgres `numeric`: two distinct numerics that collapse to one f64 tie here and not
 * there. The global and local reads run the function's query, not the function: a plpgsql `RETURN QUERY` materialises the whole
 * table before any LIMIT, which a sampler must not do.
 *
 * Not covered: ASC (geo-db cannot scan upward), `raw` (votes_count has no geo-db reader), entities with no score row (the SQL
 * puts them last in the caller's order; best_order samples only scored ids), the 1000-id cap (pinned by the golden test), and
 * duplicates and unknown ids in a list (golden only). Logs counts only, never an entity.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {GeoDbError} from "./client"
import type {DerivedScanReader, DerivedScoresReader, ScoreCard} from "./derived"
import {entitiesInBestOrder, entitiesOrderedByScore} from "./rankedReads/bestOrder"
import {readCards, readScoreConfig, sampleIds} from "./rankingScoresShadow"
import {failureResult, viaReference} from "./referenceSide"
import {sampleTable} from "./scoresShadow"

export const READS = ["best_order", "global_score", "local_score"] as const
export type Read = (typeof READS)[number]
export const RESULTS = ["match", "mismatch", "failed", "reference_timeout", "reference_failed"] as const
export type Result = (typeof RESULTS)[number]

export type BestOrderShadowConfig = {enabled: boolean; intervalMs: number; sample: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readBestOrderShadowConfig(env: Record<string, string | undefined>): BestOrderShadowConfig {
	const flag = env.GEODB_ENTITIES_BEST_ORDER_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GEODB_ENTITIES_BEST_ORDER_SHADOW_INTERVAL_MS, 60_000, 86_400_000),
		// geo-db takes 64 cards per DerivedScores request.
		sample: wholeNumber(env.GEODB_ENTITIES_BEST_ORDER_SHADOW_SAMPLE, 20, 64),
	}
}

/** Whether two id lists hold the same ids in the same positions. Pure. */
export function samePage(pg: readonly string[], twin: readonly string[]): boolean {
	return pg.length === twin.length && pg.every((id, i) => id === twin[i])
}

let active = false
const counts = new Map<string, number>()
const bump = (read: Read, r: Result): void => {
	const k = `${read}\t${r}`
	counts.set(k, (counts.get(k) ?? 0) + 1)
}

/** Empty until mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderBestOrderShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_entities_best_order_shadow_total Sampled pages of entities_in_best_order and entities_ordered_by_score against the same pages built from geo-db (derived collections 2, 3 and 21), by read and result: match (same ids in the same positions), mismatch, failed (geo-db) or reference_timeout / reference_failed (the Postgres side). Compares order, ties included.",
		"# TYPE gaia_api_entities_best_order_shadow_total counter",
	]
	for (const read of READS)
		for (const r of RESULTS)
			lines.push(
				`gaia_api_entities_best_order_shadow_total{read="${read}",result="${r}"} ${counts.get(`${read}\t${r}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetBestOrderShadowMetricsForTests(): void {
	active = false
	counts.clear()
}

function rowsOf(result: unknown): Record<string, unknown>[] {
	return ((result as {rows?: Record<string, unknown>[]}).rows ?? []) as Record<string, unknown>[]
}
const hex = (uuid: string) => uuid.replace(/-/g, "").toLowerCase()
const uuidOfHex = (h: string) =>
	`${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
const idsOf = (result: unknown) => rowsOf(result).map((r) => hex(String(r.id)))

/** `entities_in_best_order(ids)`, the real function: ids in the order it returns them. Throws on a database failure. */
export async function pgBestOrder(db: Database, ids: string[]): Promise<string[]> {
	const list = sql.join(
		ids.map((id) => sql`${id}::uuid`),
		sql`, `,
	)
	return idsOf(
		await db.execute(sql`SELECT e.id::text AS id FROM public.entities_in_best_order(ARRAY[${list}]::uuid[]) e`),
	)
}

/** The query inside `entities_ordered_by_score('global', NULL, 'DESC')`, limited to a page. */
export async function pgGlobalPage(db: Database, first: number): Promise<string[]> {
	return idsOf(
		await db.execute(
			sql`SELECT e.id::text AS id FROM entities e INNER JOIN global_scores gs ON gs.entity_id = e.id ORDER BY gs.score DESC, e.id ASC LIMIT ${first}`,
		),
	)
}

/** The query inside `entities_ordered_by_score('local', space, 'DESC')`, limited to a page. */
export async function pgLocalPage(db: Database, spaceId: string, first: number): Promise<string[]> {
	return idsOf(
		await db.execute(
			sql`SELECT e.id::text AS id FROM entities e INNER JOIN local_scores ls ON ls.entity_id = e.id AND ls.space_id = ${spaceId}::uuid ORDER BY ls.score DESC, e.id ASC LIMIT ${first}`,
		),
	)
}

export type BestOrderShadow = {
	/** One sampling pass. Resolves when done; never throws. */
	runOnce(): Promise<void>
	/** Start the periodic pass; returns the stop function. The timer does not keep the process alive. */
	start(): () => void
}

export function createBestOrderShadow(deps: {
	config: BestOrderShadowConfig
	db: Database
	reader: DerivedScanReader & Partial<DerivedScoresReader>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
}): BestOrderShadow {
	const {config, db, reader, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	active = true
	let running = false
	let logged = 0

	const record = (read: Read, pg: readonly string[], twin: readonly string[]) => {
		const ok = samePage(pg, twin)
		bump(read, ok ? "match" : "mismatch")
		// Counts only: never which entity.
		if (!ok && logged < 20) {
			logged++
			log.warn("entities best order shadow disagreement", {read})
		}
	}
	const fail = (read: Read, error: unknown) => {
		bump(read, failureResult(error))
		if (!(error instanceof GeoDbError)) log.warn("entities best order shadow failed", {read, error: String(error)})
	}

	async function bestOrder(): Promise<void> {
		if (!reader.scores) return
		try {
			const ids = await viaReference(sampleIds(db, pivotOf(), config.sample))
			if (ids.length === 0) return
			const uuids = ids.map(uuidOfHex)
			const [pg, cards, scoreConfig] = await Promise.all([
				viaReference(pgBestOrder(db, uuids)),
				viaReference(readCards(db, uuids)),
				viaReference(readScoreConfig(db)),
			])
			if (scoreConfig === null) throw new Error("entity_ranking_config has no usable row")
			const withCard = ids.filter((id) => cards.has(id))
			const served =
				withCard.length === 0
					? []
					: await reader.scores(
							scoreConfig,
							withCard.map((id) => cards.get(id) as ScoreCard),
						)
			const ranking = new Map(withCard.map((id, i) => [id, served[i]?.ranking] as const))
			if ([...ranking.values()].some((v) => v === undefined))
				throw new GeoDbError("client_failed", "short scores reply")
			const twin = entitiesInBestOrder(ids, new Set(withCard), ranking as Map<string, number>)
			record("best_order", pg, twin)
		} catch (error) {
			fail("best_order", error)
		}
	}

	async function viaScan(read: "global_score" | "local_score"): Promise<void> {
		try {
			let collection = 2
			let group = "0"
			let pg: Promise<string[]>
			if (read === "global_score") {
				pg = viaReference(pgGlobalPage(db, config.sample))
			} else {
				const row = (await viaReference(sampleTable(db, "local_scores", pivotOf(), 1)))[0]
				if (row === undefined) return
				collection = 3
				group = row.groupHex
				pg = viaReference(pgLocalPage(db, uuidOfHex(row.groupHex), config.sample))
			}
			const [expected, page] = await Promise.all([pg, reader.scan(collection, group, config.sample)])
			// A collection-3 key is the entity id then the space id; collection 2's key is the entity id.
			const twin = entitiesOrderedByScore(
				page.rows.map((r) => ({id: r.keyHex.slice(0, 32), score: r.score})),
				null,
				"DESC",
			)
			record(read, expected, twin)
		} catch (error) {
			fail(read, error)
		}
	}

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		try {
			await bestOrder()
			await viaScan("global_score")
			await viaScan("local_score")
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
