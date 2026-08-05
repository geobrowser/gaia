import {makeAddPgTableOrderByPlugin, orderByAscDesc} from "graphile-utils"

export const ValueOrderByScorePlugin = makeAddPgTableOrderByPlugin(
	"public",
	"values",
	(build) => {
		const {pgSql: sql} = build

		const localScore = orderByAscDesc(
			"LOCAL_SCORE",
			({queryBuilder}) => {
				const t = queryBuilder.getTableAlias()
				return sql.fragment`(
					SELECT ls.score FROM public.local_scores ls
					WHERE ls.entity_id = ${t}.entity_id
						AND ls.space_id = ${t}.space_id
				)`
			},
			{unique: false, nulls: "last"},
		)

		const globalScore = orderByAscDesc(
			"GLOBAL_SCORE",
			({queryBuilder}) => {
				const t = queryBuilder.getTableAlias()
				return sql.fragment`(
					SELECT gs.score FROM public.global_scores gs
					WHERE gs.entity_id = ${t}.entity_id
				)`
			},
			{unique: false, nulls: "last"},
		)

		// RAW_SCORE is the curation score. votes_count holds one row per
		// (object, type, space, vote_kind), so without the vote_kind filter this
		// scalar subquery would match up to three rows — erroring at best, and
		// silently returning an arbitrary kind's tally at worst.
		const rawScore = orderByAscDesc(
			"RAW_SCORE",
			({queryBuilder}) => {
				const t = queryBuilder.getTableAlias()
				return sql.fragment`(
					COALESCE(
						(
							SELECT (vc.positive - vc.negative) FROM public.votes_count vc
							WHERE vc.object_id = ${t}.entity_id
								AND vc.space_id = ${t}.space_id
								AND vc.object_type = 0
								AND vc.vote_kind = 0
						),
						0
					)
				)`
			},
			{unique: false, nulls: "last"},
		)

		return {...localScore, ...globalScore, ...rawScore}
	},
	"Adding orderBy local_scores.score, global_scores.score, and raw vote score to values connection",
)

export default ValueOrderByScorePlugin
