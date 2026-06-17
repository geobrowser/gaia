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
 * Diff behavior variant.
 *
 * - `v1` freezes the original `/versioned` behavior shipped to prod (geobrowser
 *   consumes it): on a dual-discovery collision the first entry in sorted order
 *   wins the bucket.
 * - `v2` applies the RFC 0003/0006 corrections (context discovery wins the
 *   bucket; persisted context leaf preferred when surfacing the changed child).
 *
 * Defaults to `v1` everywhere so any caller that doesn't opt in keeps prod
 * behavior.
 */
export type DiffVariant = "v1" | "v2"

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
 * Dual-discovery collision (same entityId via context AND BLOCKS fallback) is
 * resolved per `variant`:
 * - `v1` (prod): sort first, then first-in-sorted-order wins — a BLOCKS-fallback
 *   entry (real position) can sort ahead of its context entry (null position)
 *   and win, bucketing the entity under `blocks`.
 * - `v2` (RFC 0003): context discovery wins the type bucket, position inherited
 *   from whichever entry has one, then sort.
 *
 * Diffs without any dual-discovery collision behave identically under both.
 *
 * @param entities - Entities discovered via context or relation lookup
 * @param blocksTypeId - The relation type used for the static `blocks` bucket (default: BLOCKS)
 * @param variant - Diff behavior variant (default `v1` = frozen prod behavior)
 * @returns Grouped entities with static blocks and dynamic groups
 */
export function groupEntitiesByContext(
	entities: DiscoveredEntity[],
	blocksTypeId: NormalizedUuid = normalizeUuid(SystemIds.BLOCKS),
	variant: DiffVariant = "v1",
): Effect.Effect<GroupedEntities, never, never> {
	return Effect.sync(() => {
		const blocks: NormalizedUuid[] = []
		const dynamicGroups = new Map<NormalizedUuid, NormalizedUuid[]>()

		// Sort by position (nulls last). Use localeCompare to match geo-sdk's
		// canonical Position.compare (geo-sdk/src/core/position.ts), which the
		// frontend/editor sort with — diverging would make the diff's block order
		// disagree with the order the page renders for mixed-case positions.
		const byPosition = (a: DiscoveredEntity, b: DiscoveredEntity) => {
			if (a.position === null && b.position === null) return 0
			if (a.position === null) return 1
			if (b.position === null) return -1
			return a.position.localeCompare(b.position)
		}

		// Dedupe + order, preserving each variant's exact collision behavior.
		let sorted: DiscoveredEntity[]
		if (variant === "v2") {
			// Dedupe first: context discovery wins over BLOCKS-relation fallback so
			// the RFC's edges[0].type_id grouping takes precedence; position is
			// inherited from whichever entry has one. Then sort.
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
			sorted = Array.from(deduped.values()).sort(byPosition)
		} else {
			// v1 (frozen prod behavior): sort first, then keep the first occurrence
			// in sorted order.
			const seen = new Set<NormalizedUuid>()
			sorted = []
			for (const entity of [...entities].sort(byPosition)) {
				if (seen.has(entity.entityId)) continue
				seen.add(entity.entityId)
				sorted.push(entity)
			}
		}

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
			attributes: {"grouping.entity_count": entities.length, "grouping.variant": variant},
		}),
	)
}

/**
 * Merge entities from context-based discovery and relation-based fallback.
 *
 * Context-based entities already carry `contextEdgeTypeId`. Relation-based
 * entries are converted to `DiscoveredEntity` with `contextEdgeTypeId = null`,
 * which `groupEntitiesByContext` interprets as "use the fallback type ID
 * (its `blocksTypeId` argument)" when bucketing.
 *
 * The relation type itself is not stored on the merged entries — it's only
 * applied at grouping time, where the caller can decide which static bucket
 * the fallback maps to.
 *
 * @param contextEntities - Entities discovered via context metadata
 * @param relationEntities - Entities discovered via relation lookup (fallback)
 * @returns Merged list ready for grouping
 */
export function mergeDiscoveryResults(
	contextEntities: DiscoveredEntity[],
	relationEntities: Array<{entityId: NormalizedUuid; position: string | null}>,
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
