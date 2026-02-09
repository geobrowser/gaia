/**
 * Serialization layer for versioned entity responses.
 *
 * Converts internal Uuid fields (dashed hex) to Base58 for API output.
 * Internal types use Uuid for correctness (equality, Map keys, DB queries).
 * This module converts at the response boundary.
 */

import {type Uuid, toBase58} from "../utils/uuid"
import type {
	BlockChange,
	BlockSnapshot,
	DynamicGroupItem,
	EntityDiff,
	EntitySnapshot,
	GroupedEntityDiff,
	GroupedEntitySnapshot,
	PaginatedProposalDiff,
	RelationChange,
	ValueChange,
	VersionEntry,
	VersionedRelation,
	VersionedValue,
} from "./types"

// Helper to encode a Uuid field, preserving null/undefined
function enc(uuid: Uuid): string {
	return toBase58(uuid)
}

function encOpt(uuid: Uuid | null | undefined): string | null | undefined {
	if (uuid === null) return null
	if (uuid === undefined) return undefined
	return toBase58(uuid)
}

// =============================================================================
// Serialized response types (string instead of Uuid for JSON output)
// =============================================================================

function serializeValue(v: VersionedValue) {
	return {
		...v,
		propertyId: enc(v.propertyId),
		spaceId: enc(v.spaceId),
		contextRootId: encOpt(v.contextRootId),
		contextEdgeTypeId: encOpt(v.contextEdgeTypeId),
	}
}

function serializeRelation(r: VersionedRelation) {
	return {
		...r,
		relationId: enc(r.relationId),
		typeId: enc(r.typeId),
		fromEntityId: enc(r.fromEntityId),
		fromSpaceId: encOpt(r.fromSpaceId),
		toEntityId: enc(r.toEntityId),
		toSpaceId: encOpt(r.toSpaceId),
		spaceId: enc(r.spaceId),
		contextRootId: encOpt(r.contextRootId),
		contextEdgeTypeId: encOpt(r.contextEdgeTypeId),
	}
}

function serializeBlockSnapshot(b: BlockSnapshot) {
	return {
		id: enc(b.id),
		values: b.values.map(serializeValue),
		relations: b.relations.map(serializeRelation),
	}
}

function serializeValueChange(vc: ValueChange) {
	return {
		...vc,
		propertyId: enc(vc.propertyId),
		spaceId: enc(vc.spaceId),
	}
}

function serializeRelationEndpoint(endpoint: {toEntityId: Uuid; toSpaceId?: Uuid | null; position?: string | null}) {
	return {
		...endpoint,
		toEntityId: enc(endpoint.toEntityId),
		toSpaceId: encOpt(endpoint.toSpaceId),
	}
}

function serializeRelationChange(rc: RelationChange) {
	return {
		...rc,
		relationId: enc(rc.relationId),
		typeId: enc(rc.typeId),
		spaceId: enc(rc.spaceId),
		before: rc.before ? serializeRelationEndpoint(rc.before) : rc.before,
		after: rc.after ? serializeRelationEndpoint(rc.after) : rc.after,
	}
}

function serializeBlockChange(bc: BlockChange) {
	return {
		...bc,
		id: enc(bc.id),
	}
}

function serializeEntityDiff(diff: EntityDiff) {
	return {
		...diff,
		entityId: enc(diff.entityId),
		values: diff.values.map(serializeValueChange),
		relations: diff.relations.map(serializeRelationChange),
		blocks: diff.blocks.map(serializeBlockChange),
	}
}

function serializeDynamicGroupItem(item: DynamicGroupItem) {
	// DynamicGroupItem = BlockChange | EntityDiff
	// Distinguish by checking for 'entityId' (EntityDiff) vs 'type' (BlockChange)
	if ("entityId" in item) {
		return serializeEntityDiff(item as EntityDiff)
	}
	return serializeBlockChange(item as BlockChange)
}

// =============================================================================
// Public serializers for each response shape
// =============================================================================

export function serializeEntitySnapshot(snapshot: EntitySnapshot) {
	return {
		id: enc(snapshot.id),
		values: snapshot.values.map(serializeValue),
		relations: snapshot.relations.map(serializeRelation),
		blocks: snapshot.blocks.map(serializeBlockSnapshot),
	}
}

export function serializeVersionEntries(versions: VersionEntry[]) {
	return versions.map((v) => ({
		...v,
		editId: enc(v.editId),
	}))
}

export function serializeGroupedEntityDiff(diff: GroupedEntityDiff) {
	const serializedGroups: Record<string, unknown[]> = {}
	for (const [key, items] of Object.entries(diff.groups)) {
		serializedGroups[enc(key as Uuid)] = (items as DynamicGroupItem[]).map(serializeDynamicGroupItem)
	}

	return {
		entityId: enc(diff.entityId),
		name: diff.name,
		values: diff.values.map(serializeValueChange),
		relations: diff.relations.map(serializeRelationChange),
		blocks: diff.blocks.map(serializeBlockChange),
		groupKeys: diff.groupKeys.map(enc),
		groups: serializedGroups,
	}
}

export function serializePaginatedProposalDiff(diff: PaginatedProposalDiff) {
	return {
		...diff,
		proposalId: enc(diff.proposalId),
		spaceId: enc(diff.spaceId),
		entities: diff.entities.map(serializeEntityDiff),
	}
}
