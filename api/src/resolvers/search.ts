import {SystemIds} from "@graphprotocol/grc-20"
import {and, desc, eq, isNotNull, or, sql} from "drizzle-orm"
import {Effect} from "effect"
import {entities, values} from "../services/storage/schema"
import {Storage} from "../services/storage/storage"

interface SearchArgs {
	query: string
	spaceId?: string | null
	limit?: number | null
	offset?: number | null
	threshold?: number | null
}

export const search = (args: SearchArgs) =>
	Effect.gen(function* () {
		const db = yield* Storage
		const {query, spaceId, limit = 10, offset = 0, threshold = 0.3} = args

		return yield* db.use(async (client) => {
			const searchQuery = client
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
				.where(
					and(
						// Filter by similarity threshold using pg_trgm
						sql`similarity(${values.value}, ${query}) > ${threshold}`,
						// Filter by space if provided
						spaceId ? eq(values.spaceId, spaceId) : undefined,
						// Only include non-null, non-empty values
						isNotNull(values.value),
						sql`length(trim(${values.value})) > 0`,
					),
				)
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

			const results = await searchQuery

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
		const {query, spaceId, limit = 10, offset = 0, threshold = 0.3} = args

		return yield* db.use(async (client) => {
			const searchQuery = client
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
				.where(
					and(
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
					),
				)
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

			const results = await searchQuery

			return results.map((result) => ({
				id: result.id,
				createdAt: result.createdAt,
				createdAtBlock: result.createdAtBlock,
				updatedAt: result.updatedAt,
				updatedAtBlock: result.updatedAtBlock,
			}))
		})
	})
