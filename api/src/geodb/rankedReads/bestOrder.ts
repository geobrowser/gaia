/**
 * Twins of the two ordered entity reads: `public.entities_in_best_order(uuid[])` (migration 0104) and
 * `public.entities_ordered_by_score(score_type, space_id, sort_direction)` (0056, last redefined by 0071), over the scores
 * geo-db holds in derived collections 2 (global_scores), 3 (local_scores) and 21 (the entity score overlay, via `DerivedScores`).
 *
 * They return PAGE ORDER, not rows: the point is that the same ids come back in the same positions, ties included.
 *
 * `entitiesInBestOrder`: first position of each requested id (a duplicate does not move it); an id with no `entities` row is
 * omitted; scored entities (a row in entity_ranking_scores) come first by ranking_score DESC, ties by entity id DESC; unscored
 * entities follow in the caller's order. More than 1000 ids is the SQL's 22023 error.
 *
 * `entitiesOrderedByScore`: an inner join of `entities` with the score table, so an entity with no score row is absent, ordered by
 * score (DESC by default) with ties by entity id ASC in BOTH directions. Only `local` and `global` are twinned. `raw`
 * (votes_count) has no geo-db reader, and ASC cannot be scanned from geo-db (a scan is score DESC, key ASC), so a caller that
 * holds rows can still ask for ASC here but the shadow never does.
 *
 * Entity ids compare as lowercase text, which for UUIDs is byte order, the order Postgres sorts `uuid`. Scores are `numeric` in
 * Postgres and f64 here: the golden fixtures keep them to nine decimals, where equal numerics are equal doubles and distinct ones
 * stay distinct; a live score with more digits could tie in Postgres and not here, or the reverse.
 *
 * Pinned to real plpgsql output by `__tests__/bestOrder.golden.test.ts` (`api/scripts/bestorder/gen_golden.py`). SQL stays the
 * source of truth; nothing here serves a request.
 */

export const BEST_ORDER_MAX_IDS = 1000

/** The SQL's `RAISE EXCEPTION ... USING ERRCODE = '22023'`. */
export class InvalidParameter extends Error {
	readonly code = "22023"
}

const key = (id: string) => id.replace(/-/g, "").toLowerCase()
const byIdAsc = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0)

/**
 * `entities_in_best_order(requested)`. `existing` is the set of ids that have an entities row and `ranking` the ranking_score of
 * those with an entity_ranking_scores row (keyed by id, any case or dashing). Returns the dashless lowercase ids in order.
 */
export function entitiesInBestOrder(
	requested: readonly string[],
	existing: ReadonlySet<string>,
	ranking: ReadonlyMap<string, number>,
): string[] {
	if (requested.length > BEST_ORDER_MAX_IDS) {
		throw new InvalidParameter(
			`entitiesInBestOrder accepts at most ${BEST_ORDER_MAX_IDS} ids (got ${requested.length})`,
		)
	}
	const exists = new Set([...existing].map(key))
	const score = new Map([...ranking].map(([id, s]) => [key(id), s]))
	const firstPos = new Map<string, number>()
	requested.forEach((id, i) => {
		const k = key(id)
		if (!firstPos.has(k)) firstPos.set(k, i)
	})
	const rows = [...firstPos].filter(([id]) => exists.has(id)).map(([id, pos]) => ({id, pos, score: score.get(id)}))
	const scored = rows.filter((r) => r.score !== undefined) as {id: string; pos: number; score: number}[]
	const unscored = rows.filter((r) => r.score === undefined)
	scored.sort((a, b) => b.score - a.score || byIdAsc(b.id, a.id))
	unscored.sort((a, b) => a.pos - b.pos)
	return [...scored, ...unscored].map((r) => r.id)
}

export type ScoreType = "local" | "global"
export type SortDirection = "ASC" | "DESC"

/**
 * `entities_ordered_by_score` over the rows of the score table (`local_scores` of one space, or `global_scores`). `existing`
 * limits it to ids with an entities row, as the SQL's inner join does; null means every row has one.
 */
export function entitiesOrderedByScore(
	rows: Iterable<{id: string; score: number}>,
	existing: ReadonlySet<string> | null,
	direction: SortDirection = "DESC",
): string[] {
	const exists = existing === null ? null : new Set([...existing].map(key))
	const kept = [...rows]
		.map((r) => ({id: key(r.id), score: r.score}))
		.filter((r) => exists === null || exists.has(r.id))
	const sign = direction === "ASC" ? 1 : -1
	kept.sort((a, b) => sign * (a.score - b.score) || byIdAsc(a.id, b.id))
	return kept.map((r) => r.id)
}

/** The argument checks of `entities_ordered_by_score`: a null score_type, and a local read without a space. */
export function checkOrderedByScore(scoreType: string | null, spaceId: string | null): void {
	if (scoreType === null) throw new InvalidParameter("score_type is required")
	if (scoreType === "local" && spaceId === null)
		throw new InvalidParameter("space_id is required for score_type: local")
}

/** `LIMIT first OFFSET offset` over an order. */
export function pageOf<T>(order: readonly T[], offset: number, first: number): T[] {
	return order.slice(offset, offset + first)
}
