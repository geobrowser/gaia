import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq, inArray, sql} from "drizzle-orm"
import {Effect} from "effect"
import {DataType, type QueryPropertiesArgs, type QueryTypesArgs, RenderableType} from "../../generated/graphql"
import {Batching} from "../../services/storage/dataloaders"
import {properties, relations} from "../../services/storage/schema"
import {Storage} from "../../services/storage/storage"

// Constants for renderable type relations
const RENDERABLE_TYPE_RELATION_ID = "2316bbe1-c76f-4635-83f2-3e03b4f1fe46"

export function getProperties(args: QueryPropertiesArgs) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const dataTypeFilter = args.filter?.dataType
			const idFilter = args.filter?.id

			// Build where conditions
			const whereConditions = []

			if (dataTypeFilter) {
				whereConditions.push(eq(properties.type, getDataTypeAsText(dataTypeFilter)))
			}

			if (idFilter?.in) {
				if (idFilter.in.length > 0) {
					whereConditions.push(inArray(properties.id, idFilter.in))
				} else {
					// Empty array means no results should be returned
					whereConditions.push(sql`1 = 0`)
				}
			}

			const result = await client.query.properties.findMany({
				where: whereConditions.length > 0 ? and(...whereConditions) : undefined,
				limit: args.limit ?? 100,
				offset: args.offset ?? 0,
			})

			return result.map((property) => ({
				id: property.id,
				dataType: getTextAsDataType(property.type),
				renderableType: null, // Will be resolved by field resolver
			}))
		})
	})
}

export function getProperty(propertyId: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching

		const property = yield* batching.loadProperty(propertyId)

		if (!property) {
			return null
		}

		return {
			id: propertyId,
			dataType: getTextAsDataType(property.type),
			renderableType: null, // Will be resolved by field resolver
		}
	})
}

export function getPropertiesForType(typeId: string, args: QueryTypesArgs) {
	return Effect.gen(function* () {
		const db = yield* Storage

		// Always include Name and Description properties first
		const systemProperties = [
			{
				id: SystemIds.NAME_PROPERTY,
				dataType: DataType.Text,
				renderableType: null,
			},
			{
				id: SystemIds.DESCRIPTION_PROPERTY,
				dataType: DataType.Text,
				renderableType: null,
			},
		]

		// Query existing custom properties with space filtering (no pagination here)
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

		const customProperties = result.map((r) => ({
			id: r.propertyId,
			dataType: getTextAsDataType(r.propertyType),
			renderableType: null, // Will be resolved by field resolver
		}))

		// Filter out system properties if they're already in custom properties to avoid duplicates
		const customPropertyIds = new Set(customProperties.map((p) => p.id))
		const systemPropsToAdd = systemProperties.filter((p) => !customPropertyIds.has(p.id))

		// Combine system properties with custom properties
		const allProperties = [...systemPropsToAdd, ...customProperties]

		// Apply pagination to the combined result
		const limit = Number(args.limit ?? 100)
		const offset = Number(args.offset ?? 0)

		return allProperties.slice(offset, offset + limit)
	})
}

export function getPropertyRelationValueTypes(propertyId: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching

		const relations = yield* batching.loadEntityRelations(propertyId)

		const relationValueTypes = relations
			.filter((relation) => relation.typeId === SystemIds.RELATION_VALUE_RELATIONSHIP_TYPE)
			.map((r) => ({id: r.toEntityId}))

		return relationValueTypes
	})
}

export function getPropertyRenderableType(propertyId: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching

		const relations = yield* batching.loadEntityRelations(propertyId)

		const renderableTypeRelation = relations.find((relation) => relation.typeId === RENDERABLE_TYPE_RELATION_ID)

		if (!renderableTypeRelation) {
			return null
		}

		// Map the toEntityId to RenderableType enum
		switch (renderableTypeRelation.toEntityId) {
			case SystemIds.IMAGE:
				return RenderableType.Image
			case SystemIds.URL:
				return RenderableType.Url
			default:
				return null
		}
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
