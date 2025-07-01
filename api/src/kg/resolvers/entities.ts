import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {BatchingError, type GraphQLContext} from "~/src/types"
import {
	BlockType,
	DataSourceType,
	type QueryEntitiesArgs,
	type QueryRelationsArgs,
	type RelationFilter,
	type ValueFilter,
} from "../../generated/graphql"
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

export function getEntity(id: string, context: GraphQLContext) {
	return Effect.gen(function* () {
		const entity = yield* Effect.tryPromise({
			try: () => context.entitiesLoader.load(id),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity ${id}: ${String(error)}`,
				}),
		}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("getEntity.loadEntity"))

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

export function getEntityName(id: string, context: GraphQLContext) {
	return Effect.gen(function* () {
		const name = yield* Effect.tryPromise({
			try: () => context.entityNamesLoader.load(id),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity name ${id}: ${String(error)}`,
				}),
		}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("getEntityName.loadEntityName"))

		return name
	})
}

export function getEntityDescription(id: string, context: GraphQLContext) {
	return Effect.gen(function* () {
		const description = yield* Effect.tryPromise({
			try: () => context.entityDescriptionsLoader.load(id),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity description ${id}: ${String(error)}`,
				}),
		}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("loadEntityDescription"))

		return description
	})
}

export function getValues(
	{
		id,
		spaceId,
		filter,
	}: {
		id: string
		spaceId?: string | null
		filter?: ValueFilter | null
	},
	context: GraphQLContext,
) {
	return Effect.gen(function* () {
		const values = yield* Effect.tryPromise({
			try: () => context.entityValuesLoader.load({entityId: id, spaceId, filter}),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity values for ${id}: ${String(error)}`,
				}),
		}).pipe(Effect.annotateSpans({entityId: id, spaceId, filter}), Effect.withSpan("getValues.loadEntityValues"))

		return values
	})
}

export function getRelations(
	{
		id,
		spaceId,
		filter,
	}: {
		id: string
		spaceId?: string | null
		filter?: RelationFilter | null
	},
	context: GraphQLContext,
) {
	return Effect.gen(function* () {
		const relations = yield* Effect.tryPromise({
			try: () => context.entityRelationsLoader.load({entityId: id, spaceId, filter}),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity relations for ${id}: ${String(error)}`,
				}),
		}).pipe(
			Effect.annotateSpans({entityId: id, spaceId, filter}),
			Effect.withSpan("getRelations.loadEntityRelations"),
		)

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

export function getBacklinks(
	{
		id,
		spaceId,
		filter,
	}: {
		id: string
		spaceId?: string | null
		filter?: RelationFilter | null
	},
	context: GraphQLContext,
) {
	return Effect.gen(function* () {
		const backlinks = yield* Effect.tryPromise({
			try: () => context.entityBacklinksLoader.load({entityId: id, spaceId, filter}),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity backlinks for ${id}: ${String(error)}`,
				}),
		}).pipe(Effect.annotateSpans({entityId: id, spaceId, filter}), Effect.withSpan("loadEntityBacklinks"))

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

export function getEntityTypes(id: string, context: GraphQLContext) {
	return Effect.gen(function* () {
		const relations = yield* Effect.tryPromise({
			try: () => context.entityRelationsLoader.load({entityId: id}),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity relations for ${id}: ${String(error)}`,
				}),
		}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("getEntityTypes.loadEntityRelations"))

		// Filter for type relations and load the target entities
		const typeRelations = relations.filter((relation) => relation.typeId === SystemIds.TYPES_PROPERTY)

		// Use batching to load the type entities
		const typeEntities = yield* Effect.forEach(
			typeRelations,
			(relation) =>
				Effect.tryPromise({
					try: () => context.entitiesLoader.load(relation.toEntityId),
					catch: (error) =>
						new BatchingError({
							cause: error,
							message: `Failed to batch load entity ${id}: ${String(error)}`,
						}),
				}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("getEntityTypes.loadEntity")),
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

export function getSpaces(id: string, context: GraphQLContext) {
	return Effect.gen(function* () {
		// Load both values and relations for the entity
		const [values, relations] = yield* Effect.all([
			Effect.tryPromise({
				try: () => context.entityValuesLoader.load({entityId: id}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity values for ${id}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("getSpaces.loadEntityValues")),
			Effect.tryPromise({
				try: () => context.entityRelationsLoader.load({entityId: id}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity relations for ${id}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("getSpaces.loadEntityRelations")),
		])

		const propertySpaces = values.map((p) => p.spaceId)
		const relationSpaces = relations.map((r) => r.spaceId)

		return Array.from(new Set([...propertySpaces, ...relationSpaces]))
	})
}

export function getBlocks(entityId: string, context: GraphQLContext) {
	return Effect.gen(function* () {
		// Get all relations for the entity
		const relations = yield* Effect.tryPromise({
			try: () => context.entityRelationsLoader.load({entityId}),
			catch: (error) =>
				new BatchingError({
					cause: error,
					message: `Failed to batch load entity relations for ${entityId}: ${String(error)}`,
				}),
		}).pipe(Effect.annotateSpans({entityId}), Effect.withSpan("getBlocks.loadEntityRelations"))

		// Filter for block relations
		const blockRelations = relations
			.filter((relation) => relation.typeId === SystemIds.BLOCKS)
			.sort((a, b) => (Number(a.position) || 0) - (Number(b.position) || 0))

		// Load all block entities in parallel
		const blockEntities = yield* Effect.forEach(
			blockRelations,
			(relation) =>
				Effect.gen(function* () {
					const entity = yield* Effect.tryPromise({
						try: () => context.entitiesLoader.load(relation.toEntityId),
						catch: (error) =>
							new BatchingError({
								cause: error,
								message: `Failed to batch load entity ${relation.toEntityId}: ${String(error)}`,
							}),
					}).pipe(
						Effect.annotateSpans({entityId: relation.toEntityId}),
						Effect.withSpan("getBlocks.loadEntity"),
					)

					if (!entity) return null

					// Load entity relations and values in parallel
					const [entityRelations, entityValues] = yield* Effect.all([
						Effect.tryPromise({
							try: () =>
								context.entityRelationsLoader.load({
									entityId: relation.toEntityId,
								}),
							catch: (error) =>
								new BatchingError({
									cause: error,
									message: `Failed to batch load entity relations for ${relation.toEntityId}: ${String(error)}`,
								}),
						}).pipe(
							Effect.annotateSpans({entityId: relation.toEntityId}),
							Effect.withSpan("getBlocks.loadEntityRelations"),
						),
						Effect.tryPromise({
							try: () =>
								context.entityValuesLoader.load({
									entityId: relation.toEntityId,
								}),
							catch: (error) =>
								new BatchingError({
									cause: error,
									message: `Failed to batch load entity values for ${relation.toEntityId}: ${String(error)}`,
								}),
						}).pipe(
							Effect.annotateSpans({entityId: relation.toEntityId}),
							Effect.withSpan("getBlocks.loadEntityValues"),
						),
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
