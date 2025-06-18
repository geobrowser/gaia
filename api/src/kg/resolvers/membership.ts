import {Effect} from "effect"
import type {QueryMembersArgs} from "~/src/generated/graphql"
import {Storage} from "~/src/services/storage/storage"

export function getMembers(args: QueryMembersArgs) {
	const {spaceId, limit, offset} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.members.findMany({
				where: (members, {eq}) => {
					if (spaceId) {
						return eq(members.spaceId, spaceId)
					}
				},
				limit: limit ?? 100,
				offset: offset ?? 0,
			})

			return result.map((member) => ({
				id: `${member.address}:${member.spaceId}`,
				address: member.address,
				spaceId: member.spaceId,
			}))
		})
	})
}

export function getEditors(args: QueryMembersArgs) {
	const {spaceId, limit, offset} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.members.findMany({
				where: (members, {eq}) => {
					if (spaceId) {
						return eq(members.spaceId, spaceId)
					}
				},
				limit: limit ?? 100,
				offset: offset ?? 0,
			})

			return result.map((member) => ({
				id: `${member.address}:${member.spaceId}`,
				address: member.address,
				spaceId: member.spaceId,
			}))
		})
	})
}
