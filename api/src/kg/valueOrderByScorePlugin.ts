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

		return {...localScore, ...globalScore}
	},
	"Adding orderBy local_scores.score and global_scores.score to values connection",
)

export default ValueOrderByScorePlugin
