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

		const rawScore = orderByAscDesc(
			"RAW_SCORE",
			({queryBuilder}) => {
				const t = queryBuilder.getTableAlias()
				return sql.fragment`(
					SELECT (vc.upvotes - vc.downvotes) FROM public.votes_count vc
					WHERE vc.object_id = ${t}.entity_id
						AND vc.space_id = ${t}.space_id
						AND vc.object_type = 0
				)`
			},
			{unique: false, nulls: "last-iff-ascending"},
		)

		return {...localScore, ...globalScore, ...rawScore}
	},
	"Adding orderBy local_scores.score, global_scores.score, and raw vote score to values connection",
)

export default ValueOrderByScorePlugin
