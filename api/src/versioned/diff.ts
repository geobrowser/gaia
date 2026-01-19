/**
 * Diff computation for versioned entities.
 * Uses the `diff` package for word-level text diffs.
 */

import { diffWords } from "diff";
import { SystemIds } from "@graphprotocol/grc-20";

import type {
	DiffChunk,
	VersionedValue,
	VersionedRelation,
	BlockSnapshot,
	EntitySnapshot,
	ValueChange,
	RelationChange,
	BlockChange,
	EntityDiff,
} from "./types";

// ============================================================================
// Value Diffing
// ============================================================================

/**
 * Create a unique key for a value (property + space).
 */
function valueKey(v: VersionedValue): string {
	return `${v.propertyId}:${v.spaceId}`;
}

/**
 * Extract the string value from a VersionedValue for text diffing.
 */
function getStringValue(v: VersionedValue): string {
	return v.string ?? "";
}

/**
 * Determine if a value is a text type (has string content).
 */
function isTextValue(v: VersionedValue): boolean {
	return v.string !== undefined && v.string !== null;
}

/**
 * Serialize a non-text value to a string for comparison.
 */
function serializeValue(v: VersionedValue): string | null {
	if (v.boolean !== undefined && v.boolean !== null)
		return v.boolean.toString();
	if (v.number !== undefined && v.number !== null) return v.number;
	if (v.integer !== undefined && v.integer !== null)
		return v.integer.toString();
	if (v.float !== undefined && v.float !== null) return v.float.toString();
	if (v.time !== undefined && v.time !== null) return v.time;
	if (v.point !== undefined && v.point !== null) return v.point;
	if (v.date !== undefined && v.date !== null) return v.date;
	if (v.datetime !== undefined && v.datetime !== null) return v.datetime;
	if (v.bytes !== undefined && v.bytes !== null) return v.bytes;
	return null;
}

/**
 * Get the value type for a VersionedValue.
 */
function getValueType(
	v: VersionedValue
): "TEXT" | "NUMBER" | "BOOLEAN" | "TIME" | "POINT" | "DATE" | "DATETIME" | "BYTES" {
	if (v.string !== undefined && v.string !== null) return "TEXT";
	if (v.boolean !== undefined && v.boolean !== null) return "BOOLEAN";
	if (
		v.number !== undefined ||
		v.integer !== undefined ||
		v.float !== undefined
	)
		return "NUMBER";
	if (v.time !== undefined && v.time !== null) return "TIME";
	if (v.point !== undefined && v.point !== null) return "POINT";
	if (v.date !== undefined && v.date !== null) return "DATE";
	if (v.datetime !== undefined && v.datetime !== null) return "DATETIME";
	if (v.bytes !== undefined && v.bytes !== null) return "BYTES";
	return "TEXT"; // Default fallback
}

/**
 * Compute text diff using diffWords.
 */
function computeTextDiff(from: string, to: string): DiffChunk[] {
	const changes = diffWords(from, to);
	return changes.map((change) => ({
		value: change.value,
		...(change.added ? { added: true } : {}),
		...(change.removed ? { removed: true } : {}),
	}));
}

/**
 * Compute diff between two sets of values.
 */
export function diffValues(
	fromValues: VersionedValue[],
	toValues: VersionedValue[]
): ValueChange[] {
	const fromMap = new Map(fromValues.map((v) => [valueKey(v), v]));
	const toMap = new Map(toValues.map((v) => [valueKey(v), v]));
	const changes: ValueChange[] = [];

	// Find added and changed values
	for (const [key, toValue] of toMap) {
		const fromValue = fromMap.get(key);

		if (!fromValue) {
			// Added value
			if (isTextValue(toValue)) {
				changes.push({
					propertyId: toValue.propertyId,
					spaceId: toValue.spaceId,
					type: "TEXT",
					diff: computeTextDiff("", getStringValue(toValue)),
				});
			} else {
				changes.push({
					propertyId: toValue.propertyId,
					spaceId: toValue.spaceId,
					type: getValueType(toValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
					from: null,
					to: serializeValue(toValue),
				});
			}
		} else {
			// Check if changed
			const fromStr = isTextValue(fromValue)
				? getStringValue(fromValue)
				: serializeValue(fromValue);
			const toStr = isTextValue(toValue)
				? getStringValue(toValue)
				: serializeValue(toValue);

			if (fromStr !== toStr) {
				if (isTextValue(toValue) || isTextValue(fromValue)) {
					changes.push({
						propertyId: toValue.propertyId,
						spaceId: toValue.spaceId,
						type: "TEXT",
						diff: computeTextDiff(fromStr ?? "", toStr ?? ""),
					});
				} else {
					changes.push({
						propertyId: toValue.propertyId,
						spaceId: toValue.spaceId,
						type: getValueType(toValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
						from: fromStr,
						to: toStr,
					});
				}
			}
		}
	}

	// Find removed values
	for (const [key, fromValue] of fromMap) {
		if (!toMap.has(key)) {
			if (isTextValue(fromValue)) {
				changes.push({
					propertyId: fromValue.propertyId,
					spaceId: fromValue.spaceId,
					type: "TEXT",
					diff: computeTextDiff(getStringValue(fromValue), ""),
				});
			} else {
				changes.push({
					propertyId: fromValue.propertyId,
					spaceId: fromValue.spaceId,
					type: getValueType(fromValue) as Exclude<ReturnType<typeof getValueType>, "TEXT">,
					from: serializeValue(fromValue),
					to: null,
				});
			}
		}
	}

	return changes;
}

// ============================================================================
// Relation Diffing
// ============================================================================

/**
 * Compute diff between two sets of relations.
 */
export function diffRelations(
	fromRelations: VersionedRelation[],
	toRelations: VersionedRelation[]
): RelationChange[] {
	const fromMap = new Map(fromRelations.map((r) => [r.relationId, r]));
	const toMap = new Map(toRelations.map((r) => [r.relationId, r]));
	const changes: RelationChange[] = [];

	// Find added and changed relations
	for (const [id, toRel] of toMap) {
		const fromRel = fromMap.get(id);

		if (!fromRel) {
			// Added
			changes.push({
				relationId: id,
				typeId: toRel.typeId,
				spaceId: toRel.spaceId,
				changeType: "ADD",
				from: null,
				to: {
					toEntityId: toRel.toEntityId,
					toSpaceId: toRel.toSpaceId,
					position: toRel.position,
				},
			});
		} else {
			// Check if changed
			const hasChanged =
				fromRel.toEntityId !== toRel.toEntityId ||
				fromRel.toSpaceId !== toRel.toSpaceId ||
				fromRel.position !== toRel.position;

			if (hasChanged) {
				changes.push({
					relationId: id,
					typeId: toRel.typeId,
					spaceId: toRel.spaceId,
					changeType: "UPDATE",
					from: {
						toEntityId: fromRel.toEntityId,
						toSpaceId: fromRel.toSpaceId,
						position: fromRel.position,
					},
					to: {
						toEntityId: toRel.toEntityId,
						toSpaceId: toRel.toSpaceId,
						position: toRel.position,
					},
				});
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
				from: {
					toEntityId: fromRel.toEntityId,
					toSpaceId: fromRel.toSpaceId,
					position: fromRel.position,
				},
				to: null,
			});
		}
	}

	return changes;
}

// ============================================================================
// Block Diffing
// ============================================================================

/**
 * Determine block type from its relations.
 */
function getBlockType(
	block: BlockSnapshot
): "textBlock" | "imageBlock" | "dataBlock" | null {
	// Check relations for type indicators
	for (const rel of block.relations) {
		if (rel.typeId === SystemIds.TYPES_PROPERTY) {
			if (rel.toEntityId === SystemIds.TEXT_BLOCK) return "textBlock";
			if (
				rel.toEntityId === SystemIds.IMAGE_BLOCK ||
				rel.toEntityId === SystemIds.IMAGE
			)
				return "imageBlock";
			if (rel.toEntityId === SystemIds.DATA_BLOCK) return "dataBlock";
		}
	}
	return null;
}

/**
 * Extract markdown content from a text block.
 */
function getMarkdownContent(block: BlockSnapshot): string {
	const markdownValue = block.values.find(
		(v) => v.propertyId === SystemIds.MARKDOWN_CONTENT
	);
	return markdownValue?.string ?? "";
}

/**
 * Extract image URL from an image block.
 */
function getImageUrl(block: BlockSnapshot): string | null {
	const imageValue = block.values.find(
		(v) => v.propertyId === SystemIds.IMAGE_URL_PROPERTY
	);
	return imageValue?.string ?? null;
}

/**
 * Extract name from a data block.
 */
function getBlockName(block: BlockSnapshot): string | null {
	const nameValue = block.values.find(
		(v) => v.propertyId === SystemIds.NAME_PROPERTY
	);
	return nameValue?.string ?? null;
}

/**
 * Compute diff between two sets of blocks.
 */
export function diffBlocks(
	fromBlocks: BlockSnapshot[],
	toBlocks: BlockSnapshot[]
): BlockChange[] {
	const fromMap = new Map(fromBlocks.map((b) => [b.id, b]));
	const toMap = new Map(toBlocks.map((b) => [b.id, b]));
	const changes: BlockChange[] = [];

	// Find added and changed blocks
	for (const [id, toBlock] of toMap) {
		const fromBlock = fromMap.get(id);
		const blockType = getBlockType(toBlock) ?? getBlockType(fromBlock ?? toBlock);

		if (!blockType) continue; // Unknown block type

		if (!fromBlock) {
			// Added block
			switch (blockType) {
				case "textBlock":
					changes.push({
						id,
						type: "textBlock",
						diff: computeTextDiff("", getMarkdownContent(toBlock)),
					});
					break;
				case "imageBlock":
					changes.push({
						id,
						type: "imageBlock",
						from: null,
						to: getImageUrl(toBlock),
					});
					break;
				case "dataBlock":
					changes.push({
						id,
						type: "dataBlock",
						from: null,
						to: getBlockName(toBlock),
					});
					break;
			}
		} else {
			// Check if changed
			switch (blockType) {
				case "textBlock": {
					const fromContent = getMarkdownContent(fromBlock);
					const toContent = getMarkdownContent(toBlock);
					if (fromContent !== toContent) {
						changes.push({
							id,
							type: "textBlock",
							diff: computeTextDiff(fromContent, toContent),
						});
					}
					break;
				}
				case "imageBlock": {
					const fromUrl = getImageUrl(fromBlock);
					const toUrl = getImageUrl(toBlock);
					if (fromUrl !== toUrl) {
						changes.push({
							id,
							type: "imageBlock",
							from: fromUrl,
							to: toUrl,
						});
					}
					break;
				}
				case "dataBlock": {
					const fromName = getBlockName(fromBlock);
					const toName = getBlockName(toBlock);
					if (fromName !== toName) {
						changes.push({
							id,
							type: "dataBlock",
							from: fromName,
							to: toName,
						});
					}
					break;
				}
			}
		}
	}

	// Find removed blocks
	for (const [id, fromBlock] of fromMap) {
		if (!toMap.has(id)) {
			const blockType = getBlockType(fromBlock);
			if (!blockType) continue;

			switch (blockType) {
				case "textBlock":
					changes.push({
						id,
						type: "textBlock",
						diff: computeTextDiff(getMarkdownContent(fromBlock), ""),
					});
					break;
				case "imageBlock":
					changes.push({
						id,
						type: "imageBlock",
						from: getImageUrl(fromBlock),
						to: null,
					});
					break;
				case "dataBlock":
					changes.push({
						id,
						type: "dataBlock",
						from: getBlockName(fromBlock),
						to: null,
					});
					break;
			}
		}
	}

	return changes;
}

// ============================================================================
// Entity Diffing
// ============================================================================

/**
 * Extract entity name from values.
 */
function getEntityName(snapshot: EntitySnapshot): string | null {
	const nameValue = snapshot.values.find(
		(v) => v.propertyId === SystemIds.NAME_PROPERTY
	);
	return nameValue?.string ?? null;
}

/**
 * Compute a full entity diff between two snapshots.
 */
export function diffEntitySnapshots(
	entityId: string,
	from: EntitySnapshot,
	to: EntitySnapshot
): EntityDiff {
	return {
		entityId,
		name: getEntityName(to) ?? getEntityName(from),
		values: diffValues(from.values, to.values),
		relations: diffRelations(from.relations, to.relations),
		blocks: diffBlocks(from.blocks, to.blocks),
	};
}
