/**
 * Diff computation for versioned entities.
 */

import type {
	VersionedValue,
	VersionedRelation,
	BlockSnapshot,
	EntitySnapshot,
	ValueChanges,
	RelationChanges,
	BlockChanges,
	BlockChange,
	EntityDiff,
} from "./types";

/**
 * Create a unique key for a value (property + space).
 */
function valueKey(v: VersionedValue): string {
	return `${v.propertyId}:${v.spaceId}`;
}

/**
 * Check if two values are equal (same actual value data).
 */
function valuesEqual(a: VersionedValue, b: VersionedValue): boolean {
	return (
		a.string === b.string &&
		a.boolean === b.boolean &&
		a.number === b.number &&
		a.time === b.time &&
		a.point === b.point &&
		a.language === b.language &&
		a.unit === b.unit &&
		a.integer === b.integer &&
		a.float === b.float &&
		a.bytes === b.bytes &&
		a.date === b.date &&
		a.datetime === b.datetime &&
		JSON.stringify(a.schedule) === JSON.stringify(b.schedule) &&
		JSON.stringify(a.embedding) === JSON.stringify(b.embedding)
	);
}

/**
 * Compute diff between two sets of values.
 */
export function diffValues(
	fromValues: VersionedValue[],
	toValues: VersionedValue[]
): ValueChanges {
	const fromMap = new Map(fromValues.map((v) => [valueKey(v), v]));
	const toMap = new Map(toValues.map((v) => [valueKey(v), v]));

	const added: VersionedValue[] = [];
	const removed: VersionedValue[] = [];
	const changed: ValueChanges["changed"] = [];

	// Find added and changed
	for (const [key, toValue] of toMap) {
		const fromValue = fromMap.get(key);
		if (!fromValue) {
			added.push(toValue);
		} else if (!valuesEqual(fromValue, toValue)) {
			changed.push({
				propertyId: toValue.propertyId,
				spaceId: toValue.spaceId,
				from: fromValue,
				to: toValue,
			});
		}
	}

	// Find removed
	for (const [key, fromValue] of fromMap) {
		if (!toMap.has(key)) {
			removed.push(fromValue);
		}
	}

	return { added, removed, changed };
}

/**
 * Check if two relations are equal.
 */
function relationsEqual(a: VersionedRelation, b: VersionedRelation): boolean {
	return (
		a.typeId === b.typeId &&
		a.fromEntityId === b.fromEntityId &&
		a.fromSpaceId === b.fromSpaceId &&
		a.toEntityId === b.toEntityId &&
		a.toSpaceId === b.toSpaceId &&
		a.position === b.position &&
		a.spaceId === b.spaceId &&
		a.verified === b.verified
	);
}

/**
 * Compute diff between two sets of relations.
 */
export function diffRelations(
	fromRelations: VersionedRelation[],
	toRelations: VersionedRelation[]
): RelationChanges {
	const fromMap = new Map(fromRelations.map((r) => [r.relationId, r]));
	const toMap = new Map(toRelations.map((r) => [r.relationId, r]));

	const added: VersionedRelation[] = [];
	const removed: VersionedRelation[] = [];
	const changed: RelationChanges["changed"] = [];

	// Find added and changed
	for (const [id, toRel] of toMap) {
		const fromRel = fromMap.get(id);
		if (!fromRel) {
			added.push(toRel);
		} else if (!relationsEqual(fromRel, toRel)) {
			changed.push({
				relationId: id,
				from: fromRel,
				to: toRel,
			});
		}
	}

	// Find removed
	for (const [id, fromRel] of fromMap) {
		if (!toMap.has(id)) {
			removed.push(fromRel);
		}
	}

	return { added, removed, changed };
}

/**
 * Compute diff between two sets of blocks.
 */
export function diffBlocks(
	fromBlocks: BlockSnapshot[],
	toBlocks: BlockSnapshot[]
): BlockChanges {
	const fromMap = new Map(fromBlocks.map((b) => [b.id, b]));
	const toMap = new Map(toBlocks.map((b) => [b.id, b]));

	const added: BlockSnapshot[] = [];
	const removed: string[] = [];
	const changed: BlockChange[] = [];

	// Find added and changed
	for (const [id, toBlock] of toMap) {
		const fromBlock = fromMap.get(id);
		if (!fromBlock) {
			added.push(toBlock);
		} else {
			// Check if block content changed
			const valueDiff = diffValues(fromBlock.values, toBlock.values);
			const relationDiff = diffRelations(fromBlock.relations, toBlock.relations);

			const hasChanges =
				valueDiff.added.length > 0 ||
				valueDiff.removed.length > 0 ||
				valueDiff.changed.length > 0 ||
				relationDiff.added.length > 0 ||
				relationDiff.removed.length > 0 ||
				relationDiff.changed.length > 0;

			if (hasChanges) {
				changed.push({
					id,
					values: valueDiff,
					relations: relationDiff,
				});
			}
		}
	}

	// Find removed
	for (const [id] of fromMap) {
		if (!toMap.has(id)) {
			removed.push(id);
		}
	}

	return { added, removed, changed };
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
		values: diffValues(from.values, to.values),
		relations: diffRelations(from.relations, to.relations),
		blocks: diffBlocks(from.blocks, to.blocks),
	};
}
