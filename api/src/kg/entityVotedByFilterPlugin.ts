/**
 * Filters the entities connection by one user's votes, in two forms:
 *
 * 1. `votedBy` / `votedByKinds` / `votedByTypes` arguments on `entities` /
 *    `entitiesConnection` — "only what this user voted on" (GEO-2913, GEO-2962).
 * 2. A `votedBy` field on `EntityFilter` — the same predicate, but usable inside
 *    `filter`, so it can be negated with `not` and nested wherever an
 *    `EntityFilter` is accepted (GEO-2894).
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
 * Usage:
 *   entities(votedBy: "uuid", first: 100) { ... }                 # any vote kind
 *   entities(votedBy: "uuid", votedByKinds: [1, 2], first: 100)   # stance + veracity
 *   entities(votedBy: "uuid", votedByKinds: [1, 2], votedByTypes: [0, 1])  # positions still held
 *
 *   # Everything this user has NOT taken a position on (GEO-2894):
 *   entitiesConnection(filter: {not: {votedBy: {userId: "uuid", kinds: [1, 2], types: [0, 1]}}})
 *
 * **Vote kinds matter.** 0 is curation, 1 is stance, 2 is veracity, and it is
 * stance and veracity together that mean "a position on a claim". Counting the
 * table unfiltered overstates a person's positions roughly threefold — 567 rows
 * against a true 192 on the reference account. `votedByKinds` is therefore
 * offered, but deliberately not defaulted: this plugin does not decide what a
 * caller means by a position, it just stops them having to post-filter.
 *
 * **Vote types matter too.** 0 is agree/verify, 1 disagree/dispute, 2 neither.
 * `user_votes` is unique per (user, object, object_type, space, kind), so taking
 * a position back does not delete the row — it rewrites it to vote_type 2. A
 * `votedBy` read without `votedByTypes: [0, 1]` therefore counts claims the
 * person no longer holds a position on (50 of 1,731 across the 20 accounts
 * measured for GEO-2962; 12 of 34 on the worst one). Not defaulted either, so
 * existing callers keep today's answer.
 *
 * Shape of the SQL — a plain semi-join over one table:
 *
 *   WHERE e.id IN (SELECT uv.object_id FROM user_votes uv
 *                  WHERE uv.user_id = $1 AND uv.vote_kind = ANY($2)
 *                    AND uv.vote_type = ANY($3))
 *
 * Deliberately NOT `EXISTS (...) OR EXISTS (...)`: that pair cannot be planned
 * as a semi-join and cost 22.5 s per request on a sparse space in GEO-2882. One
 * table and one IN keeps it a semi-join.
 *
 * The filter field uses a correlated `EXISTS` instead, because it is mostly used
 * negated and `NOT EXISTS` becomes an anti-join where `NOT IN` cannot. See
 * buildVotedByExistsCondition for the measurement.
 *
 * Index: `user_votes` is keyed on
 * (user_id, object_id, object_type, space_id, vote_kind) — the primary key added
 * by migration 0086 for GEO-2916 — so `user_id = $1` is served by that index's
 * leading column, and `vote_kind` is in the index too. `vote_type` is not, so
 * `votedByTypes` turns the index-only scan into an index scan with a heap filter
 * over that one user's rows — hundreds, not the table. No new index is required.
 *
 * `object_type` is not constrained. Every row measured carries 0, and the
 * semi-join against `entities.id` already restricts the result to real entities,
 * so filtering on it would only add a predicate that can never change the answer.
 */

export type VotedBySelection = {
	userId: string
	kinds?: number[] | null
	types?: number[] | null
}

// The kind / type narrowing shared by both forms. Empty arrays mean "any".
const buildVoteClauses = (sql: any, {kinds, types}: VotedBySelection) => {
	const kindClause =
		kinds && kinds.length > 0
			? sql.fragment`AND uv.vote_kind = ANY(${sql.value(kinds)}::smallint[])`
			: sql.fragment``
	const typeClause =
		types && types.length > 0
			? sql.fragment`AND uv.vote_type = ANY(${sql.value(types)}::smallint[])`
			: sql.fragment``
	return sql.fragment`${kindClause} ${typeClause}`
}

// Argument form: an uncorrelated IN, so the vote set drives the scan. For
// "this person's positions" that is the right way round — a few hundred votes
// probing entities_pkey, not the entity table probing votes.
const buildVotedByInCondition = (sql: any, tableAlias: any, selection: VotedBySelection) =>
	sql.fragment`${tableAlias}.id IN (
		SELECT uv.object_id
		FROM user_votes uv
		WHERE uv.user_id = ${sql.value(selection.userId)}::uuid
		${buildVoteClauses(sql, selection)}
	)`

// Filter form: a correlated EXISTS on the (user_id, object_id) prefix of
// user_votes_pkey. Written this way for the negated case, which is the one the
// filter exists for: connection-filter renders `not` as `NOT (<fragment>)`, and
// Postgres turns `NOT EXISTS` into an anti-join, which leaves the rest of the
// query free to choose the driving set (the tag's claims) and probe the index
// once per candidate. `NOT (id IN (...))` cannot become an anti-join: it stays a
// hashed SubPlan filter on the entities scan, and the planner then seq-scans
// entities to apply it. Measured on a seeded 3M-entity database, Explore's
// tagged-claims page for a 5,094-vote user: 131 ms with NOT IN, 22 ms with NOT
// EXISTS, against 16 ms with no exclusion at all.
const buildVotedByExistsCondition = (sql: any, tableAlias: any, selection: VotedBySelection) =>
	sql.fragment`EXISTS (
		SELECT 1
		FROM user_votes uv
		WHERE uv.user_id = ${sql.value(selection.userId)}::uuid
		AND uv.object_id = ${tableAlias}.id
		${buildVoteClauses(sql, selection)}
	)`

const VOTED_BY_FILTER_TYPE = "EntityVotedByFilter"

export const EntityVotedByFilterPlugin = (builder: any) => {
	// The input type for EntityFilter.votedBy.
	builder.hook("init", (_: any, build: any) => {
		const {
			newWithHooks,
			graphql: {GraphQLInputObjectType, GraphQLList, GraphQLNonNull, GraphQLInt},
		} = build
		newWithHooks(
			GraphQLInputObjectType,
			{
				name: VOTED_BY_FILTER_TYPE,
				description:
					"Matches entities one user has voted on. Negate with `not` for the entities they have not. " +
					"Vote kinds: 0 curation, 1 stance, 2 veracity. Vote types: 0 agree/verify, 1 disagree/dispute, " +
					"2 neither (a retracted position).",
				fields: () => ({
					userId: {
						description: "The voter (their personal space id).",
						type: new GraphQLNonNull(build.getTypeByName("UUID")),
					},
					kinds: {
						description: "Vote kinds to count. Omit or pass [] for any kind.",
						type: new GraphQLList(new GraphQLNonNull(GraphQLInt)),
					},
					types: {
						description:
							"Vote types to count. Omit or pass [] for any type; [0, 1] leaves out retracted positions.",
						type: new GraphQLList(new GraphQLNonNull(GraphQLInt)),
					},
				}),
			},
			{isEntityVotedByFilter: true},
		)
		return _
	})

	// EntityFilter.votedBy
	builder.hook("GraphQLInputObjectType:fields", (fields: any, build: any, context: any) => {
		const {
			scope: {isPgConnectionFilter, pgIntrospection: table},
			fieldWithHooks,
			Self,
		} = context
		if (!isPgConnectionFilter || table?.kind !== "class" || table.name !== "entities") {
			return fields
		}

		const {pgSql: sql, connectionFilterRegisterResolver} = build
		const VotedByFilterType = build.getTypeByName(VOTED_BY_FILTER_TYPE)
		if (!VotedByFilterType || !connectionFilterRegisterResolver) return fields

		connectionFilterRegisterResolver(
			Self.name,
			"votedBy",
			({sourceAlias, fieldValue}: {sourceAlias: any; fieldValue: VotedBySelection | null}) => {
				if (fieldValue == null) return null
				return buildVotedByExistsCondition(sql, sourceAlias, fieldValue)
			},
		)

		return build.extend(
			fields,
			{
				votedBy: fieldWithHooks(
					"votedBy",
					{
						description:
							"Entities this user has voted on. Use inside `not` for the entities they have not voted on.",
						type: VotedByFilterType,
					},
					{isPgConnectionFilterField: true},
				),
			},
			"Adding votedBy to EntityFilter (GEO-2894)",
		)
	})

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

		// `votedByKinds` / `votedByTypes` are read inside the `votedBy` generator
		// rather than getting their own: on their own they would mean "voted on by
		// anybody with one of these kinds", which is a different and much more
		// expensive question than the one this argument exists to answer.
		// Requiring `votedBy` keeps the semi-join anchored on the indexed user_id
		// column.
		addArgDataGenerator(
			({
				votedBy,
				votedByKinds,
				votedByTypes,
			}: {
				votedBy?: string
				votedByKinds?: number[]
				votedByTypes?: number[]
			}) => {
				if (!votedBy) return {}
				return {
					pgQuery: (queryBuilder: any) => {
						queryBuilder.where(
							buildVotedByInCondition(sql, queryBuilder.getTableAlias(), {
								userId: votedBy,
								kinds: votedByKinds,
								types: votedByTypes,
							}),
						)
					},
				}
			},
		)

		return build.extend(args, {
			votedBy: {
				description:
					"Only entities this user has voted on. Pair with votedByKinds / votedByTypes to select which votes count.",
				type: UUIDType,
			},
			votedByKinds: {
				description:
					"Vote kinds to count when votedBy is set: 0 curation, 1 stance, 2 veracity. Omit for any kind. Ignored without votedBy.",
				type: new GraphQLList(new GraphQLNonNull(GraphQLInt)),
			},
			votedByTypes: {
				description:
					"Vote types to count when votedBy is set: 0 agree/verify, 1 disagree/dispute, 2 neither (a retracted position). " +
					"Pass [0, 1] for positions still held. Omit for any type. Ignored without votedBy.",
				type: new GraphQLList(new GraphQLNonNull(GraphQLInt)),
			},
		})
	})
}

export default EntityVotedByFilterPlugin
