/**
 * The feed signal runs watermark twin (zero-Postgres critical path). `readSeen` (forYou/queries.ts) answers every `/for-you` page
 * with `max(data_through)` over the `personalization.feed_signal_runs` rows whose status is `succeeded`: the watermark the seen
 * counts are as fresh as. The writer side is ranking-indexer's feed_signal_runs_mirror.rs (GEODB_FEED_RUNS_MIRROR, off), which
 * keeps the newest row of each status in geo-db's `cursors` collection (10) under `feed_signal_run:<status>`, value
 * `<finished_at in microseconds>:<data_through RFC 3339, or "-">/<generation>`. This is the reader.
 *
 * Which statuses count. Only `succeeded`, as in `readSeen` and in `feed_signal_window` (api/drizzle/0109): `unchanged` means
 * analytics published nothing newer, so it wrote no signals and its `data_through` is not data the tables hold; `failed` wrote
 * nothing. (`feed_signal_freshness` also counts `unchanged` for the age of the last run, but takes the watermark from `succeeded`
 * alone; that is a different reader.) So the twin does one `get` of `feed_signal_run:succeeded`.
 *
 * Why the newest row's `data_through` equals the max. `feed_signal_window` declares a run `unchanged` unless the offered
 * watermark is newer than the last succeeded one, so succeeded watermarks only rise with `finished_at`; the mirror holds the newest
 * `finished_at`, which is the max. Postgres' `timestamptz` has microseconds and a JS Date milliseconds: both sides are read into
 * Dates, so "same instant" is to the millisecond.
 *
 * The twin serves nothing: GEODB_FEED_RUNS_SHADOW is off by default and `readSeen` stays on Postgres. When on, a timer
 * (`createIntervalSampler`, default every 5 minutes) reads Postgres' max (one aggregate over the `succeeded` rows, which the
 * `(status, finished_at)` index finds and the retention window keeps to a few thousand rows at most; off the request path) and
 * geo-db's row and compares the instants:
 *
 * `gaia_api_feed_runs_shadow_total{result}`:
 *  - match: the same instant, or neither side has a succeeded run;
 *  - mismatch: both have one and the instants differ, or Postgres has none and geo-db does;
 *  - absent: geo-db holds no `feed_signal_run:succeeded` row while Postgres has a watermark. This is the state until
 *    GEODB_FEED_RUNS_MIRROR is on, and is not a disagreement;
 *  - malformed: the row is not the mirror's shape;
 *  - failed: a geo-db read failed;
 *  - reference_timeout / reference_failed: the Postgres read hit its statement timeout, or failed otherwise (referenceSide.ts).
 * Logs counts only, never a value.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import type {DerivedReader} from "./derived"
import {createIntervalSampler, type IntervalSampler, readIntervalSamplerConfig} from "./intervalSampler"
import {failureResult, viaReference} from "./referenceSide"

/** The `cursors` collection (geo-db 6a770ba752); the mirror's `GEODB_CURSOR_COLLECTION` default. */
export const CURSORS_COLLECTION = 10
/** The only status whose `data_through` is a watermark: see the file comment. */
export const COMPLETED_STATUS = "succeeded"
export const feedRunKey = (status: string): string => `feed_signal_run:${status}`
const keyHex = (key: string): string => Buffer.from(key, "utf8").toString("hex")

export type FeedRunsShadowConfig = {enabled: boolean; intervalMs: number}

export function readFeedRunsShadowConfig(env: Record<string, string | undefined>): FeedRunsShadowConfig {
	return readIntervalSamplerConfig(env, "GEODB_FEED_RUNS_SHADOW", "GEODB_FEED_RUNS_SHADOW_INTERVAL_MS")
}

/** A mirror value, `<block>:<cursor>` with cursor `<data_through|->/<generation>`; `null` is a malformed value. */
export function parseFeedRunValue(value: string): {finishedAt: Date; dataThrough: Date | null} | null {
	const colon = value.indexOf(":")
	if (colon < 0) return null
	const block = value.slice(0, colon)
	if (!/^\d+$/.test(block)) return null
	const slash = value.indexOf("/", colon + 1)
	if (slash < 0) return null
	const through = value.slice(colon + 1, slash)
	const finishedAt = new Date(Math.floor(Number(block) / 1000))
	if (Number.isNaN(finishedAt.getTime())) return null
	if (through === "-") return {finishedAt, dataThrough: null}
	if (!/^\d{4}-\d\d-\d\dT[\d:.]+Z$/.test(through)) return null
	const dataThrough = new Date(through)
	return Number.isNaN(dataThrough.getTime()) ? null : {finishedAt, dataThrough}
}

export class FeedRunMalformed extends Error {}

/**
 * The derived twin of `max(data_through)` over the succeeded runs: `{present: false}` when geo-db holds no row,
 * `{present: true, watermark: null}` when the row has no cutoff. Rejects FeedRunMalformed or GeoDbError.
 */
export async function readFeedRunsWatermark(
	reader: Pick<DerivedReader, "get">,
): Promise<{present: false} | {present: true; watermark: Date | null}> {
	const row = await reader.get(CURSORS_COLLECTION, keyHex(feedRunKey(COMPLETED_STATUS)))
	if (!row) return {present: false}
	const parsed = parseFeedRunValue(row.value.toString("utf8"))
	if (!parsed) throw new FeedRunMalformed()
	return {present: true, watermark: parsed.dataThrough}
}

export const RESULTS = [
	"match",
	"mismatch",
	"absent",
	"malformed",
	"failed",
	"reference_timeout",
	"reference_failed",
] as const
export type Result = (typeof RESULTS)[number]

/** Same instant (to the millisecond), or both empty. Pure. */
export function compareWatermarks(
	postgres: Date | null,
	geo: {present: false} | {present: true; watermark: Date | null},
): Result {
	if (!geo.present) return postgres === null ? "match" : "absent"
	if (postgres === null || geo.watermark === null) return postgres === geo.watermark ? "match" : "mismatch"
	return postgres.getTime() === geo.watermark.getTime() ? "match" : "mismatch"
}

let active = false
const results = new Map<Result, number>()
const bump = (result: Result): void => {
	results.set(result, (results.get(result) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once on, every series exists at zero. */
export function renderFeedRunsShadowMetrics(): string {
	if (!active) return ""
	return [
		"# HELP gaia_api_feed_runs_shadow_total Whether geo-db's mirrored newest succeeded feed_signal_runs row (cursors collection 10) holds the same data_through instant as Postgres' max over the succeeded runs, by result: match, mismatch, absent (geo-db has no row: the mirror is not on, not a disagreement), malformed, failed (geo-db) or reference_timeout / reference_failed (the Postgres read).",
		"# TYPE gaia_api_feed_runs_shadow_total counter",
		...RESULTS.map((r) => `gaia_api_feed_runs_shadow_total{result="${r}"} ${results.get(r) ?? 0}`),
	].join("\n")
}

export function __resetFeedRunsShadowMetricsForTests(): void {
	active = false
	results.clear()
}

/** Postgres' watermark: the same filter `readSeen` applies. */
export async function readPostgresWatermark(db: Database): Promise<Date | null> {
	const result = await db.execute(
		sql`SELECT max(r.data_through) AS watermark FROM personalization.feed_signal_runs r WHERE r.status = ${COMPLETED_STATUS}`,
	)
	const value = ((result as {rows?: Record<string, unknown>[]}).rows ?? [])[0]?.watermark
	if (value === null || value === undefined) return null
	const at = new Date(value as string | Date)
	if (Number.isNaN(at.getTime())) throw new Error("feed_signal_runs watermark is not a time")
	return at
}

export type FeedRunsShadow = IntervalSampler

export function createFeedRunsShadow(deps: {
	config: FeedRunsShadowConfig
	db: Database
	reader: Pick<DerivedReader, "get">
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	readWatermark?: typeof readPostgresWatermark
}): FeedRunsShadow {
	const {config, db, reader, log} = deps
	const readWatermark = deps.readWatermark ?? readPostgresWatermark
	active = true
	let logged = 0

	async function pass(): Promise<void> {
		try {
			const postgres = await viaReference(readWatermark(db))
			const outcome = compareWatermarks(postgres, await readFeedRunsWatermark(reader))
			bump(outcome)
			if (outcome === "mismatch" && logged < 20) {
				logged++
				log.warn("feed runs shadow: watermarks differ")
			}
		} catch (error) {
			if (error instanceof FeedRunMalformed) {
				bump("malformed")
				if (logged < 20) {
					logged++
					log.warn("feed runs shadow: mirrored row is not the mirror's shape")
				}
				return
			}
			const result = failureResult(error)
			bump(result)
			if (result !== "failed") log.warn("feed runs shadow failed", {result, error: String(error)})
		}
	}

	return createIntervalSampler({
		enabled: config.enabled,
		intervalMs: config.intervalMs,
		pass,
		log,
		label: "feed runs shadow",
	})
}
