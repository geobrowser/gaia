import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {BlockType, DataSourceType, type QueryEntitiesArgs, type QueryRelationsArgs} from "../../generated/graphql"
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
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.entities.findFirst({
				where: (entities, {eq}) => eq(entities.id, id),
			})

			if (!result) {
				return null
			}

			return {
				id: result.id,
				createdAt: result.createdAt,
				createdAtBlock: result.createdAtBlock,
				updatedAt: result.updatedAt,
				updatedAtBlock: result.updatedAtBlock,
			}
		})
	})
}

export function getEntityName(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const nameProperty = yield* db.use(async (client) => {
			const result = await client.query.values.findFirst({
				where: (values, {eq, and}) =>
					and(eq(values.propertyId, SystemIds.NAME_PROPERTY), eq(values.entityId, id)),
			})

			return result
		})

		return nameProperty?.value ?? null
	})
}

export function getEntityDescription(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const nameProperty = yield* db.use(async (client) => {
			const result = await client.query.values.findFirst({
				where: (values, {eq, and}) =>
					and(eq(values.propertyId, SystemIds.DESCRIPTION_PROPERTY), eq(values.entityId, id)),
			})

			return result
		})

		return nameProperty?.value ?? null
	})
}

export function getValues(id: string, spaceId?: string | null) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.values.findMany({
				where: (values, {eq, and}) => {
					const conditions = [eq(values.entityId, id)]
					if (spaceId) {
						conditions.push(eq(values.spaceId, spaceId))
					}
					return and(...conditions)
				},
			})

			return result
		})
	})
}

export function getRelations(id: string, spaceId?: string | null) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findMany({
				where: (relations, {eq, and}) => {
					const conditions = [eq(relations.fromEntityId, id)]
					if (spaceId) {
						conditions.push(eq(relations.spaceId, spaceId))
					}
					return and(...conditions)
				},
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

export function getBacklinks(id: string, spaceId?: string | null) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findMany({
				where: (relations, {eq, and}) => {
					const conditions = [eq(relations.toEntityId, id)]
					if (spaceId) {
						conditions.push(eq(relations.spaceId, spaceId))
					}
					return and(...conditions)
				},
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
	const {filter, limit = 100, offset = 0} = args

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
					const conditions: any[] = []

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
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findMany({
				where: (relations, {eq, and}) =>
					and(eq(relations.fromEntityId, id), eq(relations.typeId, SystemIds.TYPES_PROPERTY)),
				with: {
					toEntity: true,
				},
			})

			return result.map((relation) => ({
				id: relation.toEntity.id,
				createdAt: relation.toEntity.createdAt,
				createdAtBlock: relation.toEntity.createdAtBlock,
				updatedAt: relation.toEntity.updatedAt,
				updatedAtBlock: relation.toEntity.updatedAtBlock,
			}))
		})
	})
}

export function getSpaces(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			// There's currently some kind of circular dependency or disambiguation
			// issue with drizzle if we try and query properties and relations at
			// the same time using query.entities.findFirst({ with: { properties: true, relations: true } })
			//
			// For now we just query them separately. This avoids joins so might be
			// faster anyway (needs validation).
			const [values, relations] = await Promise.all([
				client.query.values.findMany({
					where: (values, {eq}) => eq(values.entityId, id),
					columns: {
						spaceId: true,
					},
				}),
				client.query.relations.findMany({
					where: (relations, {eq}) => eq(relations.fromEntityId, id),
					columns: {
						spaceId: true,
					},
				}),
			])

			const propertySpaces = values.map((p) => p.spaceId)
			const relationSpaces = relations.map((r) => r.spaceId)

			return Array.from(new Set([...propertySpaces, ...relationSpaces]))
		})
	})
}

export function getBlocks(entityId: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			// Get all block relations for the entity
			const blockRelations = await client.query.relations.findMany({
				where: (relations, {eq, and}) =>
					and(eq(relations.fromEntityId, entityId), eq(relations.typeId, SystemIds.BLOCKS)),
				with: {
					toEntity: {
						with: {
							fromRelations: {
								with: {
									toEntity: true,
								},
							},
							values: true,
						},
					},
				},
				orderBy: (relations, {asc}) => asc(relations.position),
			})

			return blockRelations.map((relation) => {
				const block = relation.toEntity
				const blockTypeId =
					block.fromRelations.find((r) => r.typeId === SystemIds.TYPES_PROPERTY)?.toEntity?.id ?? null

				// Determine the appropriate value based on block type
				let value: string | null = null
				let type: BlockType = BlockType.Text
				let dataSourceType: DataSourceType | null = null

				if (blockTypeId === SystemIds.TEXT_BLOCK) {
					type = BlockType.Text
					value = block.values.find((v) => v.propertyId === SystemIds.MARKDOWN_CONTENT)?.value ?? null
				} else if (blockTypeId === SystemIds.IMAGE_TYPE) {
					type = BlockType.Image
					value = block.values.find((v) => v.propertyId === SystemIds.IMAGE_URL_PROPERTY)?.value ?? null
				} else if (blockTypeId === SystemIds.DATA_BLOCK) {
					type = BlockType.Data
					value = block.values.find((v) => v.propertyId === SystemIds.FILTER)?.value ?? null
					const maybeDataSourceType =
						block.fromRelations.find((r) => r.typeId === SystemIds.DATA_SOURCE_TYPE_RELATION_TYPE)?.toEntity
							?.id ?? null

					dataSourceType = getDataSourceType(maybeDataSourceType)
				}

				return {
					id: block.id,
					type: type,
					value: value,
					dataSourceType,
					entity: {
						id: block.id,
						createdAt: block.createdAt,
						createdAtBlock: block.createdAtBlock,
						updatedAt: block.updatedAt,
						updatedAtBlock: block.updatedAtBlock,
					},
				}
			})
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
