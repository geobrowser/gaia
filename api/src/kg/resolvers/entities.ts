import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {
	BlockType,
	DataSourceType,
	type QueryEntitiesArgs,
	type QueryRelationsArgs,
	type RelationFilter,
	type ValueFilter,
} from "../../generated/graphql"
import {Batching} from "../../services/storage/dataloaders"
import {Storage} from "../../services/storage/storage"
import {buildEntityWhere, type EntityFilter} from "./filters"

export function getEntities(args: QueryEntitiesArgs) {
	const {filter, limit = 100, offset = 0, spaceId} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		const whereClauses = buildEntityWhere(filter as EntityFilter, spaceId)

		return yield* db.use(async (client) => {
			const entitiesWithMatchingValue = await client.query.entities.findMany({
				limit: Number(limit),
				offset: Number(offset),
				with: {
					values: {
						columns: {
							propertyId: true,
							value: true,
						},
					},
				},
				where: whereClauses,
			})

			return entitiesWithMatchingValue.map((result) => {
				return {
					id: result.id,
					createdAt: result.createdAt,
					createdAtBlock: result.createdAtBlock,
					updatedAt: result.updatedAt,
					updatedAtBlock: result.updatedAtBlock,
					name: result.values.find((p) => p.propertyId === SystemIds.NAME_PROPERTY)?.value,
				}
			})
		})
	})
}

export function getEntity(id: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching

		const entity = yield* batching.loadEntity(id)

		if (!entity) {
			return null
		}

		return {
			id: entity.id,
			createdAt: entity.createdAt,
			createdAtBlock: entity.createdAtBlock,
			updatedAt: entity.updatedAt,
			updatedAtBlock: entity.updatedAtBlock,
		}
	})
}

export function getEntityName(id: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching
		const name = yield* batching.loadEntityName(id)

		return name
	})
}

export function getEntityDescription(id: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching
		const description = yield* batching.loadEntityDescription(id)

		return description
	})
}

export function getValues(id: string, spaceId?: string | null, filter?: ValueFilter | null) {
	return Effect.gen(function* () {
		const batching = yield* Batching
		const values = yield* batching.loadEntityValues(id, spaceId, filter)

		return values
	})
}

export function getRelations(id: string, spaceId?: string | null, filter?: RelationFilter | null) {
	return Effect.gen(function* () {
		const batching = yield* Batching
		const relations = yield* batching.loadEntityRelations(id, spaceId, filter)

		return relations.map((relation) => ({
			id: relation.id,
			entityId: relation.entityId,
			typeId: relation.typeId,
			fromId: relation.fromEntityId,
			toId: relation.toEntityId,
			toSpaceId: relation.toSpaceId,
			verified: relation.verified,
			position: relation.position,
			spaceId: relation.spaceId,
		}))
	})
}

export function getBacklinks(id: string, spaceId?: string | null, filter?: RelationFilter | null) {
	return Effect.gen(function* () {
		const batching = yield* Batching
		const backlinks = yield* batching.loadEntityBacklinks(id, spaceId, filter)

		return backlinks.map((relation) => ({
			id: relation.id,
			entityId: relation.entityId,
			typeId: relation.typeId,
			fromId: relation.fromEntityId,
			toId: relation.toEntityId,
			toSpaceId: relation.toSpaceId,
			verified: relation.verified,
			position: relation.position,
			spaceId: relation.spaceId,
		}))
	})
}

export function getRelation(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findFirst({
				where: (relations, {eq}) => eq(relations.id, id),
			})

			if (!result) {
				return null
			}

			return {
				id: result.id,
				entityId: result.entityId,
				typeId: result.typeId,
				fromId: result.fromEntityId,
				toId: result.toEntityId,
				toSpaceId: result.toSpaceId,
				verified: result.verified,
				position: result.position,
				spaceId: result.spaceId,
			}
		})
	})
}

export function getAllRelations(args: QueryRelationsArgs) {
	const {filter, spaceId, limit = 100, offset = 0} = args

	return Effect.gen(function* () {
		// Early return for empty string filters since they will never match any valid entity IDs
		if (filter?.relationEntityId === "") {
			return []
		}
		if (filter?.typeId === "") {
			return []
		}
		if (filter?.fromEntityId === "") {
			return []
		}
		if (filter?.toEntityId === "") {
			return []
		}

		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findMany({
				where: (relations, {eq, and}) => {
					const conditions: ReturnType<typeof eq>[] = []

					if (filter?.typeId) {
						conditions.push(eq(relations.typeId, filter.typeId))
					}
					if (filter?.fromEntityId) {
						conditions.push(eq(relations.fromEntityId, filter.fromEntityId))
					}
					if (filter?.toEntityId) {
						conditions.push(eq(relations.toEntityId, filter.toEntityId))
					}
					if (filter?.relationEntityId) {
						conditions.push(eq(relations.entityId, filter.relationEntityId))
					}
					if (spaceId) {
						conditions.push(eq(relations.spaceId, spaceId))
					}

					return conditions.length > 0 ? and(...conditions) : undefined
				},
				limit: Number(limit),
				offset: Number(offset),
			})

			return result.map((relation) => ({
				id: relation.id,
				entityId: relation.entityId,
				typeId: relation.typeId,
				fromId: relation.fromEntityId,
				toId: relation.toEntityId,
				toSpaceId: relation.toSpaceId,
				verified: relation.verified,
				position: relation.position,
				spaceId: relation.spaceId,
			}))
		})
	})
}

export function getEntityTypes(id: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching
		const relations = yield* batching.loadEntityRelations(id)

		// Filter for type relations and load the target entities
		const typeRelations = relations.filter((relation) => relation.typeId === SystemIds.TYPES_PROPERTY)

		// Use batching to load the type entities
		const typeEntities = yield* Effect.forEach(
			typeRelations,
			(relation) => batching.loadEntity(relation.toEntityId),
			{concurrency: "unbounded"},
		)

		// Filter out null results and transform
		return typeEntities
			.filter((entity): entity is NonNullable<typeof entity> => entity !== null)
			.map((entity) => ({
				id: entity.id,
				createdAt: entity.createdAt,
				createdAtBlock: entity.createdAtBlock,
				updatedAt: entity.updatedAt,
				updatedAtBlock: entity.updatedAtBlock,
			}))
	})
}

export function getSpaces(id: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching

		// Load both values and relations for the entity
		const [values, relations] = yield* Effect.all([batching.loadEntityValues(id), batching.loadEntityRelations(id)])

		const propertySpaces = values.map((p) => p.spaceId)
		const relationSpaces = relations.map((r) => r.spaceId)

		return Array.from(new Set([...propertySpaces, ...relationSpaces]))
	})
}

export function getBlocks(entityId: string) {
	return Effect.gen(function* () {
		const batching = yield* Batching

		// Get all relations for the entity
		const relations = yield* batching.loadEntityRelations(entityId)

		// Filter for block relations
		const blockRelations = relations
			.filter((relation) => relation.typeId === SystemIds.BLOCKS)
			.sort((a, b) => (Number(a.position) || 0) - (Number(b.position) || 0))

		// Load all block entities in parallel
		const blockEntities = yield* Effect.forEach(
			blockRelations,
			(relation) =>
				Effect.gen(function* () {
					const entity = yield* batching.loadEntity(relation.toEntityId)
					if (!entity) return null

					// Load entity relations and values in parallel
					const [entityRelations, entityValues] = yield* Effect.all([
						batching.loadEntityRelations(relation.toEntityId),
						batching.loadEntityValues(relation.toEntityId),
					])

					return {
						entity,
						relations: entityRelations,
						values: entityValues,
					}
				}),
			{concurrency: "unbounded"},
		)

		return blockEntities
			.filter((blockData): blockData is NonNullable<typeof blockData> => blockData !== null)
			.map((blockData) => {
				const {entity, relations: fromRelations, values} = blockData
				const blockTypeId = fromRelations.find((r) => r.typeId === SystemIds.TYPES_PROPERTY)?.toEntityId ?? null

				// Determine the appropriate value based on block type
				let value: string | null = null
				let type: BlockType = BlockType.Text
				let dataSourceType: DataSourceType | null = null

				if (blockTypeId === SystemIds.TEXT_BLOCK) {
					type = BlockType.Text
					value = values.find((v) => v.propertyId === SystemIds.MARKDOWN_CONTENT)?.value ?? null
				} else if (blockTypeId === SystemIds.IMAGE_TYPE) {
					type = BlockType.Image
					value = values.find((v) => v.propertyId === SystemIds.IMAGE_URL_PROPERTY)?.value ?? null
				} else if (blockTypeId === SystemIds.DATA_BLOCK) {
					type = BlockType.Data
					value = values.find((v) => v.propertyId === SystemIds.FILTER)?.value ?? null
					const maybeDataSourceType =
						fromRelations.find((r) => r.typeId === SystemIds.DATA_SOURCE_TYPE_RELATION_TYPE)?.toEntityId ??
						null

					dataSourceType = getDataSourceType(maybeDataSourceType)
				}

				return {
					id: entity.id,
					type: type,
					value: value,
					dataSourceType,
					entity: {
						id: entity.id,
						createdAt: entity.createdAt,
						createdAtBlock: entity.createdAtBlock,
						updatedAt: entity.updatedAt,
						updatedAtBlock: entity.updatedAtBlock,
					},
				}
			})
	})
}

function getDataSourceType(dataSourceId: string | null): DataSourceType | null {
	if (!dataSourceId) return null

	switch (dataSourceId) {
		case SystemIds.QUERY_DATA_SOURCE:
			return DataSourceType.Query
		case SystemIds.ALL_OF_GEO_DATA_SOURCE:
			return DataSourceType.Geo
		case SystemIds.COLLECTION_DATA_SOURCE:
			return DataSourceType.Collection
		default:
			return null
	}
}
