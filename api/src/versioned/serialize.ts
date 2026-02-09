/**
 * Serialization layer for versioned entity responses.
 *
 * Converts internal Uuid fields (dashed hex) to Base58 for API output.
 * Internal types use Uuid for correctness (equality, Map keys, DB queries).
 * This module converts at the response boundary.
 */

import {toBase58, type Uuid} from "../utils/uuid"
import type {
	BlockChange,
	BlockSnapshot,
	DynamicGroupItem,
	EntityDiff,
	EntitySnapshot,
	GroupedEntityDiff,
	PaginatedProposalDiff,
	RelationChange,
	ValueChange,
	VersionEntry,
	VersionedRelation,
	VersionedValue,
} from "./types"

// Helper to encode an optional Uuid field, preserving null/undefined
function toBase58Opt(uuid: Uuid | null | undefined): string | null | undefined {
	if (uuid === null) return null
	if (uuid === undefined) return undefined
	return toBase58(uuid)
}

/**
 * Type guard for EntityDiff vs BlockChange in DynamicGroupItem unions.
 *
 * EntityDiff has `entityId`, `name`, `values`, `relations`, `blocks`.
 * BlockChange has `id`, `type` (literal string), `before`, `after`.
 * We check for both `entityId` and `values` to avoid fragile single-field checks.
 */
function isEntityDiff(item: DynamicGroupItem): item is EntityDiff {
	return "entityId" in item && "values" in item
}

// =============================================================================
// Serialized response types (string instead of Uuid for JSON output)
// =============================================================================

function serializeValue(v: VersionedValue) {
	return {
		...v,
		propertyId: toBase58(v.propertyId),
		spaceId: toBase58(v.spaceId),
		contextRootId: toBase58Opt(v.contextRootId),
		contextEdgeTypeId: toBase58Opt(v.contextEdgeTypeId),
	}
}

function serializeRelation(r: VersionedRelation) {
	return {
		...r,
		relationId: toBase58(r.relationId),
		typeId: toBase58(r.typeId),
		fromEntityId: toBase58(r.fromEntityId),
		fromSpaceId: toBase58Opt(r.fromSpaceId),
		toEntityId: toBase58(r.toEntityId),
		toSpaceId: toBase58Opt(r.toSpaceId),
		spaceId: toBase58(r.spaceId),
		contextRootId: toBase58Opt(r.contextRootId),
		contextEdgeTypeId: toBase58Opt(r.contextEdgeTypeId),
	}
}

function serializeBlockSnapshot(b: BlockSnapshot) {
	return {
		id: toBase58(b.id),
		values: b.values.map(serializeValue),
		relations: b.relations.map(serializeRelation),
	}
}

function serializeValueChange(vc: ValueChange) {
	return {
		...vc,
		propertyId: toBase58(vc.propertyId),
		spaceId: toBase58(vc.spaceId),
	}
}

function serializeRelationEndpoint(endpoint: {toEntityId: Uuid; toSpaceId?: Uuid | null; position?: string | null}) {
	return {
		...endpoint,
		toEntityId: toBase58(endpoint.toEntityId),
		toSpaceId: toBase58Opt(endpoint.toSpaceId),
	}
}

function serializeRelationChange(rc: RelationChange) {
	return {
		...rc,
		relationId: toBase58(rc.relationId),
		typeId: toBase58(rc.typeId),
		spaceId: toBase58(rc.spaceId),
		before: rc.before ? serializeRelationEndpoint(rc.before) : rc.before,
		after: rc.after ? serializeRelationEndpoint(rc.after) : rc.after,
	}
}

function serializeBlockChange(bc: BlockChange) {
	return {
		...bc,
		id: toBase58(bc.id),
	}
}

function serializeEntityDiff(diff: EntityDiff) {
	return {
		...diff,
		entityId: toBase58(diff.entityId),
		values: diff.values.map(serializeValueChange),
		relations: diff.relations.map(serializeRelationChange),
		blocks: diff.blocks.map(serializeBlockChange),
	}
}

function serializeDynamicGroupItem(item: DynamicGroupItem) {
	if (isEntityDiff(item)) {
		return serializeEntityDiff(item)
	}
	return serializeBlockChange(item)
}

// =============================================================================
// Public serializers for each response shape
// =============================================================================

export function serializeEntitySnapshot(snapshot: EntitySnapshot) {
	return {
		id: toBase58(snapshot.id),
		values: snapshot.values.map(serializeValue),
		relations: snapshot.relations.map(serializeRelation),
		blocks: snapshot.blocks.map(serializeBlockSnapshot),
	}
}

export function serializeVersionEntries(versions: VersionEntry[]) {
	return versions.map((v) => ({
		...v,
		editId: toBase58(v.editId),
	}))
}

export function serializeGroupedEntityDiff(diff: GroupedEntityDiff) {
	const serializedGroups: Record<string, unknown[]> = {}
	for (const [key, items] of Object.entries(diff.groups)) {
		serializedGroups[toBase58(key as Uuid)] = (items as DynamicGroupItem[]).map(serializeDynamicGroupItem)
	}

	return {
		entityId: toBase58(diff.entityId),
		name: diff.name,
		values: diff.values.map(serializeValueChange),
		relations: diff.relations.map(serializeRelationChange),
		blocks: diff.blocks.map(serializeBlockChange),
		groupKeys: diff.groupKeys.map(toBase58),
		groups: serializedGroups,
	}
}

export function serializePaginatedProposalDiff(diff: PaginatedProposalDiff) {
	return {
		...diff,
		proposalId: toBase58(diff.proposalId),
		spaceId: toBase58(diff.spaceId),
		entities: diff.entities.map(serializeEntityDiff),
	}
}
