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
import {type Uuid, toUuid} from "../utils/uuid"
import {diffEntitySnapshots} from "./diff"
import {mapRelationRow, mapValueRow, QueryError} from "./queries"
import type {
	EntityDiff,
	EntitySnapshot,
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

export type ProposalDiffError =
	| QueryError
	| ProposalNotFoundError
	| EditBlobNotCachedError
	| EditDecodeError
	| SpaceMismatchError
	| InvalidCursorError

type Database = NodePgDatabase<Record<string, unknown>>

// The BLOCKS relation type ID from GRC-20, normalized for comparison with DB output
const BLOCKS_TYPE_ID = toUuid(SystemIds.BLOCKS)

// ============================================================================
// Database Queries
// ============================================================================

interface ProposalWithAction {
	proposal: {
		id: Uuid
		spaceId: Uuid
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
					id: toUuid(row.proposal_id),
					spaceId: toUuid(row.space_id),
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
 * Get edit blob from IPFS cache.
 */
function getIpfsCacheData(db: Database, uri: string): Effect.Effect<Buffer | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<{data: Buffer | null}>(sql`
				SELECT data FROM ipfs_cache WHERE uri = ${uri} LIMIT 1
			`)

			const row = result.rows[0]
			return row?.data ?? null
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
 * Resolve end_time to a version key for closed proposals.
 */
function resolveVersionKeyAtTimestamp(db: Database, timestamp: bigint): Effect.Effect<bigint | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			// Find the latest edit before or at the timestamp
			const result = await db.execute<{version_key: string}>(sql`
				SELECT version_key FROM edit_versions
				WHERE created_at <= to_timestamp(${timestamp.toString()}::bigint)
				ORDER BY version_key DESC
				LIMIT 1
			`)

			const row = result.rows[0]
			if (!row) {
				return null
			}

			return BigInt(row.version_key)
		},
		catch: (error) => new QueryError("resolveVersionKeyAtTimestamp", error),
	}).pipe(
		Effect.withSpan("proposal-diff.resolveVersionKeyAtTimestamp", {
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
	relationIds: Uuid[],
): Effect.Effect<Map<Uuid, Uuid>, QueryError> {
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

			const result = new Map<Uuid, Uuid>()
			for (const row of liveResult.rows) {
				result.set(toUuid(row.id), toUuid(row.from_entity_id))
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
	entityIds: Uuid[],
	spaceId: Uuid,
): Effect.Effect<Map<Uuid, EntitySnapshot>, QueryError> {
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

			// Query 2: All relations for all entities
			// Note: Live table uses `id` but versioned uses `relation_id` - alias for consistency
			const relationsResult = await db.execute<Record<string, unknown>>(sql`
				SELECT id AS relation_id, entity_id, type_id, from_entity_id, from_space_id, 
				       to_entity_id, to_space_id, position, space_id, verified
				FROM relations
				WHERE from_entity_id = ANY(${entityIdsArray}::uuid[])
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
	entityIds: Uuid[],
	spaceId: Uuid,
	versionKey: bigint,
): Effect.Effect<Map<Uuid, EntitySnapshot>, QueryError> {
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

			// Query 2: All relations at version
			const relationsResult = await db.execute<Record<string, unknown>>(sql`
				SELECT relation_id, entity_id, type_id, from_entity_id, from_space_id,
				       to_entity_id, to_space_id, position, space_id, verified,
				       context_root_id, context_edge_type_id
				FROM relation_versions
				WHERE from_entity_id = ANY(${entityIdsArray}::uuid[])
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
	entityIds: Uuid[],
	valueRows: Record<string, unknown>[],
	relationRows: Record<string, unknown>[],
): Map<Uuid, EntitySnapshot> {
	const result = new Map<Uuid, EntitySnapshot>()

	// Initialize empty snapshots for all entities
	for (const id of entityIds) {
		result.set(id, {id, values: [], relations: [], blocks: []})
	}

	// Group values by entity
	for (const row of valueRows) {
		const entityId = toUuid(row.entity_id as string)
		const snapshot = result.get(entityId)
		if (snapshot) {
			snapshot.values.push(mapValueRow(row))
		}
	}

	// Group relations by entity (excluding block relations)
	for (const row of relationRows) {
		const entityId = toUuid(row.from_entity_id as string)
		const typeId = toUuid(row.type_id as string)
		const snapshot = result.get(entityId)
		if (snapshot && typeId !== BLOCKS_TYPE_ID) {
			snapshot.relations.push(mapRelationRow(row))
		}
	}

	return result
}

// ============================================================================
// Op Application
// ============================================================================

/**
 * Convert Id (Uint8Array) to Uuid (dashed lowercase hex).
 * Validates that the byte array is exactly 16 bytes and routes through
 * toUuid for consistent validation.
 */
function idToUuid(id: Id): Uuid {
	if (id.length !== 16) {
		throw new Error(`idToUuid: expected 16 bytes, got ${id.length}`)
	}
	const hex = Array.from(id)
		.map((b) => b.toString(16).padStart(2, "0"))
		.join("")
	return toUuid(hex)
}

/**
 * Extract affected entity IDs from ops.
 * Returns both directly-extractable entity IDs and relation IDs that need lookup.
 */
function extractAffectedEntitiesAndRelations(ops: Op[]): {
	entityIds: Set<Uuid>
	relationIdsNeedingLookup: Uuid[]
} {
	const entityIds = new Set<Uuid>()
	const relationIdsNeedingLookup: Uuid[] = []

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
function extractAffectedEntities(db: Database, ops: Op[]): Effect.Effect<Uuid[], QueryError> {
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
function propertyValueToVersionedValue(pv: {property: Id; value: unknown}, spaceId: Uuid): VersionedValue {
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
 * Returns a new snapshot (does not mutate the input).
 */
function applyOpsToSnapshot(
	base: EntitySnapshot,
	ops: Op[],
	entityId: Uuid,
	spaceId: Uuid,
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
					// Clear all values and relations
					valuesMap.clear()
					relationsMap.clear()
				}
				break

			case "restoreEntity":
				// NOT IMPLEMENTED: See KNOWN LIMITATIONS at top of file.
				// Would need historical state to know what values/relations to restore.
				break

			case "createRelation":
				if (idToUuid(op.from) === entityId) {
					const relationId = idToUuid(op.id)
					const relation: VersionedRelation = {
						relationId,
						typeId: idToUuid(op.relationType),
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
				break
			}

			case "deleteRelation": {
				const relationId = idToUuid(op.id)
				const existing = relationsMap.get(relationId)
				if (existing && existing.fromEntityId === entityId) {
					relationsMap.delete(relationId)
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
function emptySnapshot(entityId: Uuid): EntitySnapshot {
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
	proposalId: Uuid,
	spaceId: Uuid,
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
		const blob = yield* getIpfsCacheData(db, contentUri)
		if (!blob) {
			return yield* Effect.fail(new EditBlobNotCachedError(contentUri))
		}

		// 5. Decode using @geoprotocol/grc-20
		const ops = yield* Effect.tryPromise({
			try: async () => {
				const edit = await decodeEditAuto(blob)
				return edit.ops
			},
			catch: (error) => new EditDecodeError(error),
		})

		// 6. Extract affected entity IDs (sorted for stable pagination)
		// This includes looking up from_entity_id for updateRelation/deleteRelation/restoreRelation ops
		const entityIds = (yield* extractAffectedEntities(db, ops)).sort()

		yield* Effect.logDebug("Entities extracted").pipe(
			Effect.annotateLogs({proposalId, opCount: ops.length, entityCount: entityIds.length}),
		)

		// 7. Validate cursor consistency (entity count shouldn't change between pages)
		if (expectedTotalEntities !== undefined && expectedTotalEntities !== entityIds.length) {
			yield* Effect.logWarning("Entity count changed between pages").pipe(
				Effect.annotateLogs({proposalId, expected: expectedTotalEntities, actual: entityIds.length}),
			)
			// Don't fail - just log the inconsistency. The proposal may have been updated.
		}

		// 8. Paginate using the already-validated cursor
		const pageEntityIds = entityIds.slice(startIndex, startIndex + limit)

		// 8. Batch fetch base states (values and relations for affected entities)
		// Note: For active proposals, we fetch current live state. There's a potential race
		// condition if live state changes between fetching the proposal and fetching values/relations,
		// but this is acceptable - the diff represents "what would change if executed right now"
		// rather than "what would change relative to a fixed point in time".
		let baseStates: Map<Uuid, EntitySnapshot>
		if (status === "active") {
			baseStates = yield* batchGetLiveSnapshots(db, pageEntityIds, spaceId)
		} else {
			// For closed/executed proposals, use versioned state at end_time
			const versionKey = yield* resolveVersionKeyAtTimestamp(db, proposal.endTime)
			if (versionKey === null) {
				// No edits existed at that time - use empty snapshots
				baseStates = new Map()
				for (const id of pageEntityIds) {
					baseStates.set(id, emptySnapshot(id))
				}
			} else {
				baseStates = yield* batchGetVersionedSnapshots(db, pageEntityIds, spaceId, versionKey)
			}
		}

		// 9. Compute diffs (in-memory, no DB calls)
		const diffs: EntityDiff[] = []
		for (const entityId of pageEntityIds) {
			const baseState = baseStates.get(entityId) ?? emptySnapshot(entityId)
			const proposedState = applyOpsToSnapshot(baseState, ops, entityId, spaceId)
			const diff = yield* diffEntitySnapshots(entityId, baseState, proposedState)
			if (!isDiffEmpty(diff)) {
				diffs.push(diff)
			}
		}

		yield* Effect.logDebug("Diffs computed").pipe(
			Effect.annotateLogs({proposalId, pageSize: pageEntityIds.length, diffCount: diffs.length}),
		)

		// 10. Build pagination info
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
