import {makeAddPgTableOrderByPlugin, orderByAscDesc} from "graphile-utils"

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
 * `nulls: "last"` matters: only ~1.2M of 48.9M entities are expected to have a score
 * until the backfill completes, and unscored entities must sort to the bottom rather
 * than the top. Postgres puts NULLs first on DESC by default, which would surface
 * precisely the unscored entities the feed is meant to rank.
 */
export const EntityOrderByRankingScorePlugin = makeAddPgTableOrderByPlugin(
	"public",
	"entities",
	(build) => {
		const {pgSql: sql} = build

		const rankingScore = orderByAscDesc(
			"RANKING_SCORE",
			({queryBuilder}) => {
				const t = queryBuilder.getTableAlias()
				return sql.fragment`(
					SELECT rs.ranking_score FROM public.entity_ranking_scores rs
					WHERE rs.entity_id = ${t}.id
				)`
			},
			{unique: false, nulls: "last"},
		)

		const qualityScore = orderByAscDesc(
			"QUALITY_SCORE",
			({queryBuilder}) => {
				const t = queryBuilder.getTableAlias()
				return sql.fragment`(
					SELECT rs.quality_score FROM public.entity_ranking_scores rs
					WHERE rs.entity_id = ${t}.id
				)`
			},
			{unique: false, nulls: "last"},
		)

		return {...rankingScore, ...qualityScore}
	},
	"Adding orderBy entity_ranking_scores.ranking_score / .quality_score to the entities connection",
)

export default EntityOrderByRankingScorePlugin
