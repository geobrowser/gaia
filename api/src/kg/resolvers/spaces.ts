import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {SpaceType} from "~/src/generated/graphql"
import {spaces} from "../../services/storage/schema" // Added relations import
import {Storage} from "../../services/storage/storage"

export const getSpaces = Effect.gen(function* () {
	const db = yield* Storage

	return yield* db.use(async (client) => {
		const spacesResult = await client.select().from(spaces)

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
					and(eq(relations.spaceId, spaceId), eq(relations.typeId, SystemIds.SPACE_TYPE)),
				with: {
					toEntity: true,
				},
			})

			if (!spaceEntity) {
				return null
			}

			return spaceEntity.toEntity
		})
	})
