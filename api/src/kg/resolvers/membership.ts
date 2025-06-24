import {Effect} from "effect"
import {Storage} from "~/src/services/storage/storage"

export function getMembers(args: {spaceId: string}) {
	const {spaceId} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.members.findMany({
				where: (members, {eq}) => {
					if (spaceId) {
						return eq(members.spaceId, spaceId)
					}
				},
			})

			return result.map((member) => ({
				id: `${member.address}:${member.spaceId}`,
				address: member.address,
				spaceId: member.spaceId,
			}))
		})
	})
}

export function getEditors(args: {spaceId: string}) {
	const {spaceId} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.members.findMany({
				where: (members, {eq}) => {
					if (spaceId) {
						return eq(members.spaceId, spaceId)
					}
				},
			})

			return result.map((member) => ({
				id: `${member.address}:${member.spaceId}`,
				address: member.address,
				spaceId: member.spaceId,
			}))
		})
	})
}
