import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq, exists} from "drizzle-orm"
import {Effect} from "effect"
import type {QueryTypesArgs} from "../../generated/graphql"
import {entities, relations} from "../../services/storage/schema"
import {Storage} from "../../services/storage/storage"

export function getTypes(args: QueryTypesArgs) {
	const {limit, offset, spaceId} = args

	const where = [
		eq(relations.fromEntityId, entities.id),
		eq(relations.typeId, SystemIds.TYPES_PROPERTY),
		eq(relations.toEntityId, SystemIds.SCHEMA_TYPE),
	]

	if (spaceId && spaceId !== "") {
		where.push(eq(relations.spaceId, spaceId))
	}

	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const types = await client.query.entities.findMany({
				limit: Number(limit),
				offset: Number(offset),
				where: exists(
					client
						.select({id: relations.id})
						.from(relations)
						.where(and(...where)),
				),
				with: {
					values: {
						columns: {
							propertyId: true,
							value: true,
						},
					},
				},
			})

			return types.map((result) => {
				return {
					id: result.id,
					name: result.values.find((p) => p.propertyId === SystemIds.NAME_PROPERTY)?.value,
					description: result.values.find((p) => p.propertyId === SystemIds.DESCRIPTION_PROPERTY)?.value,
				}
			})
		})
	})
}
