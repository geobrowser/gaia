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
import {type NormalizedUuid, normalizeUuid} from "../utils/uuid"

/**
 * Entity discovered via context metadata or relation lookup.
 */
export interface DiscoveredEntity {
	entityId: NormalizedUuid
	contextEdgeTypeId: NormalizedUuid | null
	position: string | null
}

/**
 * Result of grouping entities by their context edge type.
 */
export interface GroupedEntities {
	/** Entities grouped under the static BLOCKS key */
	blocks: NormalizedUuid[]
	/** Entities grouped under dynamic keys (key = relation type ID) */
	dynamicGroups: Map<NormalizedUuid, NormalizedUuid[]>
	/** List of dynamic group keys present (for discoverability) */
	groupKeys: NormalizedUuid[]
}

/**
 * Group discovered entities by their context edge type.
 *
 * - Entities with contextEdgeTypeId = BLOCKS go into `blocks`
 * - Entities with other contextEdgeTypeId values go into `dynamicGroups`
 * - Entities with null contextEdgeTypeId (from relation fallback) go into `blocks`
 *   if discovered via BLOCKS relation lookup
 *
 * When the same entityId appears in both context discovery (non-null
 * contextEdgeTypeId) and BLOCKS-relation fallback (null contextEdgeTypeId),
 * the context entry wins the type bucket per RFC 0003, and position is
 * inherited from whichever entry has one. Diffs without context metadata
 * behave identically to pre-RFC behavior (pure BLOCKS fallback path).
 *
 * @param entities - Entities discovered via context or relation lookup
 * @param blocksTypeId - The relation type used for the static `blocks` bucket (default: BLOCKS)
 * @returns Grouped entities with static blocks and dynamic groups
 */
export function groupEntitiesByContext(
	entities: DiscoveredEntity[],
	blocksTypeId: NormalizedUuid = normalizeUuid(SystemIds.BLOCKS),
): Effect.Effect<GroupedEntities, never, never> {
	return Effect.sync(() => {
		const blocks: NormalizedUuid[] = []
		const dynamicGroups = new Map<NormalizedUuid, NormalizedUuid[]>()

		// Pass 1: dedupe. Context discovery wins over BLOCKS-relation fallback
		// so the RFC's edges[0].type_id grouping takes precedence. Position is
		// inherited from whichever entry has one.
		const deduped = new Map<NormalizedUuid, DiscoveredEntity>()
		for (const entity of entities) {
			const existing = deduped.get(entity.entityId)
			if (!existing) {
				deduped.set(entity.entityId, entity)
				continue
			}
			deduped.set(entity.entityId, {
				entityId: entity.entityId,
				contextEdgeTypeId: existing.contextEdgeTypeId ?? entity.contextEdgeTypeId,
				position: existing.position ?? entity.position,
			})
		}

		// Pass 2: sort by position (nulls last) so block ordering is stable.
		const sorted = Array.from(deduped.values()).sort((a, b) => {
			if (a.position === null && b.position === null) return 0
			if (a.position === null) return 1
			if (b.position === null) return -1
			return a.position.localeCompare(b.position)
		})

		for (const entity of sorted) {
			// null contextEdgeTypeId means discovery was via BLOCKS fallback only.
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
	relationEntities: Array<{entityId: NormalizedUuid; position: string | null}>,
	_relationTypeId: NormalizedUuid,
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
