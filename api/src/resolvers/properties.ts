import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq} from "drizzle-orm"
import {Effect} from "effect"
import type {QueryTypesArgs} from "../generated/graphql"
import {relations} from "../services/storage/schema"
import {Storage} from "../services/storage/storage"

export function property(propertyId: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findFirst({
				where: (relations, {eq, and}) =>
					and(eq(relations.fromEntityId, propertyId), eq(relations.typeId, SystemIds.VALUE_TYPE_PROPERTY)),
			})

			if (!result) {
				return {
					id: propertyId,
					valueType: "TEXT",
				}
			}

			return {
				id: propertyId,
				valueType: getValueTypeAsText(result.toEntityId),
			}
		})
	})
}

export function properties(typeId: string, args: QueryTypesArgs) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const where = [eq(relations.fromEntityId, typeId), eq(relations.typeId, SystemIds.PROPERTIES)]

		if (args.filter) {
			const space = args.filter.spaceId

			if (space) {
				where.push(eq(relations.spaceId, space))
			}
		}

		const propertyRelations = yield* db.use(async (client) => {
			return await client.query.relations.findMany({
				where: and(...where),
				with: {
					toEntity: {
						with: {
							fromRelations: {
								where: eq(relations.typeId, SystemIds.VALUE_TYPE_PROPERTY),
							},
						},
					},
				},
			})
		})

		return propertyRelations.map((r) => {
			const maybeValueType = r.toEntity.fromRelations.find(
				(relation) => relation.typeId === SystemIds.VALUE_TYPE_PROPERTY,
			)?.toEntityId

			return {
				id: r.toEntity.id,
				valueType: getValueTypeAsText(maybeValueType),
			}
		})
	})
}

function getValueTypeAsText(valueTypeId: string | undefined): string {
	if (!valueTypeId) {
		return "TEXT"
	}

	switch (valueTypeId) {
		case SystemIds.TEXT:
			return "TEXT"
		case SystemIds.NUMBER:
			return "NUMBER"
		case SystemIds.CHECKBOX:
			return "CHECKBOX"
		case SystemIds.TIME:
			return "TIME"
		case SystemIds.URL:
			return "URL"
		case SystemIds.POINT:
			return "POINT"
		case SystemIds.IMAGE:
			return "IMAGE"
		case SystemIds.RELATION:
			return "RELATION"
		default:
			return "TEXT"
	}
}
