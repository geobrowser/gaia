/**
 * Diff computation for versioned entities.
 * Uses the `diff` package for word-level text diffs.
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {diffWords} from "diff"
import {Effect} from "effect"

import {type Uuid, toUuid} from "../utils/uuid"

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

// Normalize SystemIds for comparison with Uuid fields
const TYPES_PROPERTY = toUuid(SystemIds.TYPES_PROPERTY)
const TEXT_BLOCK = toUuid(SystemIds.TEXT_BLOCK)
const IMAGE_BLOCK = toUuid(SystemIds.IMAGE_BLOCK)
const IMAGE = toUuid(SystemIds.IMAGE)
const DATA_BLOCK = toUuid(SystemIds.DATA_BLOCK)
const MARKDOWN_CONTENT = toUuid(SystemIds.MARKDOWN_CONTENT)
const IMAGE_URL_PROPERTY = toUuid(SystemIds.IMAGE_URL_PROPERTY)
const NAME_PROPERTY = toUuid(SystemIds.NAME_PROPERTY)

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
	fromValues: VersionedValue[],
	toValues: VersionedValue[],
): Effect.Effect<ValueChange[], never, never> {
	return Effect.sync(() => {
		const fromMap = new Map(fromValues.map((v) => [valueKey(v), v]))
		const toMap = new Map(toValues.map((v) => [valueKey(v), v]))
		const changes: ValueChange[] = []

		// Find added and changed values
		for (const [key, toValue] of toMap) {
			const fromValue = fromMap.get(key)

			if (!fromValue) {
				// Added value
				if (isTextValue(toValue)) {
					const afterText = getTextValue(toValue)
					changes.push({
						propertyId: toValue.propertyId,
						spaceId: toValue.spaceId,
						type: "TEXT",
						before: null,
						after: afterText,
						diff: computeTextDiff("", afterText),
					})
				} else {
					changes.push({
						propertyId: toValue.propertyId,
						spaceId: toValue.spaceId,
						type: getValueType(toValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
						before: null,
						after: serializeValue(toValue),
					})
				}
			} else {
				// Check if changed
				const fromStr = isTextValue(fromValue) ? getTextValue(fromValue) : serializeValue(fromValue)
				const toStr = isTextValue(toValue) ? getTextValue(toValue) : serializeValue(toValue)

				if (fromStr !== toStr) {
					if (isTextValue(toValue) || isTextValue(fromValue)) {
						changes.push({
							propertyId: toValue.propertyId,
							spaceId: toValue.spaceId,
							type: "TEXT",
							before: fromStr,
							after: toStr,
							diff: computeTextDiff(fromStr ?? "", toStr ?? ""),
						})
					} else {
						changes.push({
							propertyId: toValue.propertyId,
							spaceId: toValue.spaceId,
							type: getValueType(toValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
							before: fromStr,
							after: toStr,
						})
					}
				}
			}
		}

		// Find removed values
		for (const [key, fromValue] of fromMap) {
			if (!toMap.has(key)) {
				if (isTextValue(fromValue)) {
					const beforeText = getTextValue(fromValue)
					changes.push({
						propertyId: fromValue.propertyId,
						spaceId: fromValue.spaceId,
						type: "TEXT",
						before: beforeText,
						after: null,
						diff: computeTextDiff(beforeText, ""),
					})
				} else {
					changes.push({
						propertyId: fromValue.propertyId,
						spaceId: fromValue.spaceId,
						type: getValueType(fromValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
						before: serializeValue(fromValue),
						after: null,
					})
				}
			}
		}

		return changes
	}).pipe(
		Effect.withSpan("diff.diffValues", {
			attributes: {
				"diff.from_count": fromValues.length,
				"diff.to_count": toValues.length,
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
	fromRelations: VersionedRelation[],
	toRelations: VersionedRelation[],
): Effect.Effect<RelationChange[], never, never> {
	return Effect.sync(() => {
		const fromMap = new Map(fromRelations.map((r) => [r.relationId, r]))
		const toMap = new Map(toRelations.map((r) => [r.relationId, r]))
		const changes: RelationChange[] = []

		// Find added and changed relations
		for (const [id, toRel] of toMap) {
			const fromRel = fromMap.get(id)

			if (!fromRel) {
				// Added
				changes.push({
					relationId: id,
					typeId: toRel.typeId,
					spaceId: toRel.spaceId,
					changeType: "ADD",
					before: null,
					after: {
						toEntityId: toRel.toEntityId,
						toSpaceId: toRel.toSpaceId,
						position: toRel.position,
					},
				})
			} else {
				// Check if changed
				const hasChanged =
					fromRel.toEntityId !== toRel.toEntityId ||
					fromRel.toSpaceId !== toRel.toSpaceId ||
					fromRel.position !== toRel.position

				if (hasChanged) {
					changes.push({
						relationId: id,
						typeId: toRel.typeId,
						spaceId: toRel.spaceId,
						changeType: "UPDATE",
						before: {
							toEntityId: fromRel.toEntityId,
							toSpaceId: fromRel.toSpaceId,
							position: fromRel.position,
						},
						after: {
							toEntityId: toRel.toEntityId,
							toSpaceId: toRel.toSpaceId,
							position: toRel.position,
						},
					})
				}
			}
		}

		// Find removed relations
		for (const [id, fromRel] of fromMap) {
			if (!toMap.has(id)) {
				changes.push({
					relationId: id,
					typeId: fromRel.typeId,
					spaceId: fromRel.spaceId,
					changeType: "REMOVE",
					before: {
						toEntityId: fromRel.toEntityId,
						toSpaceId: fromRel.toSpaceId,
						position: fromRel.position,
					},
					after: null,
				})
			}
		}

		return changes
	}).pipe(
		Effect.withSpan("diff.diffRelations", {
			attributes: {
				"diff.from_count": fromRelations.length,
				"diff.to_count": toRelations.length,
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
	fromBlocks: BlockSnapshot[],
	toBlocks: BlockSnapshot[],
): Effect.Effect<BlockChange[], never, never> {
	return Effect.sync(() => {
		const fromMap = new Map(fromBlocks.map((b) => [b.id, b]))
		const toMap = new Map(toBlocks.map((b) => [b.id, b]))
		const changes: BlockChange[] = []

		// Find added and changed blocks
		for (const [id, toBlock] of toMap) {
			const fromBlock = fromMap.get(id)
			const blockType = getBlockType(toBlock) ?? getBlockType(fromBlock ?? toBlock)

			if (!blockType) continue // Unknown block type

			if (!fromBlock) {
				// Added block
				switch (blockType) {
					case "textBlock": {
						const afterContent = getMarkdownContent(toBlock)
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
							after: getImageUrl(toBlock),
						})
						break
					case "dataBlock":
						changes.push({
							id,
							type: "dataBlock",
							before: null,
							after: getBlockName(toBlock),
						})
						break
				}
			} else {
				// Check if changed
				switch (blockType) {
					case "textBlock": {
						const fromContent = getMarkdownContent(fromBlock)
						const toContent = getMarkdownContent(toBlock)
						if (fromContent !== toContent) {
							changes.push({
								id,
								type: "textBlock",
								before: fromContent,
								after: toContent,
								diff: computeTextDiff(fromContent, toContent),
							})
						}
						break
					}
					case "imageBlock": {
						const fromUrl = getImageUrl(fromBlock)
						const toUrl = getImageUrl(toBlock)
						if (fromUrl !== toUrl) {
							changes.push({
								id,
								type: "imageBlock",
								before: fromUrl,
								after: toUrl,
							})
						}
						break
					}
					case "dataBlock": {
						const fromName = getBlockName(fromBlock)
						const toName = getBlockName(toBlock)
						if (fromName !== toName) {
							changes.push({
								id,
								type: "dataBlock",
								before: fromName,
								after: toName,
							})
						}
						break
					}
				}
			}
		}

		// Find removed blocks
		for (const [id, fromBlock] of fromMap) {
			if (!toMap.has(id)) {
				const blockType = getBlockType(fromBlock)
				if (!blockType) continue

				switch (blockType) {
					case "textBlock": {
						const beforeContent = getMarkdownContent(fromBlock)
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
							before: getImageUrl(fromBlock),
							after: null,
						})
						break
					case "dataBlock":
						changes.push({
							id,
							type: "dataBlock",
							before: getBlockName(fromBlock),
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
				"diff.from_count": fromBlocks.length,
				"diff.to_count": toBlocks.length,
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
function emptySnapshot(id: Uuid): BlockSnapshot {
	return {id, values: [], relations: []}
}

/**
 * Compute a full EntityDiff for a non-block entity.
 */
function diffEntitySnapshot(
	id: Uuid,
	from: BlockSnapshot,
	to: BlockSnapshot,
): Effect.Effect<EntityDiff, never, never> {
	return Effect.gen(function* () {
		const [values, relations] = yield* Effect.all([
			diffValues(from.values, to.values),
			diffRelations(from.relations, to.relations),
		])

		// BlockSnapshots don't have nested blocks, so blocks diff is empty
		const blocks: BlockChange[] = []

		return {
			entityId: id,
			name: getSnapshotName(to) ?? getSnapshotName(from),
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
	fromEntities: BlockSnapshot[],
	toEntities: BlockSnapshot[],
): Effect.Effect<DynamicGroupItem[], never, never> {
	return Effect.gen(function* () {
		const fromMap = new Map(fromEntities.map((e) => [e.id, e]))
		const toMap = new Map(toEntities.map((e) => [e.id, e]))
		const changes: DynamicGroupItem[] = []

		// Find added and changed entities
		for (const [id, toEntity] of toMap) {
			const fromEntity = fromMap.get(id)
			const blockType = getBlockType(toEntity) ?? (fromEntity ? getBlockType(fromEntity) : null)

			if (blockType) {
				// Handle as known block type
				if (!fromEntity) {
					switch (blockType) {
						case "textBlock": {
							const afterContent = getMarkdownContent(toEntity)
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
								after: getImageUrl(toEntity),
							})
							break
						case "dataBlock":
							changes.push({
								id,
								type: "dataBlock",
								before: null,
								after: getBlockName(toEntity),
							})
							break
					}
				} else {
					switch (blockType) {
						case "textBlock": {
							const fromContent = getMarkdownContent(fromEntity)
							const toContent = getMarkdownContent(toEntity)
							if (fromContent !== toContent) {
								changes.push({
									id,
									type: "textBlock",
									before: fromContent,
									after: toContent,
									diff: computeTextDiff(fromContent, toContent),
								})
							}
							break
						}
						case "imageBlock": {
							const fromUrl = getImageUrl(fromEntity)
							const toUrl = getImageUrl(toEntity)
							if (fromUrl !== toUrl) {
								changes.push({
									id,
									type: "imageBlock",
									before: fromUrl,
									after: toUrl,
								})
							}
							break
						}
						case "dataBlock": {
							const fromName = getBlockName(fromEntity)
							const toName = getBlockName(toEntity)
							if (fromName !== toName) {
								changes.push({
									id,
									type: "dataBlock",
									before: fromName,
									after: toName,
								})
							}
							break
						}
					}
				}
			} else {
				// Handle as generic entity - compute full EntityDiff
				const from = fromEntity ?? emptySnapshot(id)
				const entityDiff = yield* diffEntitySnapshot(id, from, toEntity)

				// Only include if there are actual changes
				if (entityDiff.values.length > 0 || entityDiff.relations.length > 0 || entityDiff.blocks.length > 0) {
					changes.push(entityDiff)
				}
			}
		}

		// Find removed entities
		for (const [id, fromEntity] of fromMap) {
			if (!toMap.has(id)) {
				const blockType = getBlockType(fromEntity)

				if (blockType) {
					switch (blockType) {
						case "textBlock": {
							const beforeContent = getMarkdownContent(fromEntity)
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
								before: getImageUrl(fromEntity),
								after: null,
							})
							break
						case "dataBlock":
							changes.push({
								id,
								type: "dataBlock",
								before: getBlockName(fromEntity),
								after: null,
							})
							break
					}
				} else {
					// Removed entity - diff against empty snapshot
					const entityDiff = yield* diffEntitySnapshot(id, fromEntity, emptySnapshot(id))

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
				"diff.from_count": fromEntities.length,
				"diff.to_count": toEntities.length,
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
	entityId: Uuid,
	from: EntitySnapshot,
	to: EntitySnapshot,
): Effect.Effect<EntityDiff, never, never> {
	return Effect.gen(function* () {
		const [values, relations, blocks] = yield* Effect.all([
			diffValues(from.values, to.values),
			diffRelations(from.relations, to.relations),
			diffBlocks(from.blocks, to.blocks),
		])

		return {
			entityId,
			name: getEntityName(to) ?? getEntityName(from),
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
	entityId: Uuid,
	from: GroupedEntitySnapshot,
	to: GroupedEntitySnapshot,
): Effect.Effect<GroupedEntityDiff, never, never> {
	return Effect.gen(function* () {
		// Compute base diffs
		const [values, relations, blocks] = yield* Effect.all([
			diffValues(from.values, to.values),
			diffRelations(from.relations, to.relations),
			diffBlocks(from.blocks, to.blocks),
		])

		// Compute dynamic group diffs
		// Collect all group keys from both snapshots
		const allGroupKeys = new Set([...from.groupKeys, ...to.groupKeys])
		const groups: Record<Uuid, DynamicGroupItem[]> = {}

		for (const key of allGroupKeys) {
			const fromGroup = from.groups[key] ?? []
			const toGroup = to.groups[key] ?? []
			const groupDiff = yield* diffDynamicGroup(fromGroup, toGroup)

			// Only include non-empty diffs
			if (groupDiff.length > 0) {
				groups[key] = groupDiff
			}
		}

		// groupKeys includes all keys that have changes
		const groupKeys = (Object.keys(groups) as Uuid[]).sort()

		return {
			entityId,
			name: getEntityName(to) ?? getEntityName(from),
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
				"diff.group_count": from.groupKeys.length + to.groupKeys.length,
			},
		}),
	)
}
