import {SystemIds} from "@graphprotocol/grc-20"
import {and, desc, eq, inArray, isNotNull, sql} from "drizzle-orm"
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

// Helper function to build type filter conditions for entity types
function buildTypeFilterConditions(filter: SearchFilter, entityIds: string[]) {
	const conditions = []

	// Handle types filter - filter entities that have these types
	if (filter.types?.in && filter.types.in.length > 0) {
		conditions.push(inArray(relations.toEntityId, filter.types.in))
	}

	// Always filter by the candidate entity IDs
	conditions.push(inArray(relations.fromEntityId, entityIds))
	conditions.push(eq(relations.typeId, SystemIds.TYPES_PROPERTY))

	return conditions
}

// Check if filter needs type joining
function needsTypeFilter(filter?: SearchFilter | null): boolean {
	if (!filter) return false
	return !!(filter.types?.in && filter.types.in.length > 0)
}

export const search = (args: SearchArgs) =>
	Effect.gen(function* () {
		const db = yield* Storage
		const {query, spaceId, filter, limit = 10, offset = 0, threshold = 0.3} = args

		return yield* db.use(async (client) => {
			// First, find entities by searching their names using the GIN index
			const nameSearchQuery = client
				.select({
					entityId: values.entityId,
					similarity: sql<number>`similarity(${values.value}, ${query})`.as("similarity"),
				})
				.from(values)
				.where(
					and(
						// Only search in name properties
						eq(values.propertyId, SystemIds.NAME_PROPERTY),
						// Filter by similarity threshold using pg_trgm
						sql`similarity(${values.value}, ${query}) > ${threshold}`,
						// Only include non-null, non-empty values
						isNotNull(values.value),
						sql`length(trim(${values.value})) > 0`,
						// Filter by space if provided
						...(spaceId ? [eq(values.spaceId, spaceId)] : []),
					),
				)
				.orderBy(desc(sql`similarity(${values.value}, ${query})`))
				.limit(Math.max(Number(limit) * 2, 50)) // Get more candidates for type filtering
				.offset(Number(offset || 0))

			const nameResults = await nameSearchQuery
			if (!nameResults || nameResults.length === 0) return []

			let entityIds = nameResults.map((result) => result.entityId)

			// Apply type filtering if needed
			if (needsTypeFilter(filter) && entityIds.length > 0) {
				const typeFilterQuery = client
					.select({
						fromEntityId: relations.fromEntityId,
					})
					.from(relations)
					.where(and(...buildTypeFilterConditions(filter as SearchFilter, entityIds)))
					.groupBy(relations.fromEntityId)

				const typeFiltered = await typeFilterQuery
				entityIds = typeFiltered.map((tf) => tf.fromEntityId)

				if (entityIds.length === 0) return []
			}

			// Get the final entity details, preserving similarity order
			const entityResults = await client
				.select({
					id: entities.id,
					createdAt: entities.createdAt,
					createdAtBlock: entities.createdAtBlock,
					updatedAt: entities.updatedAt,
					updatedAtBlock: entities.updatedAtBlock,
				})
				.from(entities)
				.where(inArray(entities.id, entityIds))
				.limit(Number(limit))
				.offset(0) // Already applied offset in name search

			// Sort results to match the original similarity order
			const entityMap = new Map(entityResults.map((e) => [e.id, e]))
			const sortedResults = entityIds
				.map((id) => entityMap.get(id))
				.filter(Boolean) // Remove any undefined entries
				.slice(0, Number(limit))

			// TypeScript narrowing to ensure results are defined
			return sortedResults.map((result) => {
				// This is safe because we filtered out undefined values above
				if (!result) throw new Error("Unexpected undefined result after filtering")
				return {
					id: result.id,
					createdAt: result.createdAt,
					createdAtBlock: result.createdAtBlock,
					updatedAt: result.updatedAt,
					updatedAtBlock: result.updatedAtBlock,
				}
			})
		})
	})

// Legacy function kept for compatibility - now just calls main search
export const searchNameDescription = (args: SearchArgs) => search(args)
