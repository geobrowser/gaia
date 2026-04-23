/**
 * Custom PostGraphile plugin that adds efficient space and type filtering to the entities connection.
 *
 * Instead of using computed functions (which run queries per entity),
 * this uses EXISTS subqueries with indexed lookups.
 *
 * Space filtering:
 *   Before (slow - O(n) function calls):
 *     WHERE entities_space_ids(e) @> array['space-uuid']
 *   After (fast - O(1) indexed EXISTS):
 *     WHERE EXISTS (SELECT 1 FROM values WHERE entity_id = e.id AND space_id = 'space-uuid' LIMIT 1)
 *        OR EXISTS (SELECT 1 FROM relations WHERE from_entity_id = e.id AND space_id = 'space-uuid' LIMIT 1)
 *
 * Type filtering:
 *   Before (slow - O(n) function calls):
 *     WHERE entities_type_ids(e) @> array['type-uuid']
 *   After (fast - O(1) indexed EXISTS):
 *     WHERE EXISTS (SELECT 1 FROM relations WHERE from_entity_id = e.id
 *                   AND type_id = 'SystemIds.Types' AND to_entity_id = 'type-uuid' LIMIT 1)
 *
 * Usage in GraphQL:
 *   entities(spaceId: "uuid", first: 100) { ... }              # single space
 *   entities(spaceIds: { in: ["uuid1", "uuid2"] }, first: 100) { ... } # multiple spaces (OR)
 *   entities(typeId: "uuid", first: 100) { ... }               # single type
 *   entities(typeIds: { in: ["uuid1", "uuid2"] }, first: 100) { ... }  # multiple types (OR)
 *
 * Requires indexes (already exist):
 *   - values(entity_id, space_id)
 *   - relations(from_entity_id, space_id)
 *   - relations(from_entity_id, type_id, to_entity_id)
 */

import {SystemIds} from "@geoprotocol/geo-sdk"

const SYSTEM_IDS_TYPES = SystemIds.TYPES_PROPERTY

// ============================================================================
// Space filter helpers
// ============================================================================

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

// ============================================================================
// Type filter helpers
// ============================================================================

// Helper to build the EXISTS condition for a single type ID
const buildSingleTypeCondition = (sql: any, tableAlias: any, typeId: string) => {
	return sql.fragment`EXISTS (
		SELECT 1 FROM public.relations r
		WHERE r.from_entity_id = ${tableAlias}.id
		AND r.type_id = ${sql.value(SYSTEM_IDS_TYPES)}::uuid
		AND r.to_entity_id = ${sql.value(typeId)}::uuid
		LIMIT 1
	)`
}

// Helper to build the EXISTS condition for multiple type IDs
const buildMultiTypeCondition = (sql: any, tableAlias: any, typeIds: string[]) => {
	return sql.fragment`EXISTS (
		SELECT 1 FROM public.relations r
		WHERE r.from_entity_id = ${tableAlias}.id
		AND r.type_id = ${sql.value(SYSTEM_IDS_TYPES)}::uuid
		AND r.to_entity_id = ANY(${sql.value(typeIds)}::uuid[])
		LIMIT 1
	)`
}

// Helper to build condition for checking if entity has any type
const buildHasAnyTypeCondition = (sql: any, tableAlias: any) => {
	return sql.fragment`EXISTS (
		SELECT 1 FROM public.relations r
		WHERE r.from_entity_id = ${tableAlias}.id
		AND r.type_id = ${sql.value(SYSTEM_IDS_TYPES)}::uuid
		LIMIT 1
	)`
}

// ============================================================================
// Plugin
// ============================================================================

export const EntitySpaceFilterPlugin = (builder: any) => {
	// Add spaceId/spaceIds/typeId/typeIds arguments and register data generators
	builder.hook("GraphQLObjectType:fields:field:args", (args: any, build: any, context: any) => {
		const {
			scope: {isPgFieldConnection, isPgFieldSimpleCollection, pgFieldIntrospection},
			addArgDataGenerator,
		} = context

		// Only modify entity connections/collections
		if (!isPgFieldConnection && !isPgFieldSimpleCollection) {
			return args
		}

		if (pgFieldIntrospection?.name !== "entities") {
			return args
		}

		const {pgSql: sql} = build
		const UUIDType = build.getTypeByName("UUID")
		const UUIDFilterType = build.getTypeByName("UUIDFilter")

		// ========================================================================
		// Space filter data generators
		// ========================================================================

		// Register the data generator for spaceId argument
		addArgDataGenerator(({spaceId}: {spaceId?: string}) => {
			if (!spaceId) return {}
			return {
				pgQuery: (queryBuilder: any) => {
					queryBuilder.where(buildSingleSpaceCondition(sql, queryBuilder.getTableAlias(), spaceId))
				},
			}
		})

		// Register the data generator for spaceIds argument
		addArgDataGenerator(({spaceIds}: {spaceIds?: any}) => {
			if (!spaceIds) return {}

			return {
				pgQuery: (queryBuilder: any) => {
					const tableAlias = queryBuilder.getTableAlias()

					if (spaceIds.is) {
						queryBuilder.where(buildSingleSpaceCondition(sql, tableAlias, spaceIds.is))
					}
					if (spaceIds.isNot) {
						queryBuilder.where(
							sql.fragment`NOT ${buildSingleSpaceCondition(sql, tableAlias, spaceIds.isNot)}`,
						)
					}
					if (spaceIds.in && spaceIds.in.length > 0) {
						queryBuilder.where(buildMultiSpaceCondition(sql, tableAlias, spaceIds.in))
					}
					if (spaceIds.notIn && spaceIds.notIn.length > 0) {
						queryBuilder.where(
							sql.fragment`NOT ${buildMultiSpaceCondition(sql, tableAlias, spaceIds.notIn)}`,
						)
					}
					if (spaceIds.isNull === true) {
						queryBuilder.where(sql.fragment`NOT ${buildHasAnySpaceCondition(sql, tableAlias)}`)
					}
					if (spaceIds.isNull === false) {
						queryBuilder.where(buildHasAnySpaceCondition(sql, tableAlias))
					}
				},
			}
		})

		// ========================================================================
		// Type filter data generators
		// ========================================================================

		// Register the data generator for typeId argument
		addArgDataGenerator(({typeId}: {typeId?: string}) => {
			if (!typeId) return {}
			return {
				pgQuery: (queryBuilder: any) => {
					queryBuilder.where(buildSingleTypeCondition(sql, queryBuilder.getTableAlias(), typeId))
				},
			}
		})

		// Register the data generator for typeIds argument
		addArgDataGenerator(({typeIds}: {typeIds?: any}) => {
			if (!typeIds) return {}

			return {
				pgQuery: (queryBuilder: any) => {
					const tableAlias = queryBuilder.getTableAlias()

					if (typeIds.is) {
						queryBuilder.where(buildSingleTypeCondition(sql, tableAlias, typeIds.is))
					}
					if (typeIds.isNot) {
						queryBuilder.where(
							sql.fragment`NOT ${buildSingleTypeCondition(sql, tableAlias, typeIds.isNot)}`,
						)
					}
					if (typeIds.in && typeIds.in.length > 0) {
						queryBuilder.where(buildMultiTypeCondition(sql, tableAlias, typeIds.in))
					}
					if (typeIds.notIn && typeIds.notIn.length > 0) {
						queryBuilder.where(sql.fragment`NOT ${buildMultiTypeCondition(sql, tableAlias, typeIds.notIn)}`)
					}
					// `overlaps`: semantically equivalent to `in` for type arrays
					// ("entity has at least one of these"). Custom-handled so we
					// don't fall back to the seq-scanning computed-column filter.
					if (typeIds.overlaps && typeIds.overlaps.length > 0) {
						queryBuilder.where(buildMultiTypeCondition(sql, tableAlias, typeIds.overlaps))
					}
					// `contains`: "entity has ALL of these types" — AND of per-type
					// EXISTS predicates, same indexed path as `is`.
					if (typeIds.contains && typeIds.contains.length > 0) {
						for (const t of typeIds.contains) {
							queryBuilder.where(buildSingleTypeCondition(sql, tableAlias, t))
						}
					}
					if (typeIds.isNull === true) {
						queryBuilder.where(sql.fragment`NOT ${buildHasAnyTypeCondition(sql, tableAlias)}`)
					}
					if (typeIds.isNull === false) {
						queryBuilder.where(buildHasAnyTypeCondition(sql, tableAlias))
					}
				},
			}
		})

		// ========================================================================
		// Add arguments to schema
		// ========================================================================

		return build.extend(args, {
			spaceId: {
				description: "Filter entities that have data in this space (efficient indexed lookup)",
				type: UUIDType,
			},
			spaceIds: {
				description: "Filter entities by space with operators (in, notIn, is, isNot, etc.)",
				type: UUIDFilterType,
			},
			typeId: {
				description: "Filter entities that have this type (efficient indexed lookup)",
				type: UUIDType,
			},
			typeIds: {
				description: "Filter entities by type with operators (in, notIn, is, isNot, etc.)",
				type: UUIDFilterType,
			},
		})
	})
}

export default EntitySpaceFilterPlugin
