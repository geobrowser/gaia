/**
 * Database queries for versioned entity reads.
 * Uses raw SQL for simplicity.
 * All queries are wrapped in Effect for proper error handling and tracing.
 */

import { Effect } from "effect";
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

// Error type for database query failures
export class QueryError {
	readonly _tag = "QueryError";
	constructor(
		readonly operation: string,
		readonly cause: unknown
	) {}
}

// The BLOCKS relation type ID from GRC-20
const BLOCKS_TYPE_ID = SystemIds.BLOCKS;

// Generic database type
type Database = NodePgDatabase<Record<string, unknown>>;

// =============================================================================
// Row Mapping Helpers
// =============================================================================

/**
 * Map a database row to a VersionedValue.
 */
function mapValueRow(row: Record<string, unknown>): VersionedValue {
	return {
		propertyId: row.property_id as string,
		spaceId: row.space_id as string,
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
		language: row.language as string | null,
		unit: row.unit as string | null,
		contextRootId: row.context_root_id as string | null,
		contextEdgeTypeId: row.context_edge_type_id as string | null,
	};
}

/**
 * Map a database row to a VersionedRelation.
 */
function mapRelationRow(row: Record<string, unknown>): VersionedRelation {
	return {
		relationId: row.relation_id as string,
		typeId: row.type_id as string,
		fromEntityId: row.from_entity_id as string,
		fromSpaceId: row.from_space_id as string | null,
		toEntityId: row.to_entity_id as string,
		toSpaceId: row.to_space_id as string | null,
		position: row.position as string | null,
		spaceId: row.space_id as string,
		verified: row.verified as boolean | null,
		contextRootId: row.context_root_id as string | null,
		contextEdgeTypeId: row.context_edge_type_id as string | null,
	};
}

/**
 * Resolve an edit ID to its version key.
 */
export function resolveVersionKey(
	db: Database,
	editId: string
): Effect.Effect<bigint | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<{ version_key: string }>(sql`
				SELECT version_key FROM edit_versions WHERE edit_id = ${editId} LIMIT 1
			`);

			const row = result.rows[0];
			if (!row) {
				return null;
			}

			return BigInt(row.version_key);
		},
		catch: (error) => new QueryError("resolveVersionKey", error),
	}).pipe(
		Effect.withSpan("queries.resolveVersionKey", {
			attributes: { "query.edit_id": editId },
		})
	);
}

/**
 * Get values for an entity at a specific version.
 */
export function getValuesAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<VersionedValue[], QueryError> {
	return Effect.tryPromise({
		try: async () => {
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

			return result.rows.map(mapValueRow);
		},
		catch: (error) => new QueryError("getValuesAtVersion", error),
	}).pipe(
		Effect.withSpan("queries.getValuesAtVersion", {
			attributes: {
				"query.entity_id": entityId,
				"query.version_key": versionKey.toString(),
				"query.space_id": spaceId ?? "all",
			},
		})
	);
}

/**
 * Get relations for an entity at a specific version.
 */
export function getRelationsAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<VersionedRelation[], QueryError> {
	return Effect.tryPromise({
		try: async () => {
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

			return result.rows.map(mapRelationRow);
		},
		catch: (error) => new QueryError("getRelationsAtVersion", error),
	}).pipe(
		Effect.withSpan("queries.getRelationsAtVersion", {
			attributes: {
				"query.entity_id": entityId,
				"query.version_key": versionKey.toString(),
				"query.space_id": spaceId ?? "all",
			},
		})
	);
}

/**
 * Query entities discovered via context metadata.
 * Returns ALL entities where context_root_id = entityId, regardless of edge type.
 */
function queryContextEntities(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<DiscoveredEntity[], QueryError> {
	return Effect.tryPromise({
		try: async () => {
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
		},
		catch: (error) => new QueryError("queryContextEntities", error),
	}).pipe(
		Effect.withSpan("queries.queryContextEntities", {
			attributes: {
				"query.entity_id": entityId,
				"query.version_key": versionKey.toString(),
			},
		})
	);
}

/**
 * Query entities discovered via BLOCKS relation (fallback for data without context).
 */
function queryBlocksRelationEntities(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<Array<{ entityId: string; position: string | null }>, QueryError> {
	return Effect.tryPromise({
		try: async () => {
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
		},
		catch: (error) => new QueryError("queryBlocksRelationEntities", error),
	}).pipe(
		Effect.withSpan("queries.queryBlocksRelationEntities", {
			attributes: {
				"query.entity_id": entityId,
				"query.version_key": versionKey.toString(),
			},
		})
	);
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
export function getGroupedEntityIdsAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<GroupedEntities, QueryError> {
	return Effect.gen(function* () {
		const [contextEntities, relationEntities] = yield* Effect.all([
			queryContextEntities(db, entityId, versionKey, spaceId),
			queryBlocksRelationEntities(db, entityId, versionKey, spaceId),
		]);

		// Merge and group using pure functions
		const merged = yield* mergeDiscoveryResults(
			contextEntities,
			relationEntities,
			BLOCKS_TYPE_ID
		);

		return yield* groupEntitiesByContext(merged, BLOCKS_TYPE_ID);
	}).pipe(
		Effect.withSpan("queries.getGroupedEntityIdsAtVersion", {
			attributes: {
				"query.entity_id": entityId,
				"query.version_key": versionKey.toString(),
			},
		})
	);
}

/**
 * Get block IDs for an entity at a specific version.
 *
 * This is a convenience wrapper around getGroupedEntityIdsAtVersion
 * that returns only the blocks array for backward compatibility.
 */
function getBlockIdsAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<string[], QueryError> {
	return Effect.map(
		getGroupedEntityIdsAtVersion(db, entityId, versionKey, spaceId),
		(grouped) => grouped.blocks
	);
}

/**
 * Batch fetch block snapshots at a specific version.
 * Uses 2 queries total (values + relations) instead of 2N queries.
 */
function batchGetBlockSnapshotsAtVersion(
	db: Database,
	blockIds: string[],
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<BlockSnapshot[], QueryError> {
	if (blockIds.length === 0) {
		return Effect.succeed([]);
	}

	return Effect.tryPromise({
		try: async () => {
			const versionKeyStr = versionKey.toString();
			// Cast to PostgreSQL array type for ANY() operator
			const blockIdsArray = `{${blockIds.join(",")}}`;

			// Query 1: All values for all blocks
			const valuesResult = spaceId
				? await db.execute<Record<string, unknown>>(sql`
						SELECT * FROM value_versions
						WHERE entity_id = ANY(${blockIdsArray}::uuid[])
							AND valid_from_key <= ${versionKeyStr}::bigint
							AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
							AND space_id = ${spaceId}
					`)
				: await db.execute<Record<string, unknown>>(sql`
						SELECT * FROM value_versions
						WHERE entity_id = ANY(${blockIdsArray}::uuid[])
							AND valid_from_key <= ${versionKeyStr}::bigint
							AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
					`);

			// Query 2: All relations for all blocks (excluding BLOCKS type)
			const relationsResult = spaceId
				? await db.execute<Record<string, unknown>>(sql`
						SELECT * FROM relation_versions
						WHERE from_entity_id = ANY(${blockIdsArray}::uuid[])
							AND type_id != ${BLOCKS_TYPE_ID}
							AND valid_from_key <= ${versionKeyStr}::bigint
							AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
							AND space_id = ${spaceId}
					`)
				: await db.execute<Record<string, unknown>>(sql`
						SELECT * FROM relation_versions
						WHERE from_entity_id = ANY(${blockIdsArray}::uuid[])
							AND type_id != ${BLOCKS_TYPE_ID}
							AND valid_from_key <= ${versionKeyStr}::bigint
							AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
					`);

			// Group by entity ID
			const valuesMap = new Map<string, VersionedValue[]>();
			const relationsMap = new Map<string, VersionedRelation[]>();

			for (const id of blockIds) {
				valuesMap.set(id, []);
				relationsMap.set(id, []);
			}

			for (const row of valuesResult.rows) {
				const entityId = row.entity_id as string;
				valuesMap.get(entityId)?.push(mapValueRow(row));
			}

			for (const row of relationsResult.rows) {
				const entityId = row.from_entity_id as string;
				relationsMap.get(entityId)?.push(mapRelationRow(row));
			}

			// Build snapshots in order
			return blockIds.map((id) => ({
				id,
				values: valuesMap.get(id) ?? [],
				relations: relationsMap.get(id) ?? [],
			}));
		},
		catch: (error) => new QueryError("batchGetBlockSnapshotsAtVersion", error),
	}).pipe(
		Effect.withSpan("queries.batchGetBlockSnapshotsAtVersion", {
			attributes: {
				"query.block_count": blockIds.length,
				"query.version_key": versionKey.toString(),
			},
		})
	);
}

/**
 * Get a full entity snapshot at a specific version, including blocks.
 *
 * Query count: 4 + 2 = 6 queries total (regardless of block count)
 * - 1 for entity values
 * - 1 for entity relations
 * - 2 for block discovery (context + relation fallback)
 * - 1 for all block values (batched)
 * - 1 for all block relations (batched)
 */
export function getEntitySnapshotAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<EntitySnapshot, QueryError> {
	return Effect.gen(function* () {
		const [values, allRelations, blockIds] = yield* Effect.all([
			getValuesAtVersion(db, entityId, versionKey, spaceId),
			getRelationsAtVersion(db, entityId, versionKey, spaceId),
			getBlockIdsAtVersion(db, entityId, versionKey, spaceId),
		]);

		// Filter out block relations
		const relations = allRelations.filter((r) => r.typeId !== BLOCKS_TYPE_ID);

		// Batch fetch all block snapshots (2 queries instead of 2N)
		const blocks = yield* batchGetBlockSnapshotsAtVersion(db, blockIds, versionKey, spaceId);

		return { id: entityId, values, relations, blocks };
	}).pipe(
		Effect.withSpan("queries.getEntitySnapshotAtVersion", {
			attributes: {
				"query.entity_id": entityId,
				"query.version_key": versionKey.toString(),
			},
		})
	);
}

/**
 * Get a grouped entity snapshot at a specific version.
 *
 * Returns hybrid mode response with:
 * - Static `blocks` array for BLOCKS relation type
 * - Dynamic `groups` map for other relation types
 * - `groupKeys` for discoverability of dynamic groups
 *
 * Query count: 4 + 2 = 6 queries total (regardless of block/group count)
 * - 1 for entity values
 * - 1 for entity relations
 * - 2 for block discovery (context + relation fallback)
 * - 1 for all child values (batched across blocks + dynamic groups)
 * - 1 for all child relations (batched across blocks + dynamic groups)
 */
export function getGroupedEntitySnapshotAtVersion(
	db: Database,
	entityId: string,
	versionKey: bigint,
	spaceId?: string
): Effect.Effect<GroupedEntitySnapshot, QueryError> {
	return Effect.gen(function* () {
		const [values, allRelations, grouped] = yield* Effect.all([
			getValuesAtVersion(db, entityId, versionKey, spaceId),
			getRelationsAtVersion(db, entityId, versionKey, spaceId),
			getGroupedEntityIdsAtVersion(db, entityId, versionKey, spaceId),
		]);

		// Filter out relations that are used for grouping (BLOCKS + dynamic types)
		const groupedTypeIds = new Set([BLOCKS_TYPE_ID, ...grouped.groupKeys]);
		const relations = allRelations.filter((r) => !groupedTypeIds.has(r.typeId));

		// Collect all child entity IDs for batch fetching
		const allChildIds = [
			...grouped.blocks,
			...Array.from(grouped.dynamicGroups.values()).flat(),
		];

		// Batch fetch all child snapshots (2 queries instead of 2N)
		const childSnapshots = yield* batchGetBlockSnapshotsAtVersion(
			db,
			allChildIds,
			versionKey,
			spaceId
		);

		// Build a map for quick lookup
		const snapshotMap = new Map<string, BlockSnapshot>();
		for (const snapshot of childSnapshots) {
			snapshotMap.set(snapshot.id, snapshot);
		}

		// Extract blocks (preserving order)
		const blocks = grouped.blocks.map((id) => snapshotMap.get(id)!);

		// Extract dynamic groups (preserving order within each group)
		const groups: Record<string, BlockSnapshot[]> = {};
		for (const [typeId, entityIds] of grouped.dynamicGroups) {
			groups[typeId] = entityIds.map((id) => snapshotMap.get(id)!);
		}

		return {
			id: entityId,
			values,
			relations,
			blocks,
			groupKeys: grouped.groupKeys,
			groups,
		};
	}).pipe(
		Effect.withSpan("queries.getGroupedEntitySnapshotAtVersion", {
			attributes: {
				"query.entity_id": entityId,
				"query.version_key": versionKey.toString(),
			},
		})
	);
}

/**
 * Get versions (edits) that affected an entity.
 */
export function getEntityVersions(
	db: Database,
	entityId: string,
	spaceId?: string,
	limit = 50,
	offset = 0
): Effect.Effect<VersionEntry[], QueryError> {
	return Effect.tryPromise({
		try: async () => {
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
		},
		catch: (error) => new QueryError("getEntityVersions", error),
	}).pipe(
		Effect.withSpan("queries.getEntityVersions", {
			attributes: {
				"query.entity_id": entityId,
				"query.space_id": spaceId ?? "all",
				"query.limit": limit,
				"query.offset": offset,
			},
		})
	);
}
