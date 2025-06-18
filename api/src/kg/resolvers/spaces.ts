import {SystemIds} from "@graphprotocol/grc-20"
import {inArray} from "drizzle-orm"
import {Effect} from "effect"
import {type QuerySpacesArgs, SpaceType} from "~/src/generated/graphql"
import {editors, members} from "../../services/storage/schema"
import {Storage} from "../../services/storage/storage"

export const getSpaces = (args: QuerySpacesArgs) => {
	const {filter, limit, offset} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const spacesResult = await client.query.spaces.findMany({
				where: (spaces, {inArray, sql, eq, exists, and}) => {
					const conditions = []

					// ID filter
					if (filter?.id?.in !== undefined && filter?.id.in !== null) {
						if (filter.id.in.length === 0) {
							// Return condition that matches nothing for empty arrays
							return sql`false`
						}
						conditions.push(inArray(spaces.id, filter.id.in))
					}

					// Member filter
					if (filter?.member) {
						if (filter.member.is) {
							conditions.push(
								exists(
									client
										.select()
										.from(members)
										.where(
											and(
												eq(members.spaceId, spaces.id),
												sql`LOWER(${members.address}) = LOWER(${filter.member.is})`,
											),
										),
								),
							)
						}
						if (filter.member.in !== undefined && filter.member.in !== null) {
							if (filter.member.in.length === 0) {
								// Return condition that matches nothing for empty arrays
								return sql`false`
							}
							const lowerAddresses = filter.member.in.map((addr) => addr.toLowerCase())
							conditions.push(
								exists(
									client
										.select()
										.from(members)
										.where(
											and(
												eq(members.spaceId, spaces.id),
												inArray(sql`LOWER(${members.address})`, lowerAddresses),
											),
										),
								),
							)
						}
					}

					// Editor filter
					if (filter?.editor) {
						if (filter.editor.is) {
							conditions.push(
								exists(
									client
										.select()
										.from(editors)
										.where(
											and(
												eq(editors.spaceId, spaces.id),
												sql`LOWER(${editors.address}) = LOWER(${filter.editor.is})`,
											),
										),
								),
							)
						}
						if (filter.editor.in !== undefined && filter.editor.in !== null) {
							if (filter.editor.in.length === 0) {
								// Return condition that matches nothing for empty arrays
								return sql`false`
							}
							const lowerAddresses = filter.editor.in.map((addr) => addr.toLowerCase())
							conditions.push(
								exists(
									client
										.select()
										.from(editors)
										.where(
											and(
												eq(editors.spaceId, spaces.id),
												inArray(sql`LOWER(${editors.address})`, lowerAddresses),
											),
										),
								),
							)
						}
					}

					// Combine all conditions with AND
					if (conditions.length === 0) {
						return undefined // No filters, return all
					} else if (conditions.length === 1) {
						return conditions[0]
					} else {
						return and(...conditions)
					}
				},
				limit: limit ?? 100,
				offset: offset ?? 0,
			})

			return spacesResult.map((space) => ({
				id: space.id,
				type: space.type === "Personal" ? SpaceType.Personal : SpaceType.Public,
				daoAddress: space.daoAddress,
				spaceAddress: space.spaceAddress,
				mainVotingAddress: space.mainVotingAddress,
				membershipAddress: space.membershipAddress,
				personalAddress: space.personalAddress,
			}))
		})
	})
}

export const getSpace = (id: string) =>
	Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const space = await client.query.spaces.findFirst({
				where: (spaces, {eq}) => eq(spaces.id, id),
			})

			if (!space) {
				return null
			}

			return {
				id: space.id,
				type: space.type === "Personal" ? SpaceType.Personal : SpaceType.Public,
				daoAddress: space.daoAddress,
				spaceAddress: space.spaceAddress,
				mainVotingAddress: space.mainVotingAddress,
				membershipAddress: space.membershipAddress,
				personalAddress: space.personalAddress,
			}
		})
	})

export const getSpaceEntity = (spaceId: string) =>
	Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const spaceEntity = await client.query.relations.findFirst({
				where: (relations, {eq, and}) =>
					and(
						eq(relations.spaceId, spaceId),
						eq(relations.typeId, SystemIds.TYPES_PROPERTY),
						eq(relations.toEntityId, SystemIds.SPACE_TYPE),
					),
				with: {
					fromEntity: true,
				},
			})

			if (!spaceEntity) {
				return null
			}

			return spaceEntity.fromEntity
		})
	})
