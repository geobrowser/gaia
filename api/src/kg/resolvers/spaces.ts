import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {type QuerySpacesArgs, SpaceType} from "~/src/generated/graphql"
import {Storage} from "../../services/storage/storage"

export const getSpaces = (args: QuerySpacesArgs) => {
	const {filter, limit, offset} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const spacesResult = await client.query.spaces.findMany({
				where: (spaces, {inArray, sql}) => {
					if (filter?.id?.in !== undefined && filter?.id.in !== null) {
						if (filter.id.in.length === 0) {
							// Return condition that matches nothing for empty arrays
							return sql`false`
						}
						return inArray(spaces.id, filter.id.in)
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
