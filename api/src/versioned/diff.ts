/**
 * Diff computation for versioned entities.
 * Uses the `diff` package for word-level text diffs.
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {diffWords} from "diff"
import {Effect} from "effect"

import {type NormalizedUuid, normalizeUuid} from "../utils/uuid"

import type {
	BlockChange,
	BlockSnapshot,
	DiffChunk,
	DynamicGroupItem,
	EntityDiff,
	EntitySnapshot,
	GroupedEntityDiff,
	GroupedEntitySnapshot,
	RelationChange,
	ValueChange,
	VersionedRelation,
	VersionedValue,
} from "./types"

// Normalize SystemIds for comparison with NormalizedUuid fields
const TYPES_PROPERTY = normalizeUuid(SystemIds.TYPES_PROPERTY)
const TEXT_BLOCK = normalizeUuid(SystemIds.TEXT_BLOCK)
const IMAGE_BLOCK = normalizeUuid(SystemIds.IMAGE_BLOCK)
const IMAGE = normalizeUuid(SystemIds.IMAGE)
const DATA_BLOCK = normalizeUuid(SystemIds.DATA_BLOCK)
const MARKDOWN_CONTENT = normalizeUuid(SystemIds.MARKDOWN_CONTENT)
const IMAGE_URL_PROPERTY = normalizeUuid(SystemIds.IMAGE_URL_PROPERTY)
const NAME_PROPERTY = normalizeUuid(SystemIds.NAME_PROPERTY)

// ============================================================================
// Value Diffing
// ============================================================================

/**
 * Create a unique key for a value (property + space).
 */
function valueKey(v: VersionedValue): string {
	return `${v.propertyId}:${v.spaceId}`
}

/**
 * Extract the text value from a VersionedValue for text diffing.
 */
function getTextValue(v: VersionedValue): string {
	return v.text ?? ""
}

/**
 * Determine if a value is a text type (has text content).
 */
function isTextValue(v: VersionedValue): boolean {
	return v.text !== undefined && v.text !== null
}

/**
 * Serialize a non-text value to a string for comparison.
 */
function serializeValue(v: VersionedValue): string | null {
	if (v.boolean !== undefined && v.boolean !== null) return v.boolean.toString()
	if (v.integer !== undefined && v.integer !== null) return v.integer.toString()
	if (v.float !== undefined && v.float !== null) return v.float.toString()
	if (v.decimal !== undefined && v.decimal !== null) return v.decimal
	if (v.bytes !== undefined && v.bytes !== null) return v.bytes
	if (v.date !== undefined && v.date !== null) return v.date
	if (v.time !== undefined && v.time !== null) return v.time
	if (v.datetime !== undefined && v.datetime !== null) return v.datetime
	if (v.schedule !== undefined && v.schedule !== null) return JSON.stringify(v.schedule)
	if (v.point !== undefined && v.point !== null) return v.point
	if (v.rect !== undefined && v.rect !== null) return v.rect
	if (v.embedding !== undefined && v.embedding !== null) return JSON.stringify(v.embedding)
	return null
}

/**
 * Get the value type for a VersionedValue.
 * Returns GRC-20 v2 data type names.
 */
function getValueType(
	v: VersionedValue,
):
	| "TEXT"
	| "BOOL"
	| "INT64"
	| "FLOAT64"
	| "DECIMAL"
	| "BYTES"
	| "DATE"
	| "TIME"
	| "DATETIME"
	| "SCHEDULE"
	| "POINT"
	| "RECT"
	| "EMBEDDING" {
	if (v.text !== undefined && v.text !== null) return "TEXT"
	if (v.boolean !== undefined && v.boolean !== null) return "BOOL"
	if (v.integer !== undefined && v.integer !== null) return "INT64"
	if (v.float !== undefined && v.float !== null) return "FLOAT64"
	if (v.decimal !== undefined && v.decimal !== null) return "DECIMAL"
	if (v.bytes !== undefined && v.bytes !== null) return "BYTES"
	if (v.date !== undefined && v.date !== null) return "DATE"
	if (v.time !== undefined && v.time !== null) return "TIME"
	if (v.datetime !== undefined && v.datetime !== null) return "DATETIME"
	if (v.schedule !== undefined && v.schedule !== null) return "SCHEDULE"
	if (v.point !== undefined && v.point !== null) return "POINT"
	if (v.rect !== undefined && v.rect !== null) return "RECT"
	if (v.embedding !== undefined && v.embedding !== null) return "EMBEDDING"
	return "TEXT" // Default fallback
}

/**
 * Compute text diff using diffWords.
 */
function computeTextDiff(before: string, after: string): DiffChunk[] {
	const changes = diffWords(before, after)
	return changes.map((change) => ({
		value: change.value,
		...(change.added ? {added: true} : {}),
		...(change.removed ? {removed: true} : {}),
	}))
}

/**
 * Compute diff between two sets of values.
 */
export function diffValues(
	beforeValues: VersionedValue[],
	afterValues: VersionedValue[],
): Effect.Effect<ValueChange[], never, never> {
	return Effect.sync(() => {
		const beforeMap = new Map(beforeValues.map((v) => [valueKey(v), v]))
		const afterMap = new Map(afterValues.map((v) => [valueKey(v), v]))
		const changes: ValueChange[] = []

		// Find added and changed values
		for (const [key, afterValue] of afterMap) {
			const beforeValue = beforeMap.get(key)

			if (!beforeValue) {
				// Added value
				if (isTextValue(afterValue)) {
					const afterText = getTextValue(afterValue)
					changes.push({
						propertyId: afterValue.propertyId,
						spaceId: afterValue.spaceId,
						type: "TEXT",
						before: null,
						after: afterText,
						diff: computeTextDiff("", afterText),
					})
				} else {
					changes.push({
						propertyId: afterValue.propertyId,
						spaceId: afterValue.spaceId,
						type: getValueType(afterValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
						before: null,
						after: serializeValue(afterValue),
					})
				}
			} else {
				// Check if changed
				const beforeStr = isTextValue(beforeValue) ? getTextValue(beforeValue) : serializeValue(beforeValue)
				const afterStr = isTextValue(afterValue) ? getTextValue(afterValue) : serializeValue(afterValue)

				if (beforeStr !== afterStr) {
					if (isTextValue(afterValue) || isTextValue(beforeValue)) {
						changes.push({
							propertyId: afterValue.propertyId,
							spaceId: afterValue.spaceId,
							type: "TEXT",
							before: beforeStr,
							after: afterStr,
							diff: computeTextDiff(beforeStr ?? "", afterStr ?? ""),
						})
					} else {
						changes.push({
							propertyId: afterValue.propertyId,
							spaceId: afterValue.spaceId,
							type: getValueType(afterValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
							before: beforeStr,
							after: afterStr,
						})
					}
				}
			}
		}

		// Find removed values
		for (const [key, beforeValue] of beforeMap) {
			if (!afterMap.has(key)) {
				if (isTextValue(beforeValue)) {
					const beforeText = getTextValue(beforeValue)
					changes.push({
						propertyId: beforeValue.propertyId,
						spaceId: beforeValue.spaceId,
						type: "TEXT",
						before: beforeText,
						after: null,
						diff: computeTextDiff(beforeText, ""),
					})
				} else {
					changes.push({
						propertyId: beforeValue.propertyId,
						spaceId: beforeValue.spaceId,
						type: getValueType(beforeValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
						before: serializeValue(beforeValue),
						after: null,
					})
				}
			}
		}

		return changes
	}).pipe(
		Effect.withSpan("diff.diffValues", {
			attributes: {
				"diff.before_count": beforeValues.length,
				"diff.after_count": afterValues.length,
			},
		}),
	)
}

// ============================================================================
// Relation Diffing
// ============================================================================

/**
 * Compute diff between two sets of relations.
 */
export function diffRelations(
	beforeRelations: VersionedRelation[],
	afterRelations: VersionedRelation[],
): Effect.Effect<RelationChange[], never, never> {
	return Effect.sync(() => {
		const beforeMap = new Map(beforeRelations.map((r) => [r.relationId, r]))
		const afterMap = new Map(afterRelations.map((r) => [r.relationId, r]))
		const changes: RelationChange[] = []

		// Find added and changed relations
		for (const [id, afterRel] of afterMap) {
			const beforeRel = beforeMap.get(id)

			if (!beforeRel) {
				// Added
				changes.push({
					relationId: id,
					typeId: afterRel.typeId,
					spaceId: afterRel.spaceId,
					changeType: "ADD",
					before: null,
					after: {
						toEntityId: afterRel.toEntityId,
						toSpaceId: afterRel.toSpaceId,
						position: afterRel.position,
					},
				})
			} else {
				// Check if changed
				const hasChanged =
					beforeRel.toEntityId !== afterRel.toEntityId ||
					beforeRel.toSpaceId !== afterRel.toSpaceId ||
					beforeRel.position !== afterRel.position

				if (hasChanged) {
					changes.push({
						relationId: id,
						typeId: afterRel.typeId,
						spaceId: afterRel.spaceId,
						changeType: "UPDATE",
						before: {
							toEntityId: beforeRel.toEntityId,
							toSpaceId: beforeRel.toSpaceId,
							position: beforeRel.position,
						},
						after: {
							toEntityId: afterRel.toEntityId,
							toSpaceId: afterRel.toSpaceId,
							position: afterRel.position,
						},
					})
				}
			}
		}

		// Find removed relations
		for (const [id, beforeRel] of beforeMap) {
			if (!afterMap.has(id)) {
				changes.push({
					relationId: id,
					typeId: beforeRel.typeId,
					spaceId: beforeRel.spaceId,
					changeType: "REMOVE",
					before: {
						toEntityId: beforeRel.toEntityId,
						toSpaceId: beforeRel.toSpaceId,
						position: beforeRel.position,
					},
					after: null,
				})
			}
		}

		return changes
	}).pipe(
		Effect.withSpan("diff.diffRelations", {
			attributes: {
				"diff.before_count": beforeRelations.length,
				"diff.after_count": afterRelations.length,
			},
		}),
	)
}

// ============================================================================
// Block Diffing
// ============================================================================

/**
 * Determine block type from its relations.
 */
function getBlockType(block: BlockSnapshot): "textBlock" | "imageBlock" | "dataBlock" | null {
	// Check relations for type indicators
	for (const rel of block.relations) {
		if (rel.typeId === TYPES_PROPERTY) {
			if (rel.toEntityId === TEXT_BLOCK) return "textBlock"
			if (rel.toEntityId === IMAGE_BLOCK || rel.toEntityId === IMAGE) return "imageBlock"
			if (rel.toEntityId === DATA_BLOCK) return "dataBlock"
		}
	}
	return null
}

/**
 * Extract markdown content from a text block.
 */
function getMarkdownContent(block: BlockSnapshot): string {
	const markdownValue = block.values.find((v) => v.propertyId === MARKDOWN_CONTENT)
	return markdownValue?.text ?? ""
}

/**
 * Extract image URL from an image block.
 */
function getImageUrl(block: BlockSnapshot): string | null {
	const imageValue = block.values.find((v) => v.propertyId === IMAGE_URL_PROPERTY)
	return imageValue?.text ?? null
}

/**
 * Extract name from a data block.
 */
function getBlockName(block: BlockSnapshot): string | null {
	const nameValue = block.values.find((v) => v.propertyId === NAME_PROPERTY)
	return nameValue?.text ?? null
}

/**
 * Compute diff between two sets of blocks.
 */
export function diffBlocks(
	beforeBlocks: BlockSnapshot[],
	afterBlocks: BlockSnapshot[],
): Effect.Effect<BlockChange[], never, never> {
	return Effect.sync(() => {
		const beforeMap = new Map(beforeBlocks.map((b) => [b.id, b]))
		const afterMap = new Map(afterBlocks.map((b) => [b.id, b]))
		const changes: BlockChange[] = []

		// Find added and changed blocks
		for (const [id, afterBlock] of afterMap) {
			const beforeBlock = beforeMap.get(id)
			const blockType = getBlockType(afterBlock) ?? getBlockType(beforeBlock ?? afterBlock)

			if (!blockType) continue // Unknown block type

			if (!beforeBlock) {
				// Added block
				switch (blockType) {
					case "textBlock": {
						const afterContent = getMarkdownContent(afterBlock)
						changes.push({
							id,
							type: "textBlock",
							before: null,
							after: afterContent,
							diff: computeTextDiff("", afterContent),
						})
						break
					}
					case "imageBlock":
						changes.push({
							id,
							type: "imageBlock",
							before: null,
							after: getImageUrl(afterBlock),
						})
						break
					case "dataBlock":
						changes.push({
							id,
							type: "dataBlock",
							before: null,
							after: getBlockName(afterBlock),
						})
						break
				}
			} else {
				// Check if changed
				switch (blockType) {
					case "textBlock": {
						const beforeContent = getMarkdownContent(beforeBlock)
						const afterContent = getMarkdownContent(afterBlock)
						if (beforeContent !== afterContent) {
							changes.push({
								id,
								type: "textBlock",
								before: beforeContent,
								after: afterContent,
								diff: computeTextDiff(beforeContent, afterContent),
							})
						}
						break
					}
					case "imageBlock": {
						const beforeUrl = getImageUrl(beforeBlock)
						const afterUrl = getImageUrl(afterBlock)
						if (beforeUrl !== afterUrl) {
							changes.push({
								id,
								type: "imageBlock",
								before: beforeUrl,
								after: afterUrl,
							})
						}
						break
					}
					case "dataBlock": {
						const beforeName = getBlockName(beforeBlock)
						const afterName = getBlockName(afterBlock)
						if (beforeName !== afterName) {
							changes.push({
								id,
								type: "dataBlock",
								before: beforeName,
								after: afterName,
							})
						}
						break
					}
				}
			}
		}

		// Find removed blocks
		for (const [id, beforeBlock] of beforeMap) {
			if (!afterMap.has(id)) {
				const blockType = getBlockType(beforeBlock)
				if (!blockType) continue

				switch (blockType) {
					case "textBlock": {
						const beforeContent = getMarkdownContent(beforeBlock)
						changes.push({
							id,
							type: "textBlock",
							before: beforeContent,
							after: null,
							diff: computeTextDiff(beforeContent, ""),
						})
						break
					}
					case "imageBlock":
						changes.push({
							id,
							type: "imageBlock",
							before: getImageUrl(beforeBlock),
							after: null,
						})
						break
					case "dataBlock":
						changes.push({
							id,
							type: "dataBlock",
							before: getBlockName(beforeBlock),
							after: null,
						})
						break
				}
			}
		}

		return changes
	}).pipe(
		Effect.withSpan("diff.diffBlocks", {
			attributes: {
				"diff.before_count": beforeBlocks.length,
				"diff.after_count": afterBlocks.length,
			},
		}),
	)
}

// ============================================================================
// Dynamic Group Diffing
// ============================================================================

/**
 * Extract name from a block snapshot.
 */
function getSnapshotName(snapshot: BlockSnapshot): string | null {
	const nameValue = snapshot.values.find((v) => v.propertyId === NAME_PROPERTY)
	return nameValue?.text ?? null
}

/**
 * Create an empty BlockSnapshot for diffing against when entity is added/removed.
 */
function emptySnapshot(id: NormalizedUuid): BlockSnapshot {
	return {id, values: [], relations: []}
}

/**
 * Compute a full EntityDiff for a non-block entity.
 */
function diffEntitySnapshot(
	id: NormalizedUuid,
	before: BlockSnapshot,
	after: BlockSnapshot,
): Effect.Effect<EntityDiff, never, never> {
	return Effect.gen(function* () {
		const [values, relations] = yield* Effect.all([
			diffValues(before.values, after.values),
			diffRelations(before.relations, after.relations),
		])

		// BlockSnapshots don't have nested blocks, so blocks diff is empty
		const blocks: BlockChange[] = []

		return {
			entityId: id,
			name: getSnapshotName(after) ?? getSnapshotName(before),
			values,
			relations,
			blocks,
		}
	})
}

/**
 * Compute diff for a dynamic group.
 *
 * For known block types (textBlock, imageBlock, dataBlock), returns BlockChange.
 * For other entities, returns full EntityDiff with values/relations changes.
 */
export function diffDynamicGroup(
	beforeEntities: BlockSnapshot[],
	afterEntities: BlockSnapshot[],
): Effect.Effect<DynamicGroupItem[], never, never> {
	return Effect.gen(function* () {
		const beforeMap = new Map(beforeEntities.map((e) => [e.id, e]))
		const afterMap = new Map(afterEntities.map((e) => [e.id, e]))
		const changes: DynamicGroupItem[] = []

		// Find added and changed entities
		for (const [id, afterEntity] of afterMap) {
			const beforeEntity = beforeMap.get(id)
			const blockType = getBlockType(afterEntity) ?? (beforeEntity ? getBlockType(beforeEntity) : null)

			if (blockType) {
				// Handle as known block type
				if (!beforeEntity) {
					switch (blockType) {
						case "textBlock": {
							const afterContent = getMarkdownContent(afterEntity)
							changes.push({
								id,
								type: "textBlock",
								before: null,
								after: afterContent,
								diff: computeTextDiff("", afterContent),
							})
							break
						}
						case "imageBlock":
							changes.push({
								id,
								type: "imageBlock",
								before: null,
								after: getImageUrl(afterEntity),
							})
							break
						case "dataBlock":
							changes.push({
								id,
								type: "dataBlock",
								before: null,
								after: getBlockName(afterEntity),
							})
							break
					}
				} else {
					switch (blockType) {
						case "textBlock": {
							const beforeContent = getMarkdownContent(beforeEntity)
							const afterContent = getMarkdownContent(afterEntity)
							if (beforeContent !== afterContent) {
								changes.push({
									id,
									type: "textBlock",
									before: beforeContent,
									after: afterContent,
									diff: computeTextDiff(beforeContent, afterContent),
								})
							}
							break
						}
						case "imageBlock": {
							const beforeUrl = getImageUrl(beforeEntity)
							const afterUrl = getImageUrl(afterEntity)
							if (beforeUrl !== afterUrl) {
								changes.push({
									id,
									type: "imageBlock",
									before: beforeUrl,
									after: afterUrl,
								})
							}
							break
						}
						case "dataBlock": {
							const beforeName = getBlockName(beforeEntity)
							const afterName = getBlockName(afterEntity)
							if (beforeName !== afterName) {
								changes.push({
									id,
									type: "dataBlock",
									before: beforeName,
									after: afterName,
								})
							}
							break
						}
					}
				}
			} else {
				// Handle as generic entity - compute full EntityDiff
				const before = beforeEntity ?? emptySnapshot(id)
				const entityDiff = yield* diffEntitySnapshot(id, before, afterEntity)

				// Only include if there are actual changes
				if (entityDiff.values.length > 0 || entityDiff.relations.length > 0 || entityDiff.blocks.length > 0) {
					changes.push(entityDiff)
				}
			}
		}

		// Find removed entities
		for (const [id, beforeEntity] of beforeMap) {
			if (!afterMap.has(id)) {
				const blockType = getBlockType(beforeEntity)

				if (blockType) {
					switch (blockType) {
						case "textBlock": {
							const beforeContent = getMarkdownContent(beforeEntity)
							changes.push({
								id,
								type: "textBlock",
								before: beforeContent,
								after: null,
								diff: computeTextDiff(beforeContent, ""),
							})
							break
						}
						case "imageBlock":
							changes.push({
								id,
								type: "imageBlock",
								before: getImageUrl(beforeEntity),
								after: null,
							})
							break
						case "dataBlock":
							changes.push({
								id,
								type: "dataBlock",
								before: getBlockName(beforeEntity),
								after: null,
							})
							break
					}
				} else {
					// Removed entity - diff against empty snapshot
					const entityDiff = yield* diffEntitySnapshot(id, beforeEntity, emptySnapshot(id))

					// Removed entities always have changes (the removal itself)
					if (
						entityDiff.values.length > 0 ||
						entityDiff.relations.length > 0 ||
						entityDiff.blocks.length > 0
					) {
						changes.push(entityDiff)
					}
				}
			}
		}

		return changes
	}).pipe(
		Effect.withSpan("diff.diffDynamicGroup", {
			attributes: {
				"diff.before_count": beforeEntities.length,
				"diff.after_count": afterEntities.length,
			},
		}),
	)
}

// ============================================================================
// Entity Diffing
// ============================================================================

/**
 * Extract entity name from values.
 */
function getEntityName(snapshot: EntitySnapshot | GroupedEntitySnapshot): string | null {
	const nameValue = snapshot.values.find((v) => v.propertyId === NAME_PROPERTY)
	return nameValue?.text ?? null
}

/**
 * Compute a full entity diff between two snapshots.
 */
export function diffEntitySnapshots(
	entityId: NormalizedUuid,
	before: EntitySnapshot,
	after: EntitySnapshot,
): Effect.Effect<EntityDiff, never, never> {
	return Effect.gen(function* () {
		const [values, relations, blocks] = yield* Effect.all([
			diffValues(before.values, after.values),
			diffRelations(before.relations, after.relations),
			diffBlocks(before.blocks, after.blocks),
		])

		return {
			entityId,
			name: getEntityName(after) ?? getEntityName(before),
			values,
			relations,
			blocks,
		}
	}).pipe(
		Effect.withSpan("diff.diffEntitySnapshots", {
			attributes: {"diff.entity_id": entityId},
		}),
	)
}

/**
 * Compute a grouped entity diff between two snapshots.
 *
 * Returns hybrid mode response with:
 * - Static `blocks` array for BLOCKS relation type changes
 * - Dynamic `groups` map for other relation type changes
 * - `groupKeys` for discoverability (union of keys from both snapshots)
 */
export function diffGroupedEntitySnapshots(
	entityId: NormalizedUuid,
	before: GroupedEntitySnapshot,
	after: GroupedEntitySnapshot,
): Effect.Effect<GroupedEntityDiff, never, never> {
	return Effect.gen(function* () {
		// Compute base diffs
		const [values, relations, blocks] = yield* Effect.all([
			diffValues(before.values, after.values),
			diffRelations(before.relations, after.relations),
			diffBlocks(before.blocks, after.blocks),
		])

		// Compute dynamic group diffs
		// Collect all group keys from both snapshots
		const allGroupKeys = new Set([...before.groupKeys, ...after.groupKeys])
		const groups: Record<NormalizedUuid, DynamicGroupItem[]> = {}

		for (const key of allGroupKeys) {
			const beforeGroup = before.groups[key] ?? []
			const afterGroup = after.groups[key] ?? []
			const groupDiff = yield* diffDynamicGroup(beforeGroup, afterGroup)

			// Only include non-empty diffs
			if (groupDiff.length > 0) {
				groups[key] = groupDiff
			}
		}

		// groupKeys includes all keys that have changes
		const groupKeys = (Object.keys(groups) as NormalizedUuid[]).sort()

		return {
			entityId,
			name: getEntityName(after) ?? getEntityName(before),
			values,
			relations,
			blocks,
			groupKeys,
			groups,
		}
	}).pipe(
		Effect.withSpan("diff.diffGroupedEntitySnapshots", {
			attributes: {
				"diff.entity_id": entityId,
				"diff.group_count": before.groupKeys.length + after.groupKeys.length,
			},
		}),
	)
}
