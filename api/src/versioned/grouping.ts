/**
 * Pure grouping logic for context-aware entity discovery.
 *
 * Supports hybrid mode:
 * - Static `blocks` key for BLOCKS relation type
 * - Dynamic keys for other relation types
 * - `groupKeys` array for discoverability of dynamic keys
 */

import { SystemIds } from "@graphprotocol/grc-20";

/**
 * Entity discovered via context metadata or relation lookup.
 */
export interface DiscoveredEntity {
	entityId: string;
	contextEdgeTypeId: string | null;
	position: string | null;
}

/**
 * Result of grouping entities by their context edge type.
 */
export interface GroupedEntities {
	/** Entities grouped under the static BLOCKS key */
	blocks: string[];
	/** Entities grouped under dynamic keys (key = relation type ID) */
	dynamicGroups: Map<string, string[]>;
	/** List of dynamic group keys present (for discoverability) */
	groupKeys: string[];
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
	fallbackTypeId: string = SystemIds.BLOCKS
): GroupedEntities {
	const blocks: string[] = [];
	const dynamicGroups = new Map<string, string[]>();
	const seen = new Set<string>();

	// Sort by position (nulls last) to maintain ordering
	const sorted = [...entities].sort((a, b) => {
		if (a.position === null && b.position === null) return 0;
		if (a.position === null) return 1;
		if (b.position === null) return -1;
		return a.position.localeCompare(b.position);
	});

	for (const entity of sorted) {
		// Skip duplicates
		if (seen.has(entity.entityId)) continue;
		seen.add(entity.entityId);

		// Determine the effective type ID
		// null contextEdgeTypeId means it came from relation fallback
		const typeId = entity.contextEdgeTypeId ?? fallbackTypeId;

		if (typeId === SystemIds.BLOCKS) {
			blocks.push(entity.entityId);
		} else {
			const group = dynamicGroups.get(typeId) ?? [];
			group.push(entity.entityId);
			dynamicGroups.set(typeId, group);
		}
	}

	// Build groupKeys for discoverability
	const groupKeys = Array.from(dynamicGroups.keys()).sort();

	return { blocks, dynamicGroups, groupKeys };
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
	relationEntities: Array<{ entityId: string; position: string | null }>,
	relationTypeId: string
): DiscoveredEntity[] {
	// Context entities already have contextEdgeTypeId
	const merged: DiscoveredEntity[] = [...contextEntities];

	// Relation entities get null contextEdgeTypeId (will use fallback)
	for (const rel of relationEntities) {
		merged.push({
			entityId: rel.entityId,
			contextEdgeTypeId: null,
			position: rel.position,
		});
	}

	return merged;
}
