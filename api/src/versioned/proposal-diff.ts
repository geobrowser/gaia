/**
 * Proposal diff computation.
 *
 * Computes diffs between a proposal's proposed changes and the base state.
 * - Active proposals: compare against current live state
 * - Closed proposals: compare against versioned state at end_time
 *
 * KNOWN LIMITATIONS:
 * The following op types are not fully supported for diff computation:
 *
 * - restoreEntity: The op only contains the entity ID, not the values/relations
 *   to restore. We'd need to fetch historical state to show what's being restored.
 *
 * - deleteEntity: Currently shows removal of all current values/relations, but
 *   doesn't account for the entity potentially being in a deleted state already.
 *
 * - restoreRelation: The op only contains the relation ID. Since the relation
 *   doesn't exist in the live table (it was deleted), we can't look up which
 *   entity is affected. These ops are silently skipped.
 *
 * To properly support these ops, we'd need to restructure the code to:
 * 1. Pass version context into entity extraction
 * 2. Fetch historical state for restore ops
 * 3. Track entity/relation deletion state
 */

import {decodeEditAuto, type Id, type Op} from "@geoprotocol/grc-20"
import {SystemIds} from "@graphprotocol/grc-20"
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import {type NormalizedUuid, normalizeUuid} from "../utils/uuid"
import {diffEntitySnapshots} from "./diff"
import {
	type BlockRelationEntry,
	batchGetBlockRelationsForEntities,
	batchGetBlockSnapshotsAtVersion,
	batchGetLiveBlockRelationsForEntities,
	batchGetLiveBlockSnapshots,
	mapRelationRow,
	mapValueRow,
	QueryError,
} from "./queries"
import type {
	BlockSnapshot,
	EntityDiff,
	EntitySnapshot,
	GroupedProposalDiffMode,
	PaginatedGroupedProposalDiff,
	PaginatedProposalDiff,
	ProposalDiffCursor,
	ProposalStatus,
	VersionedRelation,
	VersionedValue,
} from "./types"

// Error types for proposal diff operations
export class ProposalNotFoundError {
	readonly _tag = "ProposalNotFoundError"
	constructor(readonly proposalId: string) {}
}

export class EditBlobNotCachedError {
	readonly _tag = "EditBlobNotCachedError"
	constructor(readonly uri: string) {}
}

export class EditBlobDecodeFailedError {
	readonly _tag = "EditBlobDecodeFailedError"
	constructor(readonly uri: string) {}
}

export class EditDecodeError {
	readonly _tag = "EditDecodeError"
	constructor(readonly cause: unknown) {}
}

export class SpaceMismatchError {
	readonly _tag = "SpaceMismatchError"
	constructor(
		readonly expectedSpaceId: string,
		readonly actualSpaceId: string,
	) {}
}

export class InvalidCursorError {
	readonly _tag = "InvalidCursorError"
	constructor(readonly cursor: string) {}
}

// Grouped diff error types
export class GroupSizeLimitError {
	readonly _tag = "GroupSizeLimitError"
	constructor(
		readonly max: number,
		readonly actual: number,
	) {}
}

export class DuplicateProposalError {
	readonly _tag = "DuplicateProposalError"
	constructor(readonly duplicates: string[]) {}
}

export class MixedModeError {
	readonly _tag = "MixedModeError"
	constructor(
		readonly activeCount: number,
		readonly nonActiveCount: number,
	) {}
}

export class MissingPublishActionError {
	readonly _tag = "MissingPublishActionError"
	constructor(readonly proposalId: string) {}
}

export type ProposalDiffError =
	| QueryError
	| ProposalNotFoundError
	| EditBlobNotCachedError
	| EditBlobDecodeFailedError
	| EditDecodeError
	| SpaceMismatchError
	| InvalidCursorError

export type GroupedProposalDiffError =
	| ProposalDiffError
	| GroupSizeLimitError
	| DuplicateProposalError
	| MixedModeError
	| MissingPublishActionError

type Database = NodePgDatabase<Record<string, unknown>>

// The BLOCKS relation type ID from GRC-20, normalized for comparison with DB output
const BLOCKS_TYPE_ID = normalizeUuid(SystemIds.BLOCKS)

// ============================================================================
// Database Queries
// ============================================================================

interface ProposalWithAction {
	proposal: {
		id: NormalizedUuid
		spaceId: NormalizedUuid
		startTime: bigint
		endTime: bigint
		executedAt: bigint | null
	}
	contentUri: string | null
}

/**
 * Get proposal with its Publish action (if any) in a single query.
 */
function getProposalWithPublishAction(
	db: Database,
	proposalId: string,
): Effect.Effect<ProposalWithAction | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<{
				proposal_id: string
				space_id: string
				start_time: string
				end_time: string
				executed_at: string | null
				content_uri: string | null
			}>(sql`
				SELECT
					p.id as proposal_id,
					p.space_id,
					p.start_time,
					p.end_time,
					p.executed_at,
					pa.content_uri
				FROM proposals p
				LEFT JOIN proposal_actions pa ON pa.proposal_id = p.id AND pa.action_type = 'Publish'
				WHERE p.id = ${proposalId}
				LIMIT 1
			`)

			const row = result.rows[0]
			if (!row) {
				return null
			}

			return {
				proposal: {
					id: normalizeUuid(row.proposal_id),
					spaceId: normalizeUuid(row.space_id),
					startTime: BigInt(row.start_time),
					endTime: BigInt(row.end_time),
					executedAt: row.executed_at ? BigInt(row.executed_at) : null,
				},
				contentUri: row.content_uri,
			}
		},
		catch: (error) => new QueryError("getProposalWithPublishAction", error),
	}).pipe(
		Effect.withSpan("proposal-diff.getProposalWithPublishAction", {
			attributes: {proposalId},
		}),
	)
}

/**
 * Batch-load multiple proposals with their Publish actions in a single query.
 * Returns a Map keyed by proposal ID for O(1) lookup.
 */
function batchGetProposalsWithPublishActions(
	db: Database,
	proposalIds: NormalizedUuid[],
): Effect.Effect<Map<NormalizedUuid, ProposalWithAction>, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${proposalIds.join(",")}}`
			const result = await db.execute<{
				proposal_id: string
				space_id: string
				start_time: string
				end_time: string
				executed_at: string | null
				content_uri: string | null
			}>(sql`
				SELECT
					p.id as proposal_id,
					p.space_id,
					p.start_time,
					p.end_time,
					p.executed_at,
					pa.content_uri
				FROM proposals p
				LEFT JOIN proposal_actions pa ON pa.proposal_id = p.id AND pa.action_type = 'Publish'
				WHERE p.id = ANY(${idsArray}::uuid[])
			`)

			const map = new Map<NormalizedUuid, ProposalWithAction>()
			for (const row of result.rows) {
				const id = normalizeUuid(row.proposal_id)
				map.set(id, {
					proposal: {
						id,
						spaceId: normalizeUuid(row.space_id),
						startTime: BigInt(row.start_time),
						endTime: BigInt(row.end_time),
						executedAt: row.executed_at ? BigInt(row.executed_at) : null,
					},
					contentUri: row.content_uri,
				})
			}
			return map
		},
		catch: (error) => new QueryError("batchGetProposalsWithPublishActions", error),
	}).pipe(
		Effect.withSpan("proposal-diff.batchGetProposalsWithPublishActions", {
			attributes: {count: proposalIds.length},
		}),
	)
}

/**
 * Get edit blob from IPFS cache.
 * Returns { data, isErrored } so callers can distinguish between
 * "not cached" (no row), "cached but decode failed" (row with is_errored=true),
 * and "cached successfully" (row with data).
 */
function getIpfsCacheData(
	db: Database,
	uri: string,
): Effect.Effect<{data: Buffer | null; isErrored: boolean} | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<{data: Buffer | null; is_errored: boolean}>(sql`
				SELECT data, is_errored FROM ipfs_cache WHERE uri = ${uri} LIMIT 1
			`)

			const row = result.rows[0]
			if (!row) return null
			return {data: row.data, isErrored: row.is_errored}
		},
		catch: (error) => new QueryError("getIpfsCacheData", error),
	}).pipe(
		Effect.withSpan("proposal-diff.getIpfsCacheData", {
			attributes: {uri},
		}),
	)
}

/**
 * Determine proposal status.
 */
function getProposalStatus(proposal: ProposalWithAction["proposal"]): ProposalStatus {
	if (proposal.executedAt !== null) {
		return "executed"
	}
	const now = BigInt(Math.floor(Date.now() / 1000))
	if (now < proposal.endTime) {
		return "active"
	}
	return "closed"
}

/**
 * Resolve a timestamp to the latest version key strictly before it.
 *
 * Used for executed proposals: the base state should be just before execution,
 * so the diff shows what the proposal changed. Using `<` (not `<=`) ensures
 * edits at the exact execution timestamp are excluded from the base.
 */
function resolveVersionKeyBeforeTimestamp(db: Database, timestamp: bigint): Effect.Effect<bigint | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<{version_key: string}>(sql`
				SELECT version_key FROM edit_versions
				WHERE created_at < to_timestamp(${timestamp.toString()}::bigint)
				ORDER BY version_key DESC
				LIMIT 1
			`)

			const row = result.rows[0]
			if (!row) {
				return null
			}

			return BigInt(row.version_key)
		},
		catch: (error) => new QueryError("resolveVersionKeyBeforeTimestamp", error),
	}).pipe(
		Effect.withSpan("proposal-diff.resolveVersionKeyBeforeTimestamp", {
			attributes: {timestamp: timestamp.toString()},
		}),
	)
}

// ============================================================================
// Relation Lookup
// ============================================================================

/**
 * Batch lookup from_entity_id for multiple relation IDs.
 * Used to find which entities are affected by updateRelation/deleteRelation/restoreRelation ops.
 * Returns a map from relation ID to from_entity_id.
 */
function batchLookupRelationEntities(
	db: Database,
	relationIds: NormalizedUuid[],
): Effect.Effect<Map<NormalizedUuid, NormalizedUuid>, QueryError> {
	if (relationIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			// Note: relationIds are derived from idToUuid() which converts Uint8Array bytes to hex.
			// This can only produce [0-9a-f-] characters, making SQL injection impossible.
			const relationIdsArray = `{${relationIds.join(",")}}`

			// Look up relations in the live table only.
			// This works for deleteRelation and updateRelation ops where the relation exists.
			// Note: restoreRelation ops won't find their relation here (it's deleted),
			// so those entities won't be included in the diff. See KNOWN_LIMITATIONS below.
			const liveResult = await db.execute<{id: string; from_entity_id: string}>(sql`
				SELECT id, from_entity_id FROM relations
				WHERE id = ANY(${relationIdsArray}::uuid[])
			`)

			const result = new Map<NormalizedUuid, NormalizedUuid>()
			for (const row of liveResult.rows) {
				result.set(normalizeUuid(row.id), normalizeUuid(row.from_entity_id))
			}

			return result
		},
		catch: (error) => new QueryError("batchLookupRelationEntities", error),
	}).pipe(
		Effect.withSpan("proposal-diff.batchLookupRelationEntities", {
			attributes: {relationCount: relationIds.length},
		}),
	)
}

// ============================================================================
// Batch State Fetching
// ============================================================================

// Note on SQL array construction safety:
// All entity IDs used in these functions come from extractAffectedEntities(),
// which derives them from idToUuid(). The idToUuid() function converts Uint8Array
// bytes to hex, producing only [0-9a-f-] characters. This makes SQL injection
// impossible when constructing PostgreSQL array literals like `{uuid1,uuid2}`.

/**
 * Batch fetch live snapshots for multiple entities.
 * Uses 2 queries total (values + relations), not 2N.
 */
function batchGetLiveSnapshots(
	db: Database,
	entityIds: NormalizedUuid[],
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, EntitySnapshot>, QueryError> {
	if (entityIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			// Convert to array literal for PostgreSQL ANY()
			const entityIdsArray = `{${entityIds.join(",")}}`

			// Query 1: All values for all entities
			// Explicitly list columns to avoid fetching large 'embedding' column
			const valuesResult = await db.execute<Record<string, unknown>>(sql`
				SELECT entity_id, property_id, space_id, text, language, unit, boolean,
				       decimal, point, time, integer, float, bytes, date, datetime, 
				       schedule, rect
				FROM "values"
				WHERE entity_id = ANY(${entityIdsArray}::uuid[])
				AND space_id = ${spaceId}
			`)

			// Query 2: All relations for all entities (excluding BLOCKS relations,
			// which are fetched separately via batchGetLiveBlockRelationsForEntities)
			// Note: Live table uses `id` but versioned uses `relation_id` - alias for consistency
			const relationsResult = await db.execute<Record<string, unknown>>(sql`
				SELECT id AS relation_id, entity_id, type_id, from_entity_id, from_space_id, 
				       to_entity_id, to_space_id, position, space_id, verified
				FROM relations
				WHERE from_entity_id = ANY(${entityIdsArray}::uuid[])
				AND type_id != ${BLOCKS_TYPE_ID}
				AND space_id = ${spaceId}
			`)

			return groupByEntityId(entityIds, valuesResult.rows, relationsResult.rows)
		},
		catch: (error: unknown) => new QueryError("batchGetLiveSnapshots", error),
	}).pipe(
		Effect.withSpan("proposal-diff.batchGetLiveSnapshots", {
			attributes: {entityCount: entityIds.length, spaceId},
		}),
	)
}

/**
 * Batch fetch versioned snapshots for multiple entities at a specific version.
 */
function batchGetVersionedSnapshots(
	db: Database,
	entityIds: NormalizedUuid[],
	spaceId: NormalizedUuid,
	versionKey: bigint,
): Effect.Effect<Map<NormalizedUuid, EntitySnapshot>, QueryError> {
	if (entityIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			const versionKeyStr = versionKey.toString()
			// Convert to array literal for PostgreSQL ANY()
			const entityIdsArray = `{${entityIds.join(",")}}`

			// Query 1: All values at version
			// Explicitly list columns to avoid fetching large 'embedding' column
			const valuesResult = await db.execute<Record<string, unknown>>(sql`
				SELECT entity_id, property_id, space_id, text, language, unit, boolean,
				       decimal, point, time, integer, float, bytes, date, datetime,
				       schedule, rect, context_root_id, context_edge_type_id
				FROM value_versions
				WHERE entity_id = ANY(${entityIdsArray}::uuid[])
				AND space_id = ${spaceId}
				AND valid_from_key <= ${versionKeyStr}::bigint
				AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
			`)

			// Query 2: All relations at version (excluding BLOCKS relations,
			// which are fetched separately via batchGetBlockRelationsForEntities)
			const relationsResult = await db.execute<Record<string, unknown>>(sql`
				SELECT relation_id, entity_id, type_id, from_entity_id, from_space_id,
				       to_entity_id, to_space_id, position, space_id, verified,
				       context_root_id, context_edge_type_id
				FROM relation_versions
				WHERE from_entity_id = ANY(${entityIdsArray}::uuid[])
				AND type_id != ${BLOCKS_TYPE_ID}
				AND space_id = ${spaceId}
				AND valid_from_key <= ${versionKeyStr}::bigint
				AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
			`)

			return groupByEntityId(entityIds, valuesResult.rows, relationsResult.rows)
		},
		catch: (error: unknown) => new QueryError("batchGetVersionedSnapshots", error),
	}).pipe(
		Effect.withSpan("proposal-diff.batchGetVersionedSnapshots", {
			attributes: {entityCount: entityIds.length, spaceId, versionKey: versionKey.toString()},
		}),
	)
}

/**
 * Group query results by entity ID.
 * Uses shared mapValueRow and mapRelationRow functions from queries.ts.
 */
function groupByEntityId(
	entityIds: NormalizedUuid[],
	valueRows: Record<string, unknown>[],
	relationRows: Record<string, unknown>[],
): Map<NormalizedUuid, EntitySnapshot> {
	const result = new Map<NormalizedUuid, EntitySnapshot>()

	// Initialize empty snapshots for all entities
	for (const id of entityIds) {
		result.set(id, {id, values: [], relations: [], blocks: []})
	}

	// Group values by entity
	for (const row of valueRows) {
		const entityId = normalizeUuid(row.entity_id as string)
		const snapshot = result.get(entityId)
		if (snapshot) {
			snapshot.values.push(mapValueRow(row))
		}
	}

	// Group relations by entity
	// Note: BLOCKS relations are already excluded at the SQL level in both
	// batchGetLiveSnapshots and batchGetVersionedSnapshots queries.
	for (const row of relationRows) {
		const entityId = normalizeUuid(row.from_entity_id as string)
		const snapshot = result.get(entityId)
		if (snapshot) {
			snapshot.relations.push(mapRelationRow(row))
		}
	}

	return result
}

// ============================================================================
// Op Application
// ============================================================================

/**
 * Convert Id (Uint8Array) to NormalizedUuid.
 * Returns lowercase hex without dashes for consistent comparison with normalized DB output.
 */
function idToUuid(id: Id): NormalizedUuid {
	return Array.from(id)
		.map((b) => b.toString(16).padStart(2, "0"))
		.join("") as NormalizedUuid
}

/**
 * Extract affected entity IDs from ops.
 * Returns both directly-extractable entity IDs and relation IDs that need lookup.
 */
function extractAffectedEntitiesAndRelations(ops: Op[]): {
	entityIds: Set<NormalizedUuid>
	relationIdsNeedingLookup: NormalizedUuid[]
} {
	const entityIds = new Set<NormalizedUuid>()
	const relationIdsNeedingLookup: NormalizedUuid[] = []

	for (const op of ops) {
		switch (op.type) {
			case "createEntity":
			case "updateEntity":
			case "deleteEntity":
			case "restoreEntity":
				entityIds.add(idToUuid(op.id))
				break
			case "createRelation":
				// Relations affect the "from" entity
				entityIds.add(idToUuid(op.from))
				break
			case "updateRelation":
			case "deleteRelation":
			case "restoreRelation":
				// These ops only have the relation ID - we need to look up the from_entity_id
				relationIdsNeedingLookup.push(idToUuid(op.id))
				break
			case "createValueRef":
				entityIds.add(idToUuid(op.entity))
				break
		}
	}

	return {entityIds, relationIdsNeedingLookup}
}

/**
 * Extract all affected entity IDs from ops, including those requiring relation lookups.
 */
function extractAffectedEntities(db: Database, ops: Op[]): Effect.Effect<NormalizedUuid[], QueryError> {
	return Effect.gen(function* () {
		const {entityIds, relationIdsNeedingLookup} = extractAffectedEntitiesAndRelations(ops)

		// If we have relation ops that need lookup, fetch the from_entity_id for each
		if (relationIdsNeedingLookup.length > 0) {
			const relationEntityMap = yield* batchLookupRelationEntities(db, relationIdsNeedingLookup)
			for (const [_relationId, fromEntityId] of relationEntityMap) {
				entityIds.add(fromEntityId)
			}
		}

		return Array.from(entityIds)
	}).pipe(
		Effect.withSpan("proposal-diff.extractAffectedEntities", {
			attributes: {opCount: ops.length},
		}),
	)
}

/**
 * Extract value from PropertyValue based on data type.
 *
 * GRC-20 v2 decoded values have the format: {type: "text", value: "..."}
 * We need to convert this to our VersionedValue format.
 */
function propertyValueToVersionedValue(pv: {property: Id; value: unknown}, spaceId: NormalizedUuid): VersionedValue {
	const propertyId = idToUuid(pv.property)
	const value = pv.value as {type: string; value: unknown}

	const result: VersionedValue = {propertyId, spaceId}

	// GRC-20 v2 value types are lowercase
	switch (value.type) {
		case "text":
			result.text = value.value as string
			break
		case "bool":
			result.boolean = value.value as boolean
			break
		case "int64":
			result.integer = Number(value.value as bigint)
			break
		case "float64":
			result.float = value.value as number
			break
		case "decimal": {
			// Decimal has exponent and mantissa fields at the top level
			// Use string manipulation to preserve precision for large numbers (> 2^53)
			const dec = value as unknown as {exponent: number; mantissa: {type: string; value: bigint}}
			const mantissaStr = dec.mantissa.value.toString()
			const exp = dec.exponent
			if (exp >= 0) {
				// Positive exponent: append zeros
				result.decimal = mantissaStr + "0".repeat(exp)
			} else {
				// Negative exponent: insert decimal point
				const decimalPlaces = -exp
				if (mantissaStr.length <= decimalPlaces) {
					// Need leading zeros after decimal point
					result.decimal = "0." + "0".repeat(decimalPlaces - mantissaStr.length) + mantissaStr
				} else {
					// Insert decimal point within the string
					const insertPos = mantissaStr.length - decimalPlaces
					result.decimal = mantissaStr.slice(0, insertPos) + "." + mantissaStr.slice(insertPos)
				}
			}
			break
		}
		case "bytes":
			result.bytes = Buffer.from(value.value as Uint8Array).toString("base64")
			break
		case "date":
			result.date = value.value as string
			break
		case "time":
			result.time = value.value as string
			break
		case "datetime":
			result.datetime = value.value as string
			break
		case "schedule":
			result.schedule = value.value
			break
		case "point": {
			const pt = value.value as {lon: number; lat: number; alt?: number}
			result.point = pt.alt !== undefined ? `${pt.lat},${pt.lon},${pt.alt}` : `${pt.lat},${pt.lon}`
			break
		}
		case "rect": {
			const rect = value.value as {minLon: number; minLat: number; maxLon: number; maxLat: number}
			result.rect = `${rect.minLon},${rect.minLat},${rect.maxLon},${rect.maxLat}`
			break
		}
		case "embedding":
			result.embedding = value.value
			break
		default:
			// Unknown type - log warning and skip
			console.warn(`Unknown value type in GRC-20 edit: ${value.type}`)
	}

	return result
}

/**
 * Apply ops to a base snapshot to get the proposed state.
 * Returns a new snapshot with deep-copied values, relations, and blocks.
 * Does not mutate `base`, but does mutate `blocksRelationMap` when
 * createRelation ops add new BLOCKS relations.
 *
 * @param blocksRelationMap - Maps BLOCKS relation IDs to block entity IDs.
 *   Used to match deleteRelation/createRelation ops against BLOCKS relations,
 *   which are stored in `blocks` rather than `relations` on the snapshot.
 *   **Mutated** by createRelation ops that add new BLOCKS relations.
 */
function applyOpsToSnapshot(
	base: EntitySnapshot,
	ops: Op[],
	entityId: NormalizedUuid,
	spaceId: NormalizedUuid,
	blocksRelationMap: Map<NormalizedUuid, NormalizedUuid>,
): EntitySnapshot {
	// Deep copy the base snapshot
	const proposed: EntitySnapshot = {
		id: entityId,
		values: base.values.map((v) => ({...v})),
		relations: base.relations.map((r) => ({...r})),
		blocks: base.blocks.map((b) => ({
			...b,
			values: b.values.map((v) => ({...v})),
			relations: b.relations.map((r) => ({...r})),
		})),
	}

	// Create maps for efficient lookups
	const valuesMap = new Map<string, VersionedValue>()
	for (const v of proposed.values) {
		valuesMap.set(`${v.propertyId}:${v.spaceId}`, v)
	}

	const relationsMap = new Map<string, VersionedRelation>()
	for (const r of proposed.relations) {
		relationsMap.set(r.relationId, r)
	}

	// Track blocks by entity ID for efficient add/remove
	const blocksMap = new Map<NormalizedUuid, BlockSnapshot>()
	for (const b of proposed.blocks) {
		blocksMap.set(b.id, b)
	}

	// Apply ops that affect this entity
	for (const op of ops) {
		switch (op.type) {
			case "createEntity":
				if (idToUuid(op.id) === entityId) {
					// Set values from the create op
					for (const pv of op.values) {
						const newValue = propertyValueToVersionedValue(pv, spaceId)
						valuesMap.set(`${newValue.propertyId}:${spaceId}`, newValue)
					}
				}
				break

			case "updateEntity":
				if (idToUuid(op.id) === entityId) {
					// First unset values
					for (const unset of op.unset) {
						const propertyId = idToUuid(unset.property)
						valuesMap.delete(`${propertyId}:${spaceId}`)
					}
					// Then set values
					for (const pv of op.set) {
						const newValue = propertyValueToVersionedValue(pv, spaceId)
						valuesMap.set(`${newValue.propertyId}:${spaceId}`, newValue)
					}
				}
				break

			case "deleteEntity":
				if (idToUuid(op.id) === entityId) {
					// Clear all values, relations, and blocks
					valuesMap.clear()
					relationsMap.clear()
					blocksMap.clear()
				}
				break

			case "restoreEntity":
				// NOT IMPLEMENTED: See KNOWN LIMITATIONS at top of file.
				// Would need historical state to know what values/relations to restore.
				break

			case "createRelation":
				if (idToUuid(op.from) === entityId) {
					const relationId = idToUuid(op.id)
					const typeId = idToUuid(op.relationType)

					if (typeId === BLOCKS_TYPE_ID) {
						// BLOCKS relation: add an empty block snapshot for the target entity.
						// The block's content (values/relations) would come from separate
						// createEntity/updateEntity ops on the block entity, but those ops
						// target the block entity ID (not this parent entity), so they aren't
						// applied here. The diff will show the block as added with whatever
						// content the block has.
						const blockEntityId = idToUuid(op.to)
						if (!blocksMap.has(blockEntityId)) {
							blocksMap.set(blockEntityId, {id: blockEntityId, values: [], relations: []})
						}
						// Track this new relation so future deleteRelation ops can find it
						blocksRelationMap.set(relationId, blockEntityId)
					} else {
						const relation: VersionedRelation = {
							relationId,
							typeId,
							fromEntityId: entityId,
							fromSpaceId: op.fromSpace ? idToUuid(op.fromSpace) : null,
							toEntityId: idToUuid(op.to),
							toSpaceId: op.toSpace ? idToUuid(op.toSpace) : null,
							position: op.position ?? null,
							spaceId: spaceId,
							verified: null,
						}
						relationsMap.set(relationId, relation)
					}
				}
				break

			case "updateRelation": {
				const relationId = idToUuid(op.id)
				const existing = relationsMap.get(relationId)
				if (existing && existing.fromEntityId === entityId) {
					// Apply unsets first
					for (const field of op.unset) {
						if (field === "position") existing.position = null
						if (field === "fromSpace") existing.fromSpaceId = null
						if (field === "toSpace") existing.toSpaceId = null
					}
					// Then apply sets
					if (op.position !== undefined) existing.position = op.position
					if (op.fromSpace) existing.fromSpaceId = idToUuid(op.fromSpace)
					if (op.toSpace) existing.toSpaceId = idToUuid(op.toSpace)
				}
				// Note: updateRelation on a BLOCKS relation would change position/space,
				// but the block entity itself stays in blocksMap. No action needed.
				break
			}

			case "deleteRelation": {
				const relationId = idToUuid(op.id)

				// Check if this is a BLOCKS relation
				const blockEntityId = blocksRelationMap.get(relationId)
				if (blockEntityId) {
					// Remove the block from the proposed snapshot
					blocksMap.delete(blockEntityId)
				} else {
					// Regular relation
					const existing = relationsMap.get(relationId)
					if (existing && existing.fromEntityId === entityId) {
						relationsMap.delete(relationId)
					}
				}
				break
			}

			case "restoreRelation":
				// NOT IMPLEMENTED: See KNOWN LIMITATIONS at top of file.
				// The relation doesn't exist in live state (it was deleted), so we can't
				// look up its from_entity_id, and we don't have the relation data to restore.
				break

			case "createValueRef":
				// Value refs don't directly modify values/relations
				break
		}
	}

	// Rebuild arrays from maps
	proposed.values = Array.from(valuesMap.values())
	proposed.relations = Array.from(relationsMap.values())
	proposed.blocks = Array.from(blocksMap.values())

	return proposed
}

/**
 * Check if an entity diff is empty (no changes).
 */
function isDiffEmpty(diff: EntityDiff): boolean {
	return diff.values.length === 0 && diff.relations.length === 0 && diff.blocks.length === 0
}

/**
 * Create an empty snapshot for a new entity.
 */
function emptySnapshot(entityId: NormalizedUuid): EntitySnapshot {
	return {id: entityId, values: [], relations: [], blocks: []}
}

// ============================================================================
// Cursor Encoding
// ============================================================================

function encodeCursor(cursor: ProposalDiffCursor): string {
	return Buffer.from(JSON.stringify(cursor)).toString("base64")
}

function decodeCursor(encoded: string): ProposalDiffCursor | null {
	try {
		const json = Buffer.from(encoded, "base64").toString("utf-8")
		return JSON.parse(json) as ProposalDiffCursor
	} catch {
		return null
	}
}

// ============================================================================
// Base State Fetching Helper
// ============================================================================

/**
 * Dispatch data fetching based on proposal status and resolved base version key.
 *
 * Encapsulates the 3-way branching pattern used for fetching entity base states,
 * block relations, and block snapshots:
 * - Active proposals → fetch from live tables
 * - Non-active with no resolved version → use empty fallback
 * - Non-active with resolved version → fetch from versioned tables
 */
function fetchBaseData<T>(
	status: ProposalStatus,
	baseVersionKey: bigint | null,
	fetchLive: () => Effect.Effect<T, QueryError>,
	fetchVersioned: (versionKey: bigint) => Effect.Effect<T, QueryError>,
	empty: () => T,
): Effect.Effect<T, QueryError> {
	if (status === "active") {
		return fetchLive()
	}
	if (baseVersionKey === null) {
		return Effect.succeed(empty())
	}
	return fetchVersioned(baseVersionKey)
}

// ============================================================================
// Main Export
// ============================================================================

/**
 * Compute a paginated proposal diff.
 *
 * @param db - Database connection
 * @param proposalId - Proposal ID
 * @param spaceId - Space ID to filter values/relations
 * @param cursorStr - Optional pagination cursor
 * @param limit - Max entities per page (default 50)
 * @returns Effect that yields paginated proposal diff
 */
export function computeProposalDiff(
	db: Database,
	proposalId: NormalizedUuid,
	spaceId: NormalizedUuid,
	cursorStr?: string,
	limit = 50,
): Effect.Effect<PaginatedProposalDiff, ProposalDiffError> {
	return Effect.gen(function* () {
		yield* Effect.logDebug("ComputeProposalDiff started").pipe(
			Effect.annotateLogs({proposalId, spaceId, limit, hasCursor: !!cursorStr}),
		)

		// 1. Get proposal with publish action
		const data = yield* getProposalWithPublishAction(db, proposalId)
		if (!data) {
			yield* Effect.logInfo("Proposal not found").pipe(Effect.annotateLogs({proposalId}))
			return yield* Effect.fail(new ProposalNotFoundError(proposalId))
		}

		const {proposal, contentUri} = data
		const status = getProposalStatus(proposal)

		yield* Effect.logDebug("Proposal loaded").pipe(
			Effect.annotateLogs({proposalId, status, hasContentUri: !!contentUri}),
		)

		// 2. Validate spaceId matches the proposal's space
		if (proposal.spaceId !== spaceId) {
			return yield* Effect.fail(new SpaceMismatchError(proposal.spaceId, spaceId))
		}

		// 3. If no publish action, return empty diff
		if (!contentUri) {
			return {
				proposalId,
				spaceId,
				proposalStatus: status,
				entities: [],
				pagination: {
					cursor: null,
					hasMore: false,
					totalEntities: 0,
				},
			}
		}

		// 4. Validate cursor format early (before expensive operations)
		let startIndex = 0
		let expectedTotalEntities: number | undefined
		if (cursorStr) {
			const cursor = decodeCursor(cursorStr)
			if (cursor === null) {
				return yield* Effect.fail(new InvalidCursorError(cursorStr))
			}
			startIndex = cursor.entityIndex
			expectedTotalEntities = cursor.totalEntities
		}

		// 5. Fetch edit blob from IPFS cache
		const cacheResult = yield* getIpfsCacheData(db, contentUri)
		if (!cacheResult) {
			return yield* Effect.fail(new EditBlobNotCachedError(contentUri))
		}
		if (cacheResult.isErrored) {
			return yield* Effect.fail(new EditBlobDecodeFailedError(contentUri))
		}
		if (!cacheResult.data) {
			return yield* Effect.fail(new EditBlobNotCachedError(contentUri))
		}
		const blob = cacheResult.data

		// 6. Decode using @geoprotocol/grc-20
		const ops = yield* Effect.tryPromise({
			try: async () => {
				const edit = await decodeEditAuto(blob)
				return edit.ops
			},
			catch: (error) => new EditDecodeError(error),
		})

		// 7. Extract affected entity IDs (sorted for stable pagination)
		// This includes looking up from_entity_id for updateRelation/deleteRelation/restoreRelation ops
		const entityIds = (yield* extractAffectedEntities(db, ops)).sort()

		yield* Effect.logDebug("Entities extracted").pipe(
			Effect.annotateLogs({proposalId, opCount: ops.length, entityCount: entityIds.length}),
		)

		// 8. Validate cursor consistency. If the entity set changed between pages
		// (proposal edited, blob re-uploaded), continuing with a stale cursor
		// would silently return inconsistent results. Fail loudly so the client
		// re-fetches page 1 and acquires a fresh cursor.
		if (expectedTotalEntities !== undefined && expectedTotalEntities !== entityIds.length) {
			yield* Effect.logWarning("Cursor drift detected (entity count changed between pages)").pipe(
				Effect.annotateLogs({proposalId, expected: expectedTotalEntities, actual: entityIds.length}),
			)
			return yield* Effect.fail(new InvalidCursorError(cursorStr ?? ""))
		}

		// 9. Paginate using the already-validated cursor
		const pageEntityIds = entityIds.slice(startIndex, startIndex + limit)

		// 10. Resolve base version key once for non-active proposals.
		// Used for both entity base states and block relation/snapshot fetching.
		// For executed proposals, use the state just before execution so the diff shows
		// what the proposal changed. For closed (not executed) proposals, use end_time.
		// Using end_time for executed proposals would include the execution's own edits
		// in the base state, producing an empty diff.
		let baseVersionKey: bigint | null = null
		if (status !== "active") {
			const baseTimestamp = proposal.executedAt ?? proposal.endTime
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, baseTimestamp)
		}

		// 11. Batch fetch base states (values and relations for affected entities)
		// Note: For active proposals, we fetch current live state. There's a potential race
		// condition if live state changes between fetching the proposal and fetching values/relations,
		// but this is acceptable - the diff represents "what would change if executed right now"
		// rather than "what would change relative to a fixed point in time".
		const baseStates = yield* fetchBaseData(
			status,
			baseVersionKey,
			() => batchGetLiveSnapshots(db, pageEntityIds, spaceId),
			(vk) => batchGetVersionedSnapshots(db, pageEntityIds, spaceId, vk),
			() => {
				const m = new Map<NormalizedUuid, EntitySnapshot>()
				for (const id of pageEntityIds) m.set(id, emptySnapshot(id))
				return m
			},
		)

		// 12. Discover block relations and populate blocks on base state snapshots.
		// This step fetches BLOCKS relations for each page entity, then batch-fetches
		// the block snapshots (values + relations). The relation IDs are needed so
		// applyOpsToSnapshot can match deleteRelation ops against BLOCKS relations.
		const blockRelationsMap = yield* fetchBaseData(
			status,
			baseVersionKey,
			() => batchGetLiveBlockRelationsForEntities(db, pageEntityIds, spaceId),
			(vk) => batchGetBlockRelationsForEntities(db, pageEntityIds, vk, spaceId),
			() => {
				const m = new Map<NormalizedUuid, BlockRelationEntry[]>()
				for (const id of pageEntityIds) m.set(id, [])
				return m
			},
		)

		// Collect all unique block entity IDs across all page entities
		const allBlockIds = new Set<NormalizedUuid>()
		for (const entries of blockRelationsMap.values()) {
			for (const entry of entries) {
				allBlockIds.add(entry.blockEntityId)
			}
		}

		// Batch fetch block snapshots
		const blockIdsList = Array.from(allBlockIds)
		let blockSnapshotsMap: Map<NormalizedUuid, BlockSnapshot>
		if (blockIdsList.length === 0) {
			blockSnapshotsMap = new Map()
		} else {
			const blockSnapshots = yield* fetchBaseData(
				status,
				baseVersionKey,
				() => batchGetLiveBlockSnapshots(db, blockIdsList, spaceId),
				(vk) => batchGetBlockSnapshotsAtVersion(db, blockIdsList, vk, spaceId),
				() => [] as BlockSnapshot[],
			)
			blockSnapshotsMap = new Map(blockSnapshots.map((b) => [b.id, b]))
		}

		// Attach blocks to each entity's base state and build the relation-to-block mapping.
		// The blocksRelationMap is shared across all entities, but this is safe because each
		// entity's blocksMap in applyOpsToSnapshot is scoped per-entity. The shared map only
		// provides a lookup from relation ID to block entity ID for matching deleteRelation ops.
		const blocksRelationMap = new Map<NormalizedUuid, NormalizedUuid>()
		for (const entityId of pageEntityIds) {
			const entries = blockRelationsMap.get(entityId) ?? []
			const baseState = baseStates.get(entityId)
			if (baseState) {
				baseState.blocks = entries
					.map((entry) => {
						blocksRelationMap.set(entry.relationId, entry.blockEntityId)
						return blockSnapshotsMap.get(entry.blockEntityId)
					})
					.filter((b): b is BlockSnapshot => b !== undefined)
			}
		}

		// 13. Compute diffs (in-memory, no DB calls)
		const diffs: EntityDiff[] = []
		for (const entityId of pageEntityIds) {
			const baseState = baseStates.get(entityId) ?? emptySnapshot(entityId)
			const proposedState = applyOpsToSnapshot(baseState, ops, entityId, spaceId, blocksRelationMap)
			const diff = yield* diffEntitySnapshots(entityId, baseState, proposedState)
			if (!isDiffEmpty(diff)) {
				diffs.push(diff)
			}
		}

		yield* Effect.logDebug("Diffs computed").pipe(
			Effect.annotateLogs({proposalId, pageSize: pageEntityIds.length, diffCount: diffs.length}),
		)

		// 14. Build pagination info
		const nextIndex = startIndex + limit
		const hasMore = nextIndex < entityIds.length

		yield* Effect.logInfo("ComputeProposalDiff completed").pipe(
			Effect.annotateLogs({
				proposalId,
				status,
				totalEntities: entityIds.length,
				pageEntities: pageEntityIds.length,
				diffsReturned: diffs.length,
				hasMore,
			}),
		)

		return {
			proposalId,
			spaceId,
			proposalStatus: status,
			entities: diffs,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex, totalEntities: entityIds.length}) : null,
				hasMore,
				totalEntities: entityIds.length,
			},
		}
	}).pipe(
		Effect.withSpan("proposal-diff.computeProposalDiff", {
			attributes: {proposalId, spaceId, limit},
		}),
	)
}

// ============================================================================
// Grouped (Multi-Proposal) Diff
// ============================================================================

/** Maximum number of proposals in a single grouped diff request. */
export const MAX_GROUP_SIZE = 20

/**
 * Convert an edit's `createdAt` to seconds-since-epoch for use with
 * `resolveVersionKeyBeforeTimestamp` (which calls PostgreSQL `to_timestamp(bigint)`
 * — seconds input).
 *
 * Per the grc-20 `Edit` type, `createdAt` is microseconds since the Unix epoch:
 *   https://github.com/graphprotocol/grc-20-ts (Edit interface)
 *
 * Exported so the conversion is trivially unit-testable — getting the divisor
 * wrong (e.g. dividing by 1_000 instead of 1_000_000) silently makes historical
 * grouped-diffs return empty because the base version resolves to "now" and the
 * diff is computed against already-applied state.
 */
export function editCreatedAtToSeconds(microseconds: bigint): bigint {
	return microseconds / 1_000_000n
}

/**
 * Ordering rule for grouped-diff ops (RFC 0004): apply in edit-timestamp order
 * ascending, tiebreak by proposalId ascending. Exported so the ordering can be
 * unit-tested without wiring up ipfs + drizzle + decode.
 */
export function compareGroupedEdits(
	a: {createdAt: bigint; proposalId: NormalizedUuid},
	b: {createdAt: bigint; proposalId: NormalizedUuid},
): number {
	if (a.createdAt < b.createdAt) return -1
	if (a.createdAt > b.createdAt) return 1
	if (a.proposalId < b.proposalId) return -1
	if (a.proposalId > b.proposalId) return 1
	return 0
}

/**
 * Compute a paginated grouped proposal diff.
 *
 * Loads N proposals, fetches N edit blobs, decodes and sorts ops by
 * (edit timestamp ASC, proposal ID ASC), then runs the standard diff
 * pipeline on the concatenated op stream.
 */
export function computeGroupedProposalDiff(
	db: Database,
	proposalIds: NormalizedUuid[],
	spaceId: NormalizedUuid,
	cursorStr?: string,
	limit = 50,
): Effect.Effect<PaginatedGroupedProposalDiff, GroupedProposalDiffError> {
	return Effect.gen(function* () {
		yield* Effect.logDebug("ComputeGroupedProposalDiff started").pipe(
			Effect.annotateLogs({proposalCount: proposalIds.length, spaceId, limit, hasCursor: !!cursorStr}),
		)

		// 1. Validate group size
		if (proposalIds.length > MAX_GROUP_SIZE) {
			return yield* Effect.fail(new GroupSizeLimitError(MAX_GROUP_SIZE, proposalIds.length))
		}

		// 2. Reject duplicates
		const seen = new Set<NormalizedUuid>()
		const duplicates: string[] = []
		for (const id of proposalIds) {
			if (seen.has(id)) duplicates.push(id)
			seen.add(id)
		}
		if (duplicates.length > 0) {
			return yield* Effect.fail(new DuplicateProposalError(duplicates))
		}

		// 3. Batch-load all proposals
		const proposalsMap = yield* batchGetProposalsWithPublishActions(db, proposalIds)

		// 4. Validate all proposals exist and belong to the requested space
		for (const id of proposalIds) {
			const data = proposalsMap.get(id)
			if (!data) {
				return yield* Effect.fail(new ProposalNotFoundError(id))
			}
			if (data.proposal.spaceId !== spaceId) {
				return yield* Effect.fail(new SpaceMismatchError(spaceId, data.proposal.spaceId))
			}
			if (!data.contentUri) {
				return yield* Effect.fail(new MissingPublishActionError(id))
			}
		}

		// 5. Determine mode: all active → "active", all historical → "historical", mixed → reject
		let activeCount = 0
		let nonActiveCount = 0
		for (const id of proposalIds) {
			const data = proposalsMap.get(id)
			if (!data) continue // already validated above
			const status = getProposalStatus(data.proposal)
			if (status === "active") activeCount++
			else nonActiveCount++
		}
		if (activeCount > 0 && nonActiveCount > 0) {
			return yield* Effect.fail(new MixedModeError(activeCount, nonActiveCount))
		}
		const mode: GroupedProposalDiffMode = activeCount > 0 ? "active" : "historical"

		yield* Effect.logDebug("Group validated").pipe(Effect.annotateLogs({mode, proposalCount: proposalIds.length}))

		// 6. Validate cursor early
		let startIndex = 0
		let expectedTotalEntities: number | undefined
		if (cursorStr) {
			const cursor = decodeCursor(cursorStr)
			if (cursor === null) {
				return yield* Effect.fail(new InvalidCursorError(cursorStr))
			}
			startIndex = cursor.entityIndex
			expectedTotalEntities = cursor.totalEntities
		}

		// 7. Fetch all edit blobs in parallel
		const blobEffects = proposalIds.map((id) => {
			const data = proposalsMap.get(id)
			const contentUri = data?.contentUri ?? "" // already validated above
			return Effect.gen(function* () {
				const cacheResult = yield* getIpfsCacheData(db, contentUri)
				if (!cacheResult) {
					return yield* Effect.fail(new EditBlobNotCachedError(contentUri) as GroupedProposalDiffError)
				}
				if (cacheResult.isErrored) {
					return yield* Effect.fail(new EditBlobDecodeFailedError(contentUri) as GroupedProposalDiffError)
				}
				if (!cacheResult.data) {
					return yield* Effect.fail(new EditBlobNotCachedError(contentUri) as GroupedProposalDiffError)
				}
				return {proposalId: id, blob: cacheResult.data}
			})
		})
		const blobs = yield* Effect.all(blobEffects, {concurrency: "unbounded"})

		// 8. Decode all edits and sort by (createdAt ASC, proposalId ASC)
		const decodedEdits: {proposalId: NormalizedUuid; ops: Op[]; createdAt: bigint}[] = []
		for (const {proposalId, blob} of blobs) {
			const edit = yield* Effect.tryPromise({
				try: async () => decodeEditAuto(blob),
				catch: (error) => new EditDecodeError(error),
			})
			decodedEdits.push({
				proposalId,
				ops: edit.ops,
				createdAt: edit.createdAt,
			})
		}

		decodedEdits.sort(compareGroupedEdits)

		// Concatenate ops in sorted order
		const allOps: Op[] = decodedEdits.flatMap((e) => e.ops)

		yield* Effect.logDebug("Edits decoded and sorted").pipe(
			Effect.annotateLogs({editCount: decodedEdits.length, totalOps: allOps.length}),
		)

		// 9. Extract affected entity IDs (sorted for stable pagination)
		const entityIds = (yield* extractAffectedEntities(db, allOps)).sort()

		// 10. Validate cursor consistency. If the entity set changed between pages
		// (proposal edited, a proposal added/removed from the group, blob
		// re-uploaded), continuing with a stale cursor would silently return
		// inconsistent results. Fail loudly so the client re-fetches page 1.
		if (expectedTotalEntities !== undefined && expectedTotalEntities !== entityIds.length) {
			yield* Effect.logWarning("Cursor drift detected (entity count changed between pages)").pipe(
				Effect.annotateLogs({expected: expectedTotalEntities, actual: entityIds.length}),
			)
			return yield* Effect.fail(new InvalidCursorError(cursorStr ?? ""))
		}

		// 11. Paginate
		const pageEntityIds = entityIds.slice(startIndex, startIndex + limit)

		// 12. Resolve base version key
		// For active mode: use live state (no version key)
		// For historical mode: use versioned state before the earliest edit timestamp
		let baseVersionKey: bigint | null = null
		const firstEdit = decodedEdits[0]
		if (mode === "historical" && firstEdit) {
			const earliestTimestamp = editCreatedAtToSeconds(firstEdit.createdAt)
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, earliestTimestamp)
		}

		// Use "active" status for fetchBaseData when mode is "active", "closed" otherwise
		const fetchStatus: ProposalStatus = mode === "active" ? "active" : "closed"

		// 13. Batch fetch base states
		const baseStates = yield* fetchBaseData(
			fetchStatus,
			baseVersionKey,
			() => batchGetLiveSnapshots(db, pageEntityIds, spaceId),
			(vk) => batchGetVersionedSnapshots(db, pageEntityIds, spaceId, vk),
			() => {
				const m = new Map<NormalizedUuid, EntitySnapshot>()
				for (const id of pageEntityIds) m.set(id, emptySnapshot(id))
				return m
			},
		)

		// 14. Discover block relations and populate blocks
		const blockRelationsMap = yield* fetchBaseData(
			fetchStatus,
			baseVersionKey,
			() => batchGetLiveBlockRelationsForEntities(db, pageEntityIds, spaceId),
			(vk) => batchGetBlockRelationsForEntities(db, pageEntityIds, vk, spaceId),
			() => {
				const m = new Map<NormalizedUuid, BlockRelationEntry[]>()
				for (const id of pageEntityIds) m.set(id, [])
				return m
			},
		)

		const allBlockIds = new Set<NormalizedUuid>()
		for (const entries of blockRelationsMap.values()) {
			for (const entry of entries) {
				allBlockIds.add(entry.blockEntityId)
			}
		}

		const blockIdsList = Array.from(allBlockIds)
		let blockSnapshotsMap: Map<NormalizedUuid, BlockSnapshot>
		if (blockIdsList.length === 0) {
			blockSnapshotsMap = new Map()
		} else {
			const blockSnapshots = yield* fetchBaseData(
				fetchStatus,
				baseVersionKey,
				() => batchGetLiveBlockSnapshots(db, blockIdsList, spaceId),
				(vk) => batchGetBlockSnapshotsAtVersion(db, blockIdsList, vk, spaceId),
				() => [] as BlockSnapshot[],
			)
			blockSnapshotsMap = new Map(blockSnapshots.map((b) => [b.id, b]))
		}

		const blocksRelationMap = new Map<NormalizedUuid, NormalizedUuid>()
		for (const entityId of pageEntityIds) {
			const entries = blockRelationsMap.get(entityId) ?? []
			const baseState = baseStates.get(entityId)
			if (baseState) {
				baseState.blocks = entries
					.map((entry) => {
						blocksRelationMap.set(entry.relationId, entry.blockEntityId)
						return blockSnapshotsMap.get(entry.blockEntityId)
					})
					.filter((b): b is BlockSnapshot => b !== undefined)
			}
		}

		// 15. Compute diffs
		const diffs: EntityDiff[] = []
		for (const entityId of pageEntityIds) {
			const baseState = baseStates.get(entityId) ?? emptySnapshot(entityId)
			const proposedState = applyOpsToSnapshot(baseState, allOps, entityId, spaceId, blocksRelationMap)
			const diff = yield* diffEntitySnapshots(entityId, baseState, proposedState)
			if (!isDiffEmpty(diff)) {
				diffs.push(diff)
			}
		}

		// 16. Pagination
		const nextIndex = startIndex + limit
		const hasMore = nextIndex < entityIds.length

		yield* Effect.logInfo("ComputeGroupedProposalDiff completed").pipe(
			Effect.annotateLogs({
				mode,
				proposalCount: proposalIds.length,
				totalEntities: entityIds.length,
				pageEntities: pageEntityIds.length,
				diffsReturned: diffs.length,
				hasMore,
			}),
		)

		return {
			proposalIds,
			spaceId,
			mode,
			entities: diffs,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex, totalEntities: entityIds.length}) : null,
				hasMore,
				totalEntities: entityIds.length,
			},
		}
	}).pipe(
		Effect.withSpan("proposal-diff.computeGroupedProposalDiff", {
			attributes: {proposalCount: proposalIds.length, spaceId, limit},
		}),
	)
}
