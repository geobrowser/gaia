/**
 * Custom PostGraphile plugin that adds efficient space and type filtering to the entities connection.
 *
 * Instead of using computed functions (which run queries per entity),
 * this uses EXISTS subqueries with indexed lookups.
 *
 * Space filtering (positive position) uses a semi-join:
 *     WHERE e.id IN (SELECT v.entity_id   FROM values    v WHERE v.space_id = 'space-uuid'
 *                    UNION
 *                    SELECT r.from_entity_id FROM relations r WHERE r.space_id = 'space-uuid')
 *
 *   An `EXISTS OR EXISTS` pair cannot be planned as a semi-join, which made a sparse
 *   space take 22.5 s per request (GEO-2882). Negated space filters keep the EXISTS
 *   form, which measures faster there. See buildSingleSpaceSemiJoin for the numbers.
 *
 * Type filtering:
 *   Before (slow - O(n) function calls):
 *     WHERE entities_type_ids(e) @> array['type-uuid']
 *   After (fast - O(1) indexed EXISTS):
 *     WHERE EXISTS (SELECT 1 FROM relations WHERE from_entity_id = e.id
 *                   AND type_id = 'SystemIds.Types' AND to_entity_id = 'type-uuid' LIMIT 1)
 *
 * System-type filtering mirrors type filtering but matches the System Type
 * relation (system-minted, non-user-editable) instead of regular Type
 * relations. It is kept separate from typeId/typeIds so consumers can reliably
 * identify system entities — see entities.systemTypeIds.
 *
 * Usage in GraphQL:
 *   entities(spaceId: "uuid", first: 100) { ... }              # single space
 *   entities(spaceIds: { in: ["uuid1", "uuid2"] }, first: 100) { ... } # multiple spaces (OR)
 *   entities(typeId: "uuid", first: 100) { ... }               # single type
 *   entities(typeIds: { in: ["uuid1", "uuid2"] }, first: 100) { ... }  # multiple types (OR)
 *   entities(systemTypeId: "uuid", first: 100) { ... }         # single system type
 *   entities(systemTypeIds: { in: ["uuid1", "uuid2"] }, first: 100) { ... }  # multiple system types (OR)
 *
 * Requires indexes (already exist):
 *   - values(entity_id, space_id)
 *   - relations(from_entity_id, space_id)
 *   - relations(from_entity_id, type_id, to_entity_id)
 */

import {SystemIds} from "@geoprotocol/geo-sdk"
import {SYSTEM_TYPE_RELATION_TYPE_ID} from "./systemTypeIds"

const SYSTEM_IDS_TYPES = SystemIds.TYPES_PROPERTY

// System Type relation (system-minted, non-user-editable: the indexer always
// drops user attempts to author it). Kept separate from regular Type relations
// so systemTypeIds reliably identifies system entities — a user can author a
// Type relation pointing at a system-type entity, but cannot forge a System
// Type relation. See ./systemTypeIds for the shared constant.
const SYSTEM_TYPE = SYSTEM_TYPE_RELATION_TYPE_ID

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

// Positive-position space filter: a real semi-join.
//
// `EXISTS(...) OR EXISTS(...)` cannot be transformed into a semi-join, so Postgres
// falls back to hashed SubPlans and walks `entities_pkey` row by row until the LIMIT
// is satisfied. The counter-intuitive part is that the pathological case is the
// SPARSE space, not the large one: a 9-entity space made the scan cross ~half the
// table (24.6M rows removed, 39.3M buffers, 22,527 ms) on every debates page load,
// and `first: 5` timed out just as hard as a big page — it is not result size.
//
// `id IN (SELECT ... UNION SELECT ...)` is a real semi-join; the Merge Append streams
// sorted index-only scans so the LIMIT propagates down. Measured with
// EXPLAIN (ANALYZE, BUFFERS) against the live testnet DB for GEO-2882:
//
//   sparse space   (9 entities)       22,527 ms / 39.3M buffers -> 0.124 ms / 57 buffers
//   large space    (60,916 entities)     409 ms /  355K buffers -> 0.130 ms / 61 buffers
//   multi-space ANY(), sparse         22,766 ms                 -> 0.201 ms
//
// No regression on large spaces — it is faster there too, because the current plan
// materialises the whole hashed SubPlan before emitting a first row.
// `values_space_id_idx` / `relations_space_from_to_idx` already exist.
//
// ONLY valid where the condition appears POSITIVELY. Negation measured worse
// (`NOT (id IN (union))` 506 ms vs 416 ms for `NOT (EXISTS OR EXISTS)` on the large
// space), so the isNot / notIn / isNull:true paths keep the EXISTS helpers below.
// `values.entity_id` and `relations.from_entity_id` are both NOT NULL, so there is
// no NOT IN null trap either way.
const buildSingleSpaceSemiJoin = (sql: any, tableAlias: any, spaceId: string) => {
	return sql.fragment`${tableAlias}.id IN (
		SELECT v.entity_id FROM public.values v
		WHERE v.space_id = ${sql.value(spaceId)}::uuid
		UNION
		SELECT r.from_entity_id FROM public.relations r
		WHERE r.space_id = ${sql.value(spaceId)}::uuid
	)`
}

const buildMultiSpaceSemiJoin = (sql: any, tableAlias: any, spaceIds: string[]) => {
	return sql.fragment`${tableAlias}.id IN (
		SELECT v.entity_id FROM public.values v
		WHERE v.space_id = ANY(${sql.value(spaceIds)}::uuid[])
		UNION
		SELECT r.from_entity_id FROM public.relations r
		WHERE r.space_id = ANY(${sql.value(spaceIds)}::uuid[])
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
// System type filter helpers
// ============================================================================

// Helper to build the EXISTS condition for a single system type ID
const buildSingleSystemTypeCondition = (sql: any, tableAlias: any, systemTypeId: string) => {
	return sql.fragment`EXISTS (
		SELECT 1 FROM public.relations r
		WHERE r.from_entity_id = ${tableAlias}.id
		AND r.type_id = ${sql.value(SYSTEM_TYPE)}::uuid
		AND r.to_entity_id = ${sql.value(systemTypeId)}::uuid
		LIMIT 1
	)`
}

// Helper to build the EXISTS condition for multiple system type IDs
const buildMultiSystemTypeCondition = (sql: any, tableAlias: any, systemTypeIds: string[]) => {
	return sql.fragment`EXISTS (
		SELECT 1 FROM public.relations r
		WHERE r.from_entity_id = ${tableAlias}.id
		AND r.type_id = ${sql.value(SYSTEM_TYPE)}::uuid
		AND r.to_entity_id = ANY(${sql.value(systemTypeIds)}::uuid[])
		LIMIT 1
	)`
}

// Helper to build condition for checking if entity has any system type
const buildHasAnySystemTypeCondition = (sql: any, tableAlias: any) => {
	return sql.fragment`EXISTS (
		SELECT 1 FROM public.relations r
		WHERE r.from_entity_id = ${tableAlias}.id
		AND r.type_id = ${sql.value(SYSTEM_TYPE)}::uuid
		LIMIT 1
	)`
}

// ============================================================================
// Plugin
// ============================================================================

export const EntitySpaceFilterPlugin = (builder: any) => {
	// Add spaceId(s)/typeId(s)/systemTypeId(s) arguments and register data generators
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
					queryBuilder.where(buildSingleSpaceSemiJoin(sql, queryBuilder.getTableAlias(), spaceId))
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
						queryBuilder.where(buildSingleSpaceSemiJoin(sql, tableAlias, spaceIds.is))
					}
					if (spaceIds.isNot) {
						queryBuilder.where(
							sql.fragment`NOT ${buildSingleSpaceCondition(sql, tableAlias, spaceIds.isNot)}`,
						)
					}
					if (spaceIds.in && spaceIds.in.length > 0) {
						queryBuilder.where(buildMultiSpaceSemiJoin(sql, tableAlias, spaceIds.in))
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
		// System type filter data generators
		// ========================================================================

		// Register the data generator for systemTypeId argument
		addArgDataGenerator(({systemTypeId}: {systemTypeId?: string}) => {
			if (!systemTypeId) return {}
			return {
				pgQuery: (queryBuilder: any) => {
					queryBuilder.where(buildSingleSystemTypeCondition(sql, queryBuilder.getTableAlias(), systemTypeId))
				},
			}
		})

		// Register the data generator for systemTypeIds argument
		addArgDataGenerator(({systemTypeIds}: {systemTypeIds?: any}) => {
			if (!systemTypeIds) return {}

			return {
				pgQuery: (queryBuilder: any) => {
					const tableAlias = queryBuilder.getTableAlias()

					if (systemTypeIds.is) {
						queryBuilder.where(buildSingleSystemTypeCondition(sql, tableAlias, systemTypeIds.is))
					}
					if (systemTypeIds.isNot) {
						queryBuilder.where(
							sql.fragment`NOT ${buildSingleSystemTypeCondition(sql, tableAlias, systemTypeIds.isNot)}`,
						)
					}
					if (systemTypeIds.in && systemTypeIds.in.length > 0) {
						queryBuilder.where(buildMultiSystemTypeCondition(sql, tableAlias, systemTypeIds.in))
					}
					if (systemTypeIds.notIn && systemTypeIds.notIn.length > 0) {
						queryBuilder.where(
							sql.fragment`NOT ${buildMultiSystemTypeCondition(sql, tableAlias, systemTypeIds.notIn)}`,
						)
					}
					if (systemTypeIds.isNull === true) {
						queryBuilder.where(sql.fragment`NOT ${buildHasAnySystemTypeCondition(sql, tableAlias)}`)
					}
					if (systemTypeIds.isNull === false) {
						queryBuilder.where(buildHasAnySystemTypeCondition(sql, tableAlias))
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
			systemTypeId: {
				description:
					"Filter entities that have this system type (efficient indexed lookup). System types are system-managed and non-user-editable.",
				type: UUIDType,
			},
			systemTypeIds: {
				description:
					"Filter entities by system type with operators (in, notIn, is, isNot, etc.). System types are system-managed and non-user-editable.",
				type: UUIDFilterType,
			},
		})
	})
}

export default EntitySpaceFilterPlugin
