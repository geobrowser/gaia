/**
 * Custom PostGraphile plugin that adds efficient space filtering to the entities connection.
 *
 * Instead of using the computed `entities_space_ids` function (which runs 2 queries per entity),
 * this uses EXISTS subqueries with indexed lookups:
 *
 * Before (slow - O(n) function calls):
 *   WHERE entities_space_ids(e) @> array['space-uuid']
 *
 * After (fast - O(1) indexed EXISTS):
 *   WHERE EXISTS (SELECT 1 FROM values WHERE entity_id = e.id AND space_id = 'space-uuid' LIMIT 1)
 *      OR EXISTS (SELECT 1 FROM relations WHERE from_entity_id = e.id AND space_id = 'space-uuid' LIMIT 1)
 *
 * Usage in GraphQL:
 *   entities(spaceId: "e252f9e1-...", first: 100) { ... }              # single space
 *   entities(spaceIds: { in: ["uuid1", "uuid2"] }, first: 100) { ... } # multiple spaces (OR)
 *
 * Requires indexes (already exist):
 *   - values(entity_id, space_id)
 *   - relations(from_entity_id, space_id)
 */

// Helper to build the EXISTS condition for a single space ID
const buildSingleSpaceCondition = (sql: any, tableAlias: any, spaceId: string) => {
	return sql.fragment`(
		EXISTS (
			SELECT 1 FROM public.values v
			WHERE v.entity_id = ${tableAlias}.id
			AND v.space_id = ${sql.value(spaceId)}::uuid
			LIMIT 1
		)
		OR EXISTS (
			SELECT 1 FROM public.relations r
			WHERE r.from_entity_id = ${tableAlias}.id
			AND r.space_id = ${sql.value(spaceId)}::uuid
			LIMIT 1
		)
	)`
}

// Helper to build the EXISTS condition for multiple space IDs
const buildMultiSpaceCondition = (sql: any, tableAlias: any, spaceIds: string[]) => {
	return sql.fragment`(
		EXISTS (
			SELECT 1 FROM public.values v
			WHERE v.entity_id = ${tableAlias}.id
			AND v.space_id = ANY(${sql.value(spaceIds)}::uuid[])
			LIMIT 1
		)
		OR EXISTS (
			SELECT 1 FROM public.relations r
			WHERE r.from_entity_id = ${tableAlias}.id
			AND r.space_id = ANY(${sql.value(spaceIds)}::uuid[])
			LIMIT 1
		)
	)`
}

// Helper to build condition for checking if entity has any space
const buildHasAnySpaceCondition = (sql: any, tableAlias: any) => {
	return sql.fragment`(
		EXISTS (
			SELECT 1 FROM public.values v
			WHERE v.entity_id = ${tableAlias}.id
			AND v.space_id IS NOT NULL
			LIMIT 1
		)
		OR EXISTS (
			SELECT 1 FROM public.relations r
			WHERE r.from_entity_id = ${tableAlias}.id
			AND r.space_id IS NOT NULL
			LIMIT 1
		)
	)`
}

export const EntitySpaceFilterPlugin = (builder: any) => {
	// Add spaceId/spaceIds arguments and register data generators
	builder.hook("GraphQLObjectType:fields:field:args", (args: any, build: any, context: any) => {
		const {
			scope: { fieldName, isPgFieldConnection, isPgFieldSimpleCollection, pgFieldIntrospection },
			addArgDataGenerator,
		} = context

		// Only modify entity connections/collections
		if (!isPgFieldConnection && !isPgFieldSimpleCollection) {
			return args
		}

		if (pgFieldIntrospection?.name !== "entities") {
			return args
		}

		const { pgSql: sql } = build
		const UUIDType = build.getTypeByName("UUID")
		const UUIDFilterType = build.getTypeByName("UUIDFilter")

		// Register the data generator for spaceId argument
		addArgDataGenerator(({ spaceId }: { spaceId?: string }) => {
			if (!spaceId) return {}
			return {
				pgQuery: (queryBuilder: any) => {
					queryBuilder.where(buildSingleSpaceCondition(sql, queryBuilder.getTableAlias(), spaceId))
				},
			}
		})

		// Register the data generator for spaceIds argument
		addArgDataGenerator(({ spaceIds }: { spaceIds?: any }) => {
			if (!spaceIds) return {}

			return {
				pgQuery: (queryBuilder: any) => {
					const tableAlias = queryBuilder.getTableAlias()

					// Handle `is` operator (single value equality)
					if (spaceIds.is) {
						queryBuilder.where(buildSingleSpaceCondition(sql, tableAlias, spaceIds.is))
					}

					// Handle `isNot` operator (single value inequality)
					if (spaceIds.isNot) {
						queryBuilder.where(sql.fragment`NOT ${buildSingleSpaceCondition(sql, tableAlias, spaceIds.isNot)}`)
					}

					// Handle `in` operator (any of multiple values)
					if (spaceIds.in && spaceIds.in.length > 0) {
						queryBuilder.where(buildMultiSpaceCondition(sql, tableAlias, spaceIds.in))
					}

					// Handle `notIn` operator (none of multiple values)
					if (spaceIds.notIn && spaceIds.notIn.length > 0) {
						queryBuilder.where(sql.fragment`NOT ${buildMultiSpaceCondition(sql, tableAlias, spaceIds.notIn)}`)
					}

					// Handle `isNull` operator
					if (spaceIds.isNull === true) {
						// Entity has no space associations
						queryBuilder.where(sql.fragment`NOT ${buildHasAnySpaceCondition(sql, tableAlias)}`)
					}
					if (spaceIds.isNull === false) {
						// Entity has at least one space association
						queryBuilder.where(buildHasAnySpaceCondition(sql, tableAlias))
					}
				},
			}
		})

		return build.extend(args, {
			spaceId: {
				description: "Filter entities that have data in this space (efficient indexed lookup)",
				type: UUIDType,
			},
			spaceIds: {
				description: "Filter entities by space with operators (in, notIn, is, isNot, etc.)",
				type: UUIDFilterType,
			},
		})
	})
}

export default EntitySpaceFilterPlugin
