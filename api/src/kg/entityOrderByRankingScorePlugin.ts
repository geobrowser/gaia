import type {Build} from "graphile-build"
import {makeAddPgTableOrderByPlugin} from "graphile-utils"
import type {MakeAddPgTableOrderByPluginOrders} from "graphile-utils/node8plus/makeAddPgTableOrderByPlugin"

/**
 * Adds RANKING_SCORE / QUALITY_SCORE ordering to the entities connection, for the
 * Explore feed "Best" sort (Phase A).
 *
 * The feed's own entry point is `entities_ranked_for_feed(...)`, which also applies
 * the blocklist and excluded-type filters that must never be skipped. This plugin
 * exists for the cases that want the ordering without candidate generation — e.g.
 * ranking a set already narrowed by another filter. Prefer the function for the feed
 * itself, or the exclusions become every caller's responsibility.
 *
 * WHY A SENTINEL INSTEAD OF `nulls: "last"` (GEO-2795)
 *
 * This used to pass `{nulls: "last"}`, which silently lost every unscored row as soon
 * as a cursor was involved. `nulls` only affects how PostGraphile renders ORDER BY; it
 * is NOT mirrored into the cursor's keyset predicate, which is built in
 * graphile-build-pg's `setCursorComparator` as, per order-by level:
 *
 *   (expr > $cursor) OR (expr = $cursor AND <next level>)
 *
 * There is no null handling in that construction at all. For a row whose expression is
 * NULL, both `NULL > $cursor` and `NULL = $cursor` evaluate to NULL — never true — so
 * the row cannot satisfy the bound on any page after the first. It is not merely
 * mis-ordered, it is unreachable, and the connection then reports `hasNextPage: false`
 * because it genuinely has no remaining rows that can match.
 *
 * Measured on testnet before this change: paging Claim entities tagged `Debate` under
 * RANKING_SCORE_DESC returned 297 of 353 rows. All 59 missing rows had no score; none
 * of the scored rows were lost. `ID_DESC` on the identical filter paged exactly.
 *
 * The NULL is not a null column — `ranking_score` has no nulls in 48.9M rows. It is the
 * correlated subquery returning no row for the ~700k entities absent from
 * `entity_ranking_scores`. So substituting a sentinel removes the NULL at the source and
 * the generic comparator becomes correct for those rows without special-casing.
 *
 * WHY A DIFFERENT SENTINEL PER DIRECTION
 *
 * `nulls: "last"` meant last in *both* directions. A single sentinel cannot preserve
 * that: -1 sorts unscored last under DESC but first under ASC. So the two directions are
 * built by hand with the sentinel that keeps unscored rows at the bottom of each. That
 * keeps ASC's public behaviour identical to before rather than quietly inverting it.
 *
 * Both sentinels are finite and far outside the observed range (min 17679.2,
 * max 17888.4, no negatives). Finite matters: the sentinel is what lands in the cursor,
 * cursors are JSON, and `Infinity` is not JSON-representable — it would round-trip as
 * `null` and reintroduce the bug it is meant to remove.
 */

/** Below every real score, so unscored rows sort last under DESC. */
const UNSCORED_SENTINEL_DESC = "-1"

/** Above every real score, so unscored rows sort last under ASC. */
const UNSCORED_SENTINEL_ASC = "1e12"

/**
 * One ASC/DESC pair for a column of `entity_ranking_scores`, mirroring the shape
 * `orderByAscDesc` produces but with a per-direction sentinel. `unique: false` is
 * required and load-bearing: PostGraphile appends the primary key to make the order
 * unique, and `setCursorComparator` throws outright if the order is not unique, so the
 * entity id is what disambiguates tied scores.
 */
function scoreOrderBy(
	baseName: string,
	column: "ranking_score" | "quality_score",
	build: Build,
): MakeAddPgTableOrderByPluginOrders {
	const {pgSql: sql} = build

	const scoreOrSentinel =
		(sentinel: string) =>
		({queryBuilder}: {queryBuilder: {getTableAlias: () => unknown}}) => {
			const t = queryBuilder.getTableAlias()
			// `::float8`, not `::numeric`, and this is the whole of the tie fix (GEO-2795).
			//
			// A cursor is base64 JSON, and PostGraphile puts the order-by value in it as a JSON
			// number — a double. Decoding a real one shows exactly that:
			//
			//   ["RANKING_SCORE_DESC", [17885.66401799888, "93227664-fed5-4f48-9e46-22bd7293d665"]]
			//
			// `ranking_score` is unconstrained `numeric` and the stored value there is
			// 17885.6640179988796355000000, so the cursor is a rounded copy. The continuation
			// predicate is `(expr > $c) OR (expr = $c AND <next>)`, and against the exact numeric:
			//
			//   numeric = 17885.66401799888  -> false, so the id tiebreak can NEVER fire
			//   numeric < 17885.66401799888  -> true,  so the boundary row repeats on the next page
			//
			// Which is why both symptoms appear and why adding ID_DESC changed nothing: the id was
			// always in the cursor, but the score comparison fails before it is ever consulted. A
			// value that rounds *down* duplicates, one that rounds *up* is dropped. 3 rows share
			// that boundary score, which is exactly the 3 duplicates in the report.
			//
			// Ordering by the double makes the comparison exact, because the column and the cursor
			// then hold the same value. Precision is not a concern for ranking: scores span
			// 17679-17888 and float8 keeps ~11 decimal places at that magnitude, while real score
			// differences are in the 4th. Any genuine tie is broken by the primary key, which
			// PostGraphile appends and which now actually gets reached.
			return sql.fragment`COALESCE((
				SELECT rs.${sql.identifier(column)} FROM public.entity_ranking_scores rs
				WHERE rs.entity_id = ${t}.id
			), ${sql.literal(sentinel)})::float8`
		}

	return {
		[`${baseName}_ASC`]: {
			value: {
				alias: `${baseName}_ASC`,
				specs: [[scoreOrSentinel(UNSCORED_SENTINEL_ASC), true]],
				unique: false,
			},
		},
		[`${baseName}_DESC`]: {
			value: {
				alias: `${baseName}_DESC`,
				specs: [[scoreOrSentinel(UNSCORED_SENTINEL_DESC), false]],
				unique: false,
			},
		},
	}
}

export const EntityOrderByRankingScorePlugin = makeAddPgTableOrderByPlugin(
	"public",
	"entities",
	(build) => ({
		...scoreOrderBy("RANKING_SCORE", "ranking_score", build),
		...scoreOrderBy("QUALITY_SCORE", "quality_score", build),
	}),
	"Adding orderBy entity_ranking_scores.ranking_score / .quality_score to the entities connection",
)

export default EntityOrderByRankingScorePlugin
