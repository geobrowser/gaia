import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq} from "drizzle-orm"
import {Effect} from "effect"
import {DataType, type QueryTypesArgs} from "../generated/graphql"
import {properties, relations} from "../services/storage/schema"
import {Storage} from "../services/storage/storage"

export function getProperty(propertyId: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.properties.findFirst({
				where: (properties, {eq, and}) => and(eq(properties.id, propertyId)),
			})

			if (!result) {
				return {
					id: propertyId,
					dataType: DataType.Text,
				}
			}

			return {
				id: propertyId,
				dataType: getValueTypeAsText(result.type),
			}
		})
	})
}

export function getProperties(typeId: string, args: QueryTypesArgs) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const where = [eq(relations.fromEntityId, typeId), eq(relations.typeId, SystemIds.PROPERTIES)]

		if (args.spaceId) {
			where.push(eq(relations.spaceId, args.spaceId))
		}

		const result = yield* db.use(async (client) => {
			return await client
				.select({
					propertyId: relations.toEntityId,
					propertyType: properties.type,
				})
				.from(relations)
				.innerJoin(properties, eq(relations.toEntityId, properties.id))
				.where(and(...where))
		})

		return result.map((r) => ({
			id: r.propertyId,
			dataType: getValueTypeAsText(r.propertyType),
		}))
	})
}

function getValueTypeAsText(valueTypeId: string): DataType {
	switch (valueTypeId) {
		case "Text":
			return DataType.Text
		case "Number":
			return DataType.Number
		case "Checkbox":
			return DataType.Checkbox
		case "Time":
			return DataType.Time
		case "Point":
			return DataType.Point
		case "Relation":
			return DataType.Relation
		default:
			return DataType.Text
	}
}
