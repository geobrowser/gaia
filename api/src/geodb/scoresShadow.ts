/**
 * The score tables' shadow (GEODB_SCORES_SHADOW, off by default).
 *
 * `global_scores`, `local_scores` and `space_scores` are read by the api only INSIDE SQL (the LOCAL_SCORE / GLOBAL_SCORE order-by
 * subselects of kg/valueOrderByScorePlugin.ts and the `entities_ordered_by_score` function), so no request has a served row to
 * compare. geo-db holds the same tables in derived collections 2, 3 and 4 (the scoring-service cronjob's geodb_derived mirror,
 * GEODB_DERIVED_MIRROR). This samples each table instead: every `intervalMs`, in the background, it reads up to `sample` rows
 * from Postgres starting at a random primary-key position (wrapping to the start) and compares each with `reader.get`.
 * It never touches a request.
 *
 * Layouts (scoring-service/cronjob/src/geodb_derived): collection 2 is group 0, key the 16-byte entity id; collection 3 is group
 * the space id (as u128), key the entity id then the space id (32 bytes); collection 4 is group 0, key the 16-byte space id.
 * No value; the score is the table's `score`.
 *
 * `gaia_api_scores_shadow_total{table,result}`: match (same score), mismatch (a different score), unsupported (geo-db has no row
 * Postgres has: the mirror has not run or lags; or the row has a value or the wrong group, which this layout never writes) or
 * failed (a read failed). Blind spot: only rows Postgres has are sampled, so a row geo-db keeps after Postgres dropped it (the
 * mirror never deletes) is not seen.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {GeoDbError} from "./client"
import type {DerivedReader, DerivedRow} from "./derived"

export type ScoresShadowConfig = {
	enabled: boolean
	intervalMs: number
	sample: number
}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readScoresShadowConfig(env: Record<string, string | undefined>): ScoresShadowConfig {
	const flag = env.GEODB_SCORES_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GEODB_SCORES_SHADOW_INTERVAL_MS, 60_000, 86_400_000),
		sample: wholeNumber(env.GEODB_SCORES_SHADOW_SAMPLE, 20, 200),
	}
}

export const TABLES = ["global_scores", "local_scores", "space_scores"] as const
export type ScoreTable = (typeof TABLES)[number]
export const COLLECTION: Record<ScoreTable, number> = {global_scores: 2, local_scores: 3, space_scores: 4}
export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const

/** Scores are f64 on both sides; Postgres's numeric text round trip may differ in the last digits. */
const TOLERANCE = 1e-9

/** A sampled Postgres row: the geo-db key (hex), the group it must be in (32 hex) and the served score. */
export type SampledRow = {keyHex: string; groupHex: string; score: number}

const hex = (uuid: string) => uuid.replace(/-/g, "").toLowerCase()
const ZERO_GROUP = "0".repeat(32)

let active = false
const results = new Map<string, number>()
const bump = (table: ScoreTable, result: (typeof RESULTS)[number]): void => {
	const k = `${table}\t${result}`
	results.set(k, (results.get(k) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderScoresShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_scores_shadow_total Sampled global_scores/local_scores/space_scores rows from Postgres against geo-db's derived collections 2/3/4, by table and result: match, mismatch, unsupported (geo-db has no such row or another layout) or failed (a read failed).",
		"# TYPE gaia_api_scores_shadow_total counter",
	]
	for (const t of TABLES)
		for (const r of RESULTS)
			lines.push(`gaia_api_scores_shadow_total{table="${t}",result="${r}"} ${results.get(`${t}\t${r}`) ?? 0}`)
	return lines.join("\n")
}

export function __resetScoresShadowMetricsForTests(): void {
	active = false
	results.clear()
}

/** What a sampled Postgres row and geo-db's row say about each other. Pure. */
export function compareScore(sampled: SampledRow, held: DerivedRow | null): "match" | "mismatch" | "unsupported" {
	if (held === null) return "unsupported"
	if (held.value.length !== 0 || held.group !== sampled.groupHex) return "unsupported"
	return Math.abs(sampled.score - held.score) <= TOLERANCE * Math.max(1, Math.abs(sampled.score))
		? "match"
		: "mismatch"
}

function rowsOf(result: unknown): Record<string, unknown>[] {
	return ((result as {rows?: Record<string, unknown>[]}).rows ?? []) as Record<string, unknown>[]
}

/** The geo-db key and group of a Postgres row of `table`, or null when an id is not a UUID or the score not a number. */
export function toSampledRow(table: ScoreTable, row: Record<string, unknown>): SampledRow | null {
	const score = Number(row.score)
	if (!Number.isFinite(score)) return null
	const isId = (v: unknown): v is string =>
		typeof v === "string" && /^[0-9a-f]{8}-?([0-9a-f]{4}-?){3}[0-9a-f]{12}$/i.test(v)
	if (table === "global_scores") {
		return isId(row.entity_id) ? {keyHex: hex(row.entity_id), groupHex: ZERO_GROUP, score} : null
	}
	if (table === "space_scores") {
		return isId(row.space_id) ? {keyHex: hex(row.space_id), groupHex: ZERO_GROUP, score} : null
	}
	if (!isId(row.entity_id) || !isId(row.space_id)) return null
	return {keyHex: hex(row.entity_id) + hex(row.space_id), groupHex: hex(row.space_id), score}
}

/** Up to `limit` rows of `table` from the primary-key position `pivot` (a UUID), wrapping to the start when short. */
export async function sampleTable(
	db: Database,
	table: ScoreTable,
	pivot: string,
	limit: number,
): Promise<SampledRow[]> {
	const read = async (from: string | null, n: number) => {
		const result =
			table === "global_scores"
				? await db.execute(
						from === null
							? sql`SELECT entity_id::text, score::text FROM global_scores ORDER BY entity_id LIMIT ${n}`
							: sql`SELECT entity_id::text, score::text FROM global_scores WHERE entity_id >= ${from}::uuid ORDER BY entity_id LIMIT ${n}`,
					)
				: table === "space_scores"
					? await db.execute(
							from === null
								? sql`SELECT space_id::text, score::text FROM space_scores ORDER BY space_id LIMIT ${n}`
								: sql`SELECT space_id::text, score::text FROM space_scores WHERE space_id >= ${from}::uuid ORDER BY space_id LIMIT ${n}`,
						)
					: await db.execute(
							from === null
								? sql`SELECT entity_id::text, space_id::text, score::text FROM local_scores ORDER BY entity_id, space_id LIMIT ${n}`
								: sql`SELECT entity_id::text, space_id::text, score::text FROM local_scores WHERE entity_id >= ${from}::uuid ORDER BY entity_id, space_id LIMIT ${n}`,
						)
		return rowsOf(result)
			.map((r) => toSampledRow(table, r))
			.filter((r): r is SampledRow => r !== null)
	}
	const first = await read(pivot, limit)
	if (first.length >= limit) return first
	// The wrap re-reads from the start, which a small table (or a pivot near the front) overlaps: keep each row once.
	const seen = new Set(first.map((r) => r.keyHex))
	return first.concat((await read(null, limit)).filter((r) => !seen.has(r.keyHex)).slice(0, limit - first.length))
}

export type ScoresShadow = {
	/** One sampling pass over the three tables. Resolves when done; never throws. */
	runOnce(): Promise<void>
	/** Start the periodic pass; returns the stop function. The timer does not keep the process alive. */
	start(): () => void
}

export function createScoresShadow(deps: {
	config: ScoresShadowConfig
	db: Database
	reader: DerivedReader
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
}): ScoresShadow {
	const {config, db, reader, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	active = true
	let running = false
	let logged = 0

	async function pass(table: ScoreTable): Promise<void> {
		let sampled: SampledRow[]
		try {
			sampled = await sampleTable(db, table, pivotOf(), config.sample)
		} catch (error) {
			bump(table, "failed")
			log.warn("scores shadow sample failed", {table, error: String(error)})
			return
		}
		for (const row of sampled) {
			try {
				const outcome = compareScore(row, await reader.get(COLLECTION[table], row.keyHex))
				bump(table, outcome)
				// Counts only: never which entity.
				if (outcome === "mismatch" && logged < 20) {
					logged++
					log.warn("scores shadow mismatch", {table})
				}
			} catch (error) {
				bump(table, "failed")
				if (!(error instanceof GeoDbError)) log.warn("scores shadow failed", {table, error: String(error)})
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
