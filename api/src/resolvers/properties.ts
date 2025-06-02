import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq} from "drizzle-orm"
import {Effect} from "effect"
import {type QueryTypesArgs, ValueType} from "../generated/graphql"
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
					valueType: ValueType.Text,
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

		if (args.spaceId) {
			where.push(eq(relations.spaceId, args.spaceId))
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

function getValueTypeAsText(valueTypeId: string | undefined): ValueType {
	if (!valueTypeId) {
		return ValueType.Text
	}

	switch (valueTypeId) {
		case SystemIds.TEXT:
			return ValueType.Text
		case SystemIds.NUMBER:
			return ValueType.Number
		case SystemIds.CHECKBOX:
			return ValueType.Checkbox
		case SystemIds.TIME:
			return ValueType.Time
		case SystemIds.URL:
			return ValueType.Url
		case SystemIds.POINT:
			return ValueType.Point
		case SystemIds.IMAGE:
			return ValueType.Image
		case SystemIds.RELATION:
			return ValueType.Relation
		default:
			return ValueType.Text
	}
}
