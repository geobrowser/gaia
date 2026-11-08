/**
 * Twin of the 17 GraphQL `Entity` computed-column SQL functions (`public.entities_agrees` ... `public.entities_upvotes`).
 *
 * Fourteen of them are one-row lookups of `public.entity_ranking_scores` by `entity_id` (migrations 0074, 0078, 0079, 0113). Each
 * returns NULL when the entity has no row, and the stored value otherwise. geo-db serves the same row, from the overlay in
 * derived collection 21 when the entity is engaged and from the default its card and the ranking config build when not
 * (`DerivedScores`, wire revision 11), so the twin maps a served `ScoreRow` back to the fourteen columns. `null` stands for "no
 * Postgres row"; geo-db always serves a row, so an entity Postgres has not scored yet is a place the two legitimately differ
 * (the shadow counts it as `pg_null`, not as a mismatch).
 *
 * The other three (`last_listed_on`, `listed_on_agenda_id`, `trending_topic_listed_on`) are LIVE reads over
 * `trending.entity_listings` / `trending.entity_topic_listings`, which the #1309 twin (`./trending.ts`) already reproduces over
 * in-memory rows; this module only picks the column out of that twin's single-entity answer.
 *
 * Mapping, with the Postgres type each value has:
 *   agrees = stance_positive, disagrees = stance_negative, upvotes = positive, downvotes = negative, commenters = commenter_count
 *   (bigint); quality_score, intrinsic_score, participation_score, comment_score, ranking_score, trending_score and
 *   trending_age_credit (= age_credit) (numeric); trending_source (text: NULL, 'listing' or 'topic'); trending_topic_id (uuid, set
 *   only when the source is 'topic').
 *
 * Pinned to golden output from a scratch Postgres by `__tests__/entityColumns.golden.test.ts`
 * (`api/scripts/entitycols/gen_golden.py`). SQL stays the source of truth; nothing here serves a request.
 */

import type {ScoreRow} from "../derived"
import type {Trending} from "./trending"

/** The columns that read `entity_ranking_scores`, named after the SQL function suffix (`entities_<name>`). */
export const SCORE_COLUMNS = [
	"agrees",
	"comment_score",
	"commenters",
	"disagrees",
	"downvotes",
	"intrinsic_score",
	"participation_score",
	"quality_score",
	"ranking_score",
	"trending_age_credit",
	"trending_score",
	"trending_source",
	"trending_topic_id",
	"upvotes",
] as const
export type ScoreColumn = (typeof SCORE_COLUMNS)[number]

/** The live columns over the trending listings. */
export const LISTING_COLUMNS = ["last_listed_on", "listed_on_agenda_id", "trending_topic_listed_on"] as const
export type ListingColumn = (typeof LISTING_COLUMNS)[number]

export const ALL_COLUMNS = [...SCORE_COLUMNS, ...LISTING_COLUMNS].sort() as (ScoreColumn | ListingColumn)[]

/** A column's value: a count or score, text, a dashed uuid or a `YYYY-MM-DD` date, or null. */
export type ColumnValue = number | string | null
export type ScoreColumns = Record<ScoreColumn, ColumnValue>
export type ListingColumns = Record<ListingColumn, ColumnValue>

/** `9f...` (32 hex digits) as the dashed lowercase uuid Postgres prints. */
export function dashed(hex: string): string {
	const h = hex.toLowerCase()
	return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
}

const SOURCES = [null, "listing", "topic"] as const

/** What the fourteen columns return for an entity whose entity_ranking_scores row is `row`; every column NULL when it has none. */
export function scoreColumns(row: ScoreRow | null): ScoreColumns {
	if (row === null) {
		return Object.fromEntries(SCORE_COLUMNS.map((c) => [c, null])) as ScoreColumns
	}
	const source = SOURCES[row.trendingSource] ?? null
	return {
		agrees: row.counts[2],
		comment_score: row.comment,
		commenters: row.counts[4],
		disagrees: row.counts[3],
		downvotes: row.counts[1],
		intrinsic_score: row.intrinsic,
		participation_score: row.participation,
		quality_score: row.quality,
		ranking_score: row.ranking,
		trending_age_credit: row.ageCredit,
		trending_score: row.trending,
		trending_source: source,
		// Stored only for a 'topic' boost (migration 0113: CASE WHEN trending_source = 'topic' THEN topic_id END).
		trending_topic_id: source === "topic" && row.topicHex !== null ? dashed(row.topicHex) : null,
		upvotes: row.counts[0],
	}
}

/**
 * `entities_last_listed_on`, `entities_listed_on_agenda_id` and `entities_trending_topic_listed_on` of one entity, from the
 * trending twin. Each SQL function is `SELECT <col> FROM trending.entity_listings(ARRAY[e.id])` (or `entity_topic_listings`): no
 * row gives NULL.
 */
export function listingColumns(trending: Trending, entityId: string): ListingColumns {
	const listing = trending.entityListings([entityId]).find((r) => r.entity_id === entityId)
	const topic = trending.entityTopicListings([entityId]).find((r) => r.entity_id === entityId)
	return {
		last_listed_on: listing?.last_listed_on ?? null,
		listed_on_agenda_id: listing?.agenda_id ?? null,
		trending_topic_listed_on: topic?.listed_on ?? null,
	}
}

/** Scores are f64 here and `numeric` in Postgres; its text round trip may differ in the last digits. */
export const TOLERANCE = 1e-9

/** Whether a Postgres column value (text from a `::text` cast, or null) and a twin value agree. Pure. */
export function sameColumn(column: ScoreColumn | ListingColumn, pg: string | null, twin: ColumnValue): boolean {
	if (pg === null || twin === null) return pg === null && twin === null
	if (typeof twin === "string") return pg === twin
	const a = Number(pg)
	return Number.isFinite(a) && Math.abs(a - twin) <= TOLERANCE * Math.max(1, Math.abs(a))
}
