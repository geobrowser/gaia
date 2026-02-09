/**
 * Pure grouping logic for context-aware entity discovery.
 *
 * Supports hybrid mode:
 * - Static `blocks` key for BLOCKS relation type
 * - Dynamic keys for other relation types
 * - `groupKeys` array for discoverability of dynamic keys
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {toUuid, type Uuid} from "../utils/uuid"

/**
 * Entity discovered via context metadata or relation lookup.
 */
export interface DiscoveredEntity {
	entityId: Uuid
	contextEdgeTypeId: Uuid | null
	position: string | null
}

/**
 * Result of grouping entities by their context edge type.
 */
export interface GroupedEntities {
	/** Entities grouped under the static BLOCKS key */
	blocks: Uuid[]
	/** Entities grouped under dynamic keys (key = relation type ID) */
	dynamicGroups: Map<Uuid, Uuid[]>
	/** List of dynamic group keys present (for discoverability) */
	groupKeys: Uuid[]
}

/**
 * Group discovered entities by their context edge type.
 *
 * - Entities with contextEdgeTypeId = BLOCKS go into `blocks`
 * - Entities with other contextEdgeTypeId values go into `dynamicGroups`
 * - Entities with null contextEdgeTypeId (from relation fallback) go into `blocks`
 *   if discovered via BLOCKS relation lookup
 *
 * @param entities - Entities discovered via context or relation lookup
 * @param fallbackTypeId - The relation type used for fallback discovery (e.g., BLOCKS)
 *                         Entities with null contextEdgeTypeId are assumed to be this type
 * @returns Grouped entities with static blocks and dynamic groups
 */
export function groupEntitiesByContext(
	entities: DiscoveredEntity[],
	blocksTypeId: Uuid = toUuid(SystemIds.BLOCKS),
): Effect.Effect<GroupedEntities, never, never> {
	return Effect.sync(() => {
		const blocks: Uuid[] = []
		const dynamicGroups = new Map<Uuid, Uuid[]>()
		const seen = new Set<Uuid>()

		// Sort by position (nulls last) to maintain ordering
		const sorted = [...entities].sort((a, b) => {
			if (a.position === null && b.position === null) return 0
			if (a.position === null) return 1
			if (b.position === null) return -1
			return a.position.localeCompare(b.position)
		})

		for (const entity of sorted) {
			// Skip duplicates
			if (seen.has(entity.entityId)) continue
			seen.add(entity.entityId)

			// Determine the effective type ID
			// null contextEdgeTypeId means it came from relation fallback
			const typeId = entity.contextEdgeTypeId ?? blocksTypeId

			if (typeId === blocksTypeId) {
				blocks.push(entity.entityId)
			} else {
				const group = dynamicGroups.get(typeId) ?? []
				group.push(entity.entityId)
				dynamicGroups.set(typeId, group)
			}
		}

		// Build groupKeys for discoverability
		const groupKeys = Array.from(dynamicGroups.keys()).sort()

		return {blocks, dynamicGroups, groupKeys}
	}).pipe(
		Effect.withSpan("grouping.groupEntitiesByContext", {
			attributes: {"grouping.entity_count": entities.length},
		}),
	)
}

/**
 * Merge entities from context-based discovery and relation-based fallback.
 *
 * Context-based entities have contextEdgeTypeId set.
 * Relation-based entities have contextEdgeTypeId = null and use the relation type.
 *
 * @param contextEntities - Entities discovered via context metadata
 * @param relationEntities - Entities discovered via relation lookup (fallback)
 * @param relationTypeId - The relation type used for fallback (e.g., BLOCKS)
 * @returns Merged list ready for grouping
 */
export function mergeDiscoveryResults(
	contextEntities: DiscoveredEntity[],
	relationEntities: Array<{entityId: Uuid; position: string | null}>,
	_relationTypeId: Uuid,
): Effect.Effect<DiscoveredEntity[], never, never> {
	return Effect.sync(() => {
		// Context entities already have contextEdgeTypeId
		const merged: DiscoveredEntity[] = [...contextEntities]

		// Relation entities get null contextEdgeTypeId (will use fallback)
		for (const rel of relationEntities) {
			merged.push({
				entityId: rel.entityId,
				contextEdgeTypeId: null,
				position: rel.position,
			})
		}

		return merged
	}).pipe(
		Effect.withSpan("grouping.mergeDiscoveryResults", {
			attributes: {
				"grouping.context_count": contextEntities.length,
				"grouping.relation_count": relationEntities.length,
			},
		}),
	)
}
