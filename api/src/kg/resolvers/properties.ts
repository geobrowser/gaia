import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq} from "drizzle-orm"
import {Effect} from "effect"
import {DataType, type QueryPropertiesArgs, type QueryTypesArgs} from "../../generated/graphql"
import {properties, relations} from "../../services/storage/schema"
import {Storage} from "../../services/storage/storage"

export function getProperties(args: QueryPropertiesArgs) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const dataTypeFilter = args.filter?.dataType
			const result = await client.query.properties.findMany({
				where: dataTypeFilter
					? (properties, {eq}) => eq(properties.type, getDataTypeAsText(dataTypeFilter))
					: undefined,
				limit: args.limit ?? 100,
				offset: args.offset ?? 0,
			})

			return result.map((property) => ({
				id: property.id,
				dataType: getTextAsDataType(property.type),
			}))
		})
	})
}

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
				dataType: getTextAsDataType(result.type),
			}
		})
	})
}

export function getPropertiesForType(typeId: string, args: QueryTypesArgs) {
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
				.limit(Number(args.limit))
				.offset(Number(args.offset))
		})

		return result.map((r) => ({
			id: r.propertyId,
			dataType: getTextAsDataType(r.propertyType),
		}))
	})
}

export function getPropertyRelationValueTypes(propertyId: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const result = yield* db.use(async (client) => {
			return await client.query.relations.findMany({
				where: (relations, {and, eq}) =>
					and(
						eq(relations.fromEntityId, propertyId),
						eq(relations.typeId, SystemIds.RELATION_VALUE_RELATIONSHIP_TYPE),
					),
				with: {
					toEntity: true,
				},
			})
		})

		return result.map((r) => r.toEntity)
	})
}

function getTextAsDataType(valueTypeId: string): DataType {
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

function getDataTypeAsText(dataType: DataType): "Text" | "Number" | "Checkbox" | "Time" | "Point" | "Relation" {
	switch (dataType) {
		case DataType.Text:
			return "Text"
		case DataType.Number:
			return "Number"
		case DataType.Checkbox:
			return "Checkbox"
		case DataType.Time:
			return "Time"
		case DataType.Point:
			return "Point"
		case DataType.Relation:
			return "Relation"
		default:
			return "Text"
	}
}
