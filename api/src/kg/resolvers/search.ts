import {SystemIds} from "@graphprotocol/grc-20"
import {and, desc, eq, inArray, isNotNull, or, sql} from "drizzle-orm"
import {Effect} from "effect"
import type {SearchFilter} from "../../generated/graphql"
import {entities, relations, values} from "../../services/storage/schema"
import {Storage} from "../../services/storage/storage"

interface SearchArgs {
	query: string
	spaceId?: string | null
	filter?: SearchFilter | null
	limit?: number | null
	offset?: number | null
	threshold?: number | null
}

export const search = (args: SearchArgs) =>
	Effect.gen(function* () {
		const db = yield* Storage
		const {query, spaceId, filter, limit = 10, offset = 0, threshold = 0.3} = args

		return yield* db.use(async (client) => {
			const baseQuery = client
				.select({
					id: entities.id,
					createdAt: entities.createdAt,
					createdAtBlock: entities.createdAtBlock,
					updatedAt: entities.updatedAt,
					updatedAtBlock: entities.updatedAtBlock,
					similarity: sql<number>`MAX(similarity(${values.value}, ${query}))`.as("similarity"),
				})
				.from(entities)
				.innerJoin(values, eq(entities.id, values.entityId))

			const whereConditions = [
				// Filter by similarity threshold using pg_trgm
				sql`similarity(${values.value}, ${query}) > ${threshold}`,
				// Filter by space if provided
				spaceId ? eq(values.spaceId, spaceId) : undefined,
				// Only include non-null, non-empty values
				isNotNull(values.value),
				sql`length(trim(${values.value})) > 0`,
			].filter(Boolean)

			// Build query with or without type filtering
			const searchQuery =
				filter?.types?.in && filter.types.in.length > 0
					? baseQuery
							.innerJoin(
								relations,
								and(
									eq(relations.fromEntityId, entities.id),
									eq(relations.typeId, SystemIds.TYPES_PROPERTY),
								),
							)
							.where(and(...whereConditions, inArray(relations.toEntityId, filter.types.in)))
					: baseQuery.where(and(...whereConditions))

			const results = await searchQuery
				.groupBy(
					entities.id,
					entities.createdAt,
					entities.createdAtBlock,
					entities.updatedAt,
					entities.updatedAtBlock,
				)
				.orderBy(desc(sql`MAX(similarity(${values.value}, ${query}))`))
				.limit(limit ?? 10)
				.offset(offset ?? 0)

			// Return entities in the expected format
			return results.map((result) => ({
				id: result.id,
				createdAt: result.createdAt,
				createdAtBlock: result.createdAtBlock,
				updatedAt: result.updatedAt,
				updatedAtBlock: result.updatedAtBlock,
			}))
		})
	})

export const searchNameDescription = (args: SearchArgs) =>
	Effect.gen(function* () {
		const db = yield* Storage
		const {query, spaceId, filter, limit = 10, offset = 0, threshold = 0.3} = args

		return yield* db.use(async (client) => {
			const baseQuery = client
				.select({
					id: entities.id,
					createdAt: entities.createdAt,
					createdAtBlock: entities.createdAtBlock,
					updatedAt: entities.updatedAt,
					updatedAtBlock: entities.updatedAtBlock,
					similarity: sql<number>`MAX(similarity(${values.value}, ${query}))`.as("similarity"),
				})
				.from(entities)
				.innerJoin(values, eq(entities.id, values.entityId))

			const whereConditions = [
				// Filter by similarity threshold
				sql`similarity(${values.value}, ${query}) > ${threshold}`,
				// Only search in name and description properties using SystemIds
				or(
					eq(values.propertyId, SystemIds.NAME_PROPERTY),
					eq(values.propertyId, SystemIds.DESCRIPTION_PROPERTY),
				),
				// Filter by space if provided
				spaceId ? eq(values.spaceId, spaceId) : undefined,
				// Only include non-null, non-empty values
				isNotNull(values.value),
				sql`length(trim(${values.value})) > 0`,
			].filter(Boolean)

			// Build query with or without type filtering
			const searchQuery =
				filter?.types?.in && filter.types.in.length > 0
					? baseQuery
							.innerJoin(
								relations,
								and(
									eq(relations.fromEntityId, entities.id),
									eq(relations.typeId, SystemIds.TYPES_PROPERTY),
								),
							)
							.where(and(...whereConditions, inArray(relations.toEntityId, filter.types.in)))
					: baseQuery.where(and(...whereConditions))

			const results = await searchQuery
				.groupBy(
					entities.id,
					entities.createdAt,
					entities.createdAtBlock,
					entities.updatedAt,
					entities.updatedAtBlock,
				)
				.orderBy(desc(sql`MAX(similarity(${values.value}, ${query}))`))
				.limit(limit ?? 10)
				.offset(offset ?? 0)

			return results.map((result) => ({
				id: result.id,
				createdAt: result.createdAt,
				createdAtBlock: result.createdAtBlock,
				updatedAt: result.updatedAt,
				updatedAtBlock: result.updatedAtBlock,
			}))
		})
	})
