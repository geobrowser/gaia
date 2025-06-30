import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq, inArray, sql} from "drizzle-orm"
import {Effect} from "effect"
import {DataType, type QueryPropertiesArgs, type QueryTypesArgs} from "../../generated/graphql"
import {Batching} from "../../services/storage/dataloaders"
import {properties} from "../../services/storage/schema"
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

		const renderableType = yield* getPropertyRenderableType(propertyId)

		return {
			id: propertyId,
			dataType: getTextAsDataType(property.type),
			renderableType,
		}
	})
}

// Always include Name and Description properties first
const systemProperties = [
	{
		id: SystemIds.NAME_PROPERTY,
		dataType: DataType.Text,
	},
	{
		id: SystemIds.DESCRIPTION_PROPERTY,
		dataType: DataType.Text,
	},
]

const systemPropertyIds = new Set<string>([SystemIds.NAME_PROPERTY, SystemIds.DESCRIPTION_PROPERTY])

export function getPropertiesForType(typeId: string, args: QueryTypesArgs) {
	return Effect.gen(function* () {
		const batching = yield* Batching

		// Load entity relations and filter for properties
		const entityRelations = yield* batching.loadEntityRelations(typeId, args.spaceId)
		const propertyRelations = entityRelations.filter((relation) => relation.typeId === SystemIds.PROPERTIES)

		// Load all property details in parallel
		const propertyDetails = yield* Effect.forEach(
			propertyRelations,
			(relation) => batching.loadProperty(relation.toEntityId),
			{concurrency: "unbounded"},
		).pipe(Effect.withSpan("getPropertiesForType.propertyDetails"))

		const customProperties = propertyDetails
			.filter((property) => property !== null)
			.map((property) => ({
				id: property.id,
				dataType: getTextAsDataType(property.type),
			}))

		// Filter out system properties if they're already in custom properties to avoid duplicates
		const customPropertyIds = new Set(customProperties.map((p) => p.id))
		const systemPropsToAdd = systemProperties.filter((p) => !customPropertyIds.has(p.id))

		// Combine system properties with custom properties
		const allProperties = [...systemPropsToAdd, ...customProperties]

		// Apply pagination
		const limit = args.limit ?? 100
		const offset = args.offset ?? 0

		// Handle limit = 0 case (should return empty array)
		if (limit === 0) {
			return []
		}

		return allProperties.slice(offset, offset + limit)
	})
}

export function getPropertyRelationValueTypes(propertyId: string) {
	return Effect.gen(function* () {
		// Every property automatically has name and description added, which
		// creates a lot of unnecessary work for nested type queries which
		// also read property value types. Since we know name and description
		// are TEXT data types, we can just return early.
		if (systemPropertyIds.has(propertyId)) {
			return []
		}

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
		// Every property automatically has name and description added, which
		// creates a lot of unnecessary work for nested type queries which
		// also read property value types. Since we know name and description
		// are TEXT data types, we can just return early.
		if (systemPropertyIds.has(propertyId)) {
			return null
		}

		const batching = yield* Batching

		const relations = yield* batching.loadEntityRelations(propertyId)

		const renderableTypeRelation = relations.find((relation) => relation.typeId === RENDERABLE_TYPE_RELATION_ID)

		if (!renderableTypeRelation) {
			return null
		}

		return renderableTypeRelation.toEntityId
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
