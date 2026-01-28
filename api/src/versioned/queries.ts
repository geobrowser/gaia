/**
 * Database queries for versioned entity reads.
 * Uses raw SQL for simplicity.
 */

import { sql } from "drizzle-orm";
import type { NodePgDatabase } from "drizzle-orm/node-postgres";
import { SystemIds } from "@graphprotocol/grc-20";

import type {
	VersionedValue,
	VersionedRelation,
	BlockSnapshot,
	EntitySnapshot,
	GroupedEntitySnapshot,
	VersionEntry,
} from "./types";
import {
	groupEntitiesByContext,
	mergeDiscoveryResults,
	type DiscoveredEntity,
	type GroupedEntities,
} from "./grouping";

// The BLOCKS relation type ID from GRC-20
const BLOCKS_TYPE_ID = SystemIds.BLOCKS;

// Generic database type
type Database = NodePgDatabase<Record<string, unknown>>;

/**
 * Resolve an edit ID to its version key.
 */
export async function resolveVersionKey(
	db: Database,
	editId: string
): Promise<bigint | null> {
	const result = await db.execute<{ version_key: string }>(sql`
		SELECT version_key FROM edit_versions WHERE edit_id = ${editId} LIMIT 1
	`);

	const row = result.rows[0];
	if (!row) {
		return null;
	}

	return BigInt(row.version_key);
}

/**
 * Get values for an entity at a specific version.
 */
export async function getValuesAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<VersionedValue[]> {
	const versionKeyStr = versionKey.toString();

	const result = spaceId
		? await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM value_versions
				WHERE entity_id = ${entityId}
					AND valid_from_key <= ${versionKeyStr}::bigint
					AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
					AND space_id = ${spaceId}
			`)
		: await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM value_versions
				WHERE entity_id = ${entityId}
					AND valid_from_key <= ${versionKeyStr}::bigint
					AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
			`);

	return result.rows.map((row) => ({
		propertyId: row.property_id as string,
		spaceId: row.space_id as string,
		// Value columns (GRC-20 v2 data types)
		boolean: row.boolean as boolean | null,
		integer: row.integer as number | null,
		float: row.float as number | null,
		decimal: row.decimal as string | null,
		text: row.text as string | null,
		bytes: row.bytes ? Buffer.from(row.bytes as Buffer).toString("base64") : null,
		date: row.date as string | null,
		time: row.time as string | null,
		datetime: row.datetime as string | null,
		schedule: row.schedule as unknown | null,
		point: row.point as string | null,
		embedding: row.embedding as unknown | null,
		// Metadata
		language: row.language as string | null,
		unit: row.unit as string | null,
		// Context metadata (for block grouping)
		contextRootId: row.context_root_id as string | null,
		contextEdgeTypeId: row.context_edge_type_id as string | null,
	}));
}

/**
 * Get relations for an entity at a specific version.
 */
export async function getRelationsAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<VersionedRelation[]> {
	const versionKeyStr = versionKey.toString();

	const result = spaceId
		? await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM relation_versions
				WHERE from_entity_id = ${entityId}
					AND valid_from_key <= ${versionKeyStr}::bigint
					AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
					AND space_id = ${spaceId}
			`)
		: await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM relation_versions
				WHERE from_entity_id = ${entityId}
					AND valid_from_key <= ${versionKeyStr}::bigint
					AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
			`);

	return result.rows.map((row) => ({
		relationId: row.relation_id as string,
		typeId: row.type_id as string,
		fromEntityId: row.from_entity_id as string,
		fromSpaceId: row.from_space_id as string | null,
		toEntityId: row.to_entity_id as string,
		toSpaceId: row.to_space_id as string | null,
		position: row.position as string | null,
		spaceId: row.space_id as string,
		verified: row.verified as boolean | null,
		// Context metadata (for block grouping)
		contextRootId: row.context_root_id as string | null,
		contextEdgeTypeId: row.context_edge_type_id as string | null,
	}));
}

/**
 * Query entities discovered via context metadata.
 * Returns ALL entities where context_root_id = entityId, regardless of edge type.
 */
async function queryContextEntities(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<DiscoveredEntity[]> {
	const versionKeyStr = versionKey.toString();

	const result = spaceId
		? await db.execute<{
				entity_id: string;
				context_edge_type_id: string | null;
		  }>(sql`
				SELECT DISTINCT entity_id, context_edge_type_id FROM (
					-- Context-based discovery from values
					SELECT DISTINCT v.entity_id, v.context_edge_type_id
					FROM value_versions v
					WHERE v.context_root_id = ${entityId}
						AND v.context_edge_type_id IS NOT NULL
						AND v.valid_from_key <= ${versionKeyStr}::bigint
						AND (v.valid_to_key IS NULL OR v.valid_to_key > ${versionKeyStr}::bigint)
						AND v.space_id = ${spaceId}
					UNION
					-- Context-based discovery from relations
					SELECT DISTINCT r.entity_id, r.context_edge_type_id
					FROM relation_versions r
					WHERE r.context_root_id = ${entityId}
						AND r.context_edge_type_id IS NOT NULL
						AND r.valid_from_key <= ${versionKeyStr}::bigint
						AND (r.valid_to_key IS NULL OR r.valid_to_key > ${versionKeyStr}::bigint)
						AND r.space_id = ${spaceId}
				) context_entities
			`)
		: await db.execute<{
				entity_id: string;
				context_edge_type_id: string | null;
		  }>(sql`
				SELECT DISTINCT entity_id, context_edge_type_id FROM (
					-- Context-based discovery from values
					SELECT DISTINCT v.entity_id, v.context_edge_type_id
					FROM value_versions v
					WHERE v.context_root_id = ${entityId}
						AND v.context_edge_type_id IS NOT NULL
						AND v.valid_from_key <= ${versionKeyStr}::bigint
						AND (v.valid_to_key IS NULL OR v.valid_to_key > ${versionKeyStr}::bigint)
					UNION
					-- Context-based discovery from relations
					SELECT DISTINCT r.entity_id, r.context_edge_type_id
					FROM relation_versions r
					WHERE r.context_root_id = ${entityId}
						AND r.context_edge_type_id IS NOT NULL
						AND r.valid_from_key <= ${versionKeyStr}::bigint
						AND (r.valid_to_key IS NULL OR r.valid_to_key > ${versionKeyStr}::bigint)
				) context_entities
			`);

	return result.rows.map((row) => ({
		entityId: row.entity_id,
		contextEdgeTypeId: row.context_edge_type_id,
		position: null, // Context-based discovery doesn't have position
	}));
}

/**
 * Query entities discovered via BLOCKS relation (fallback for data without context).
 */
async function queryBlocksRelationEntities(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<Array<{ entityId: string; position: string | null }>> {
	const versionKeyStr = versionKey.toString();

	const result = spaceId
		? await db.execute<{ entity_id: string; position: string | null }>(sql`
				SELECT r.to_entity_id AS entity_id, r.position
				FROM relation_versions r
				WHERE r.from_entity_id = ${entityId}
					AND r.type_id = ${BLOCKS_TYPE_ID}
					AND r.valid_from_key <= ${versionKeyStr}::bigint
					AND (r.valid_to_key IS NULL OR r.valid_to_key > ${versionKeyStr}::bigint)
					AND r.space_id = ${spaceId}
			`)
		: await db.execute<{ entity_id: string; position: string | null }>(sql`
				SELECT r.to_entity_id AS entity_id, r.position
				FROM relation_versions r
				WHERE r.from_entity_id = ${entityId}
					AND r.type_id = ${BLOCKS_TYPE_ID}
					AND r.valid_from_key <= ${versionKeyStr}::bigint
					AND (r.valid_to_key IS NULL OR r.valid_to_key > ${versionKeyStr}::bigint)
			`);

	return result.rows.map((row) => ({
		entityId: row.entity_id,
		position: row.position,
	}));
}

/**
 * Get grouped entity IDs for an entity at a specific version.
 *
 * Supports hybrid mode:
 * - Static `blocks` array for BLOCKS relation type
 * - Dynamic groups for other relation types discovered via context
 * - `groupKeys` array for discoverability of dynamic keys
 *
 * Uses context metadata to discover entities when available, with
 * BLOCKS relation lookup as fallback for backward compatibility.
 */
export async function getGroupedEntityIdsAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<GroupedEntities> {
	const [contextEntities, relationEntities] = await Promise.all([
		queryContextEntities(db, entityId, versionKey, spaceId),
		queryBlocksRelationEntities(db, entityId, versionKey, spaceId),
	]);

	// Merge and group using pure function
	const merged = mergeDiscoveryResults(
		contextEntities,
		relationEntities,
		BLOCKS_TYPE_ID
	);

	return groupEntitiesByContext(merged, BLOCKS_TYPE_ID);
}

/**
 * Get block IDs for an entity at a specific version.
 *
 * This is a convenience wrapper around getGroupedEntityIdsAtVersion
 * that returns only the blocks array for backward compatibility.
 */
async function getBlockIdsAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<string[]> {
	const grouped = await getGroupedEntityIdsAtVersion(
		db,
		entityId,
		versionKey,
		spaceId
	);
	return grouped.blocks;
}

/**
 * Get a block snapshot at a specific version.
 */
async function getBlockSnapshotAtVersion(
	db: Database,
	blockId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<BlockSnapshot> {
	const [values, allRelations] = await Promise.all([
		getValuesAtVersion(db, blockId, versionKey, spaceId),
		getRelationsAtVersion(db, blockId, versionKey, spaceId),
	]);

	// Filter out block relations
	const relations = allRelations.filter((r) => r.typeId !== BLOCKS_TYPE_ID);

	return { id: blockId, values, relations };
}

/**
 * Get a full entity snapshot at a specific version, including blocks.
 */
export async function getEntitySnapshotAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<EntitySnapshot> {
	const [values, allRelations, blockIds] = await Promise.all([
		getValuesAtVersion(db, entityId, versionKey, spaceId),
		getRelationsAtVersion(db, entityId, versionKey, spaceId),
		getBlockIdsAtVersion(db, entityId, versionKey, spaceId),
	]);

	// Filter out block relations
	const relations = allRelations.filter((r) => r.typeId !== BLOCKS_TYPE_ID);

	// Fetch block snapshots
	const blocks = await Promise.all(
		blockIds.map((id) => getBlockSnapshotAtVersion(db, id, versionKey, spaceId))
	);

	return { id: entityId, values, relations, blocks };
}

/**
 * Get a grouped entity snapshot at a specific version.
 *
 * Returns hybrid mode response with:
 * - Static `blocks` array for BLOCKS relation type
 * - Dynamic `groups` map for other relation types
 * - `groupKeys` for discoverability of dynamic groups
 */
export async function getGroupedEntitySnapshotAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Promise<GroupedEntitySnapshot> {
	const [values, allRelations, grouped] = await Promise.all([
		getValuesAtVersion(db, entityId, versionKey, spaceId),
		getRelationsAtVersion(db, entityId, versionKey, spaceId),
		getGroupedEntityIdsAtVersion(db, entityId, versionKey, spaceId),
	]);

	// Filter out relations that are used for grouping (BLOCKS + dynamic types)
	const groupedTypeIds = new Set([BLOCKS_TYPE_ID, ...grouped.groupKeys]);
	const relations = allRelations.filter((r) => !groupedTypeIds.has(r.typeId));

	// Fetch block snapshots (static key)
	const blocks = await Promise.all(
		grouped.blocks.map((id) =>
			getBlockSnapshotAtVersion(db, id, versionKey, spaceId)
		)
	);

	// Fetch dynamic group snapshots
	const groups: Record<string, BlockSnapshot[]> = {};
	for (const [typeId, entityIds] of grouped.dynamicGroups) {
		groups[typeId] = await Promise.all(
			entityIds.map((id) =>
				getBlockSnapshotAtVersion(db, id, versionKey, spaceId)
			)
		);
	}

	return {
		id: entityId,
		values,
		relations,
		blocks,
		groupKeys: grouped.groupKeys,
		groups,
	};
}

/**
 * Get versions (edits) that affected an entity.
 */
export async function getEntityVersions(
	db: Database,
	entityId: string,
	spaceId?: string,
	limit = 50,
	offset = 0
): Promise<VersionEntry[]> {
	const result = spaceId
		? await db.execute<{
				edit_id: string;
				block_number: string;
				sequence: number;
				created_at: Date;
				version_key: string;
		  }>(sql`
				SELECT DISTINCT e.edit_id, e.block_number, e.sequence, e.created_at, e.version_key
				FROM edit_versions e
				WHERE e.version_key IN (
					SELECT DISTINCT valid_from_key FROM value_versions
					WHERE entity_id = ${entityId} AND space_id = ${spaceId}
					UNION
					SELECT DISTINCT valid_from_key FROM relation_versions
					WHERE from_entity_id = ${entityId} AND space_id = ${spaceId}
				)
				ORDER BY e.version_key DESC
				LIMIT ${limit} OFFSET ${offset}
			`)
		: await db.execute<{
				edit_id: string;
				block_number: string;
				sequence: number;
				created_at: Date;
				version_key: string;
		  }>(sql`
				SELECT DISTINCT e.edit_id, e.block_number, e.sequence, e.created_at, e.version_key
				FROM edit_versions e
				WHERE e.version_key IN (
					SELECT DISTINCT valid_from_key FROM value_versions
					WHERE entity_id = ${entityId}
					UNION
					SELECT DISTINCT valid_from_key FROM relation_versions
					WHERE from_entity_id = ${entityId}
				)
				ORDER BY e.version_key DESC
				LIMIT ${limit} OFFSET ${offset}
			`);

	return result.rows.map((row) => ({
		editId: row.edit_id,
		blockNumber: row.block_number.toString(),
		sequence: row.sequence,
		createdAt:
			row.created_at instanceof Date
				? row.created_at.toISOString()
				: new Date(row.created_at).toISOString(),
	}));
}
