/**
 * Adds `votedBy` / `votedByKinds` arguments to the entities connection, so a
 * person's positions can be fetched as an ordinary entity feed.
 *
 * Without this, "the entities this user voted on" takes two round trips —
 * `userVotesConnection` for the ids, then `entitiesConnection(filter: {id: {in:
 * ...}})` for the cards — and neither half can do the other's job. Sorting by
 * Best or Top needs the entity's score, which `userVotes` does not have;
 * filtering by type needs the entity's types, which `UserVoteFilter` does not
 * have (`objectType` there is the vote's own object kind, not a graph type).
 * Doing it client-side only filters the pages already fetched, so the list would
 * change as you scroll. See GEO-2913.
 *
 * With the argument, a person's positions go through `fetchExploreFeed` like any
 * other feed and pick up Best / New / Top, the space filter and the type filter
 * at once.
 *
 * Usage:
 *   entities(votedBy: "uuid", first: 100) { ... }                 # any vote kind
 *   entities(votedBy: "uuid", votedByKinds: [1, 2], first: 100)   # stance + veracity
 *
 * **Vote kinds matter.** 0 is curation, 1 is stance, 2 is veracity, and it is
 * stance and veracity together that mean "a position on a claim". Counting the
 * table unfiltered overstates a person's positions roughly threefold — 567 rows
 * against a true 192 on the reference account. `votedByKinds` is therefore
 * offered, but deliberately not defaulted: this plugin does not decide what a
 * caller means by a position, it just stops them having to post-filter.
 *
 * Shape of the SQL — a plain semi-join over one table:
 *
 *   WHERE e.id IN (SELECT uv.object_id FROM user_votes uv
 *                  WHERE uv.user_id = $1 AND uv.vote_kind = ANY($2))
 *
 * Deliberately NOT `EXISTS (...) OR EXISTS (...)`: that pair cannot be planned
 * as a semi-join and cost 22.5 s per request on a sparse space in GEO-2882. One
 * table and one IN keeps it a semi-join.
 *
 * Index: `user_votes` is keyed on
 * (user_id, object_id, object_type, space_id, vote_kind) — the primary key added
 * by migration 0086 for GEO-2916 — so `user_id = $1` is served by that index's
 * leading column, and `vote_kind` is in the index too. No new index is required.
 *
 * `object_type` is not constrained. Every row measured carries 0, and the
 * semi-join against `entities.id` already restricts the result to real entities,
 * so filtering on it would only add a predicate that can never change the answer.
 */

// Helper to build the semi-join for "voted on by this user", optionally
// narrowed to a set of vote kinds.
const buildVotedByCondition = (sql: any, tableAlias: any, userId: string, kinds?: number[]) => {
	const kindClause =
		kinds && kinds.length > 0
			? sql.fragment`AND uv.vote_kind = ANY(${sql.value(kinds)}::smallint[])`
			: sql.fragment``

	return sql.fragment`${tableAlias}.id IN (
		SELECT uv.object_id
		FROM user_votes uv
		WHERE uv.user_id = ${sql.value(userId)}::uuid
		${kindClause}
	)`
}

export const EntityVotedByFilterPlugin = (builder: any) => {
	builder.hook("GraphQLObjectType:fields:field:args", (args: any, build: any, context: any) => {
		const {
			scope: {isPgFieldConnection, isPgFieldSimpleCollection, pgFieldIntrospection},
			addArgDataGenerator,
		} = context

		if (!isPgFieldConnection && !isPgFieldSimpleCollection) {
			return args
		}

		if (pgFieldIntrospection?.name !== "entities") {
			return args
		}

		const {pgSql: sql} = build
		const UUIDType = build.getTypeByName("UUID")
		const GraphQLInt = build.getTypeByName("Int")
		const {GraphQLList, GraphQLNonNull} = build.graphql

		// `votedByKinds` is read inside the `votedBy` generator rather than
		// getting its own: on its own it would mean "voted on by anybody with one
		// of these kinds", which is a different and much more expensive question
		// than the one this argument exists to answer. Requiring `votedBy` keeps
		// the semi-join anchored on the indexed user_id column.
		addArgDataGenerator(({votedBy, votedByKinds}: {votedBy?: string; votedByKinds?: number[]}) => {
			if (!votedBy) return {}
			return {
				pgQuery: (queryBuilder: any) => {
					queryBuilder.where(buildVotedByCondition(sql, queryBuilder.getTableAlias(), votedBy, votedByKinds))
				},
			}
		})

		return build.extend(args, {
			votedBy: {
				description:
					"Only entities this user has voted on. Pair with votedByKinds to select which kinds of vote count.",
				type: UUIDType,
			},
			votedByKinds: {
				description:
					"Vote kinds to count when votedBy is set: 0 curation, 1 stance, 2 veracity. Omit for any kind. Ignored without votedBy.",
				type: new GraphQLList(new GraphQLNonNull(GraphQLInt)),
			},
		})
	})
}

export default EntityVotedByFilterPlugin
