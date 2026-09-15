import {gql, makeExtendSchemaPlugin} from "graphile-utils"

/** The subset of postgraphile's query builder this plugin drives. */
type SqlBuilder = {
	where: (fragment: unknown) => void
	limit: (count: number) => void
}

/** The `graphile` bag postgraphile attaches to resolveInfo for table lookups. */
type GraphileResolveInfo = {
	graphile: {
		selectGraphQLResultFromTable: (
			from: unknown,
			builder: (tableAlias: unknown, sqlBuilder: SqlBuilder) => void,
		) => Promise<unknown[]>
	}
}

type LegacyAccessorArgs = {
	userId: string
	objectId: string
	objectType: number
	spaceId: string
}

type PerKindAccessorArgs = LegacyAccessorArgs & {
	voteKind: number
}

/**
 * Restores `userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId`, the unique-key
 * accessor that migration 0071 renamed.
 *
 * Widening `user_votes`' unique constraint to include `vote_kind` makes
 * postgraphile regenerate that field as
 * `userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind`, and the old
 * name simply disappears. The shipped web client queries the old name
 * (`core/io/query-fragments.tsx`, `userEntityVoteQuery`), so without this the
 * query fails GraphQL validation the moment the migration deploys and a user's
 * own vote state stops resolving on every entity — the same failure mode the
 * `upvotes`/`downvotes` column shims exist to prevent, and the reason the two
 * are handled together.
 *
 * This resolves the CURATION row specifically, which is exactly what the old
 * field meant when curation was the only kind. It is deliberately not
 * kind-aware: callers that want stance or veracity should use the per-kind
 * accessor, which this plugin now also provides — see below.
 *
 * **The per-kind accessor is restored here too, for the same reason one step on.**
 * Migration 0086 (GEO-2916) turned `user_votes`' unique constraint into the
 * primary key, so that cursor pagination works at all — postgraphile refuses
 * before/after cursors on an ordering it cannot prove unique, and it only
 * appends PRIMARY KEY columns to make one. But once those columns are the
 * primary key, postgraphile emits their accessor as plain `userVote` and
 * `userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind` disappears —
 * the very field this file's deprecation notice tells callers to move to. That
 * is the same break 0071 caused, so it is patched the same way rather than
 * shipped as a silent API rename.
 *
 * @deprecated Remove together with the `upvotes`/`downvotes` shims in the
 * migration that ships the client reading per-kind values.
 */
export const UserVoteLegacyAccessorPlugin = makeExtendSchemaPlugin((build) => {
	const {pgSql: sql} = build

	return {
		typeDefs: gql`
			extend type Query {
				"""
				Deprecated: the curation-kind row only. Use
				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind.
				"""
				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId(
					userId: UUID!
					objectId: UUID!
					objectType: Int!
					spaceId: UUID!
				): UserVote

				"""
				The per-kind row. Restored by hand because migration 0086 turned the
				unique constraint into the primary key, which makes postgraphile emit
				this accessor as plain "userVote" instead.
				"""
				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind(
					userId: UUID!
					objectId: UUID!
					objectType: Int!
					spaceId: UUID!
					voteKind: Int!
				): UserVote
			}
		`,
		resolvers: {
			Query: {
				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId: async (
					_parent: unknown,
					args: LegacyAccessorArgs,
					_context: unknown,
					resolveInfo: GraphileResolveInfo,
				) => {
					// Going through selectGraphQLResultFromTable (rather than a raw
					// query) keeps the row shaped the way postgraphile's own UserVote
					// field resolvers expect, so every selectable field works, not
					// just the `voteType` the current client asks for.
					//
					// UUIDs arrive normalized to undashed hex; Postgres accepts that
					// form for a uuid cast.
					const rows = await resolveInfo.graphile.selectGraphQLResultFromTable(
						sql.fragment`public.user_votes`,
						(tableAlias, sqlBuilder) => {
							sqlBuilder.where(sql.fragment`${tableAlias}.user_id = ${sql.value(args.userId)}::uuid`)
							sqlBuilder.where(sql.fragment`${tableAlias}.object_id = ${sql.value(args.objectId)}::uuid`)
							sqlBuilder.where(
								sql.fragment`${tableAlias}.object_type = ${sql.value(args.objectType)}::smallint`,
							)
							sqlBuilder.where(sql.fragment`${tableAlias}.space_id = ${sql.value(args.spaceId)}::uuid`)
							sqlBuilder.where(sql.fragment`${tableAlias}.vote_kind = 0`)
							sqlBuilder.limit(1)
						},
					)
					return rows[0] ?? null
				},

				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind: async (
					_parent: unknown,
					args: PerKindAccessorArgs,
					_context: unknown,
					resolveInfo: GraphileResolveInfo,
				) => {
					const rows = await resolveInfo.graphile.selectGraphQLResultFromTable(
						sql.fragment`public.user_votes`,
						(tableAlias, sqlBuilder) => {
							sqlBuilder.where(sql.fragment`${tableAlias}.user_id = ${sql.value(args.userId)}::uuid`)
							sqlBuilder.where(sql.fragment`${tableAlias}.object_id = ${sql.value(args.objectId)}::uuid`)
							sqlBuilder.where(
								sql.fragment`${tableAlias}.object_type = ${sql.value(args.objectType)}::smallint`,
							)
							sqlBuilder.where(sql.fragment`${tableAlias}.space_id = ${sql.value(args.spaceId)}::uuid`)
							sqlBuilder.where(
								sql.fragment`${tableAlias}.vote_kind = ${sql.value(args.voteKind)}::smallint`,
							)
							sqlBuilder.limit(1)
						},
					)
					return rows[0] ?? null
				},
			},
		},
	}
})

export default UserVoteLegacyAccessorPlugin
