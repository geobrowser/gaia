/**
 * Proposal diff computation.
 *
 * Computes diffs between a proposal's proposed changes and the base state.
 * - Active proposals: compare against current live state
 * - Closed proposals: compare against versioned state at end_time
 */

import {decodeEditAuto, type Id, type Op} from "@geoprotocol/grc-20"
import {SystemIds} from "@graphprotocol/grc-20"
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import {diffEntitySnapshots} from "./diff"
import {QueryError} from "./queries"
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

// The BLOCKS relation type ID from GRC-20
const BLOCKS_TYPE_ID = SystemIds.BLOCKS

// ============================================================================
// Database Queries
// ============================================================================

interface ProposalWithAction {
	proposal: {
		id: string
		spaceId: string
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
					id: row.proposal_id,
					spaceId: row.space_id,
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
// Batch State Fetching
// ============================================================================

/**
 * Batch fetch live snapshots for multiple entities.
 * Uses 2 queries total (values + relations), not 2N.
 */
function batchGetLiveSnapshots(
	db: Database,
	entityIds: string[],
	spaceId: string,
): Effect.Effect<Map<string, EntitySnapshot>, QueryError> {
	if (entityIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			// Convert to array literal for PostgreSQL ANY()
			const entityIdsArray = `{${entityIds.join(",")}}`

			// Query 1: All values for all entities
			const valuesResult = await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM "values"
				WHERE entity_id = ANY(${entityIdsArray}::uuid[])
				AND space_id = ${spaceId}
			`)

			// Query 2: All relations for all entities
			const relationsResult = await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM relations
				WHERE from_entity_id = ANY(${entityIdsArray}::uuid[])
				AND space_id = ${spaceId}
			`)

			return groupByEntityId(entityIds, valuesResult.rows, relationsResult.rows)
		},
		catch: (error) => new QueryError("batchGetLiveSnapshots", error),
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
	entityIds: string[],
	spaceId: string,
	versionKey: bigint,
): Effect.Effect<Map<string, EntitySnapshot>, QueryError> {
	if (entityIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			const versionKeyStr = versionKey.toString()
			// Convert to array literal for PostgreSQL ANY()
			const entityIdsArray = `{${entityIds.join(",")}}`

			// Query 1: All values at version
			const valuesResult = await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM value_versions
				WHERE entity_id = ANY(${entityIdsArray}::uuid[])
				AND space_id = ${spaceId}
				AND valid_from_key <= ${versionKeyStr}::bigint
				AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
			`)

			// Query 2: All relations at version
			const relationsResult = await db.execute<Record<string, unknown>>(sql`
				SELECT * FROM relation_versions
				WHERE from_entity_id = ANY(${entityIdsArray}::uuid[])
				AND space_id = ${spaceId}
				AND valid_from_key <= ${versionKeyStr}::bigint
				AND (valid_to_key IS NULL OR valid_to_key > ${versionKeyStr}::bigint)
			`)

			return groupByEntityId(entityIds, valuesResult.rows, relationsResult.rows)
		},
		catch: (error) => new QueryError("batchGetVersionedSnapshots", error),
	}).pipe(
		Effect.withSpan("proposal-diff.batchGetVersionedSnapshots", {
			attributes: {entityCount: entityIds.length, spaceId, versionKey: versionKey.toString()},
		}),
	)
}

/**
 * Group query results by entity ID.
 */
function groupByEntityId(
	entityIds: string[],
	valueRows: Record<string, unknown>[],
	relationRows: Record<string, unknown>[],
): Map<string, EntitySnapshot> {
	const result = new Map<string, EntitySnapshot>()

	// Initialize empty snapshots for all entities
	for (const id of entityIds) {
		result.set(id, {id, values: [], relations: [], blocks: []})
	}

	// Group values by entity
	for (const row of valueRows) {
		const entityId = row.entity_id as string
		const snapshot = result.get(entityId)
		if (snapshot) {
			snapshot.values.push({
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
			})
		}
	}

	// Group relations by entity (excluding block relations)
	for (const row of relationRows) {
		const entityId = row.from_entity_id as string
		const typeId = row.type_id as string
		const snapshot = result.get(entityId)
		if (snapshot && typeId !== BLOCKS_TYPE_ID) {
			snapshot.relations.push({
				relationId: row.relation_id as string,
				typeId,
				fromEntityId: entityId,
				fromSpaceId: row.from_space_id as string | null,
				toEntityId: row.to_entity_id as string,
				toSpaceId: row.to_space_id as string | null,
				position: row.position as string | null,
				spaceId: row.space_id as string,
				verified: row.verified as boolean | null,
			})
		}
	}

	return result
}

// ============================================================================
// Op Application
// ============================================================================

/**
 * Convert Id (Uint8Array) to UUID string.
 */
function idToUuid(id: Id): string {
	// Id is a 16-byte Uint8Array, convert to UUID format
	const hex = Array.from(id)
		.map((b) => b.toString(16).padStart(2, "0"))
		.join("")
	return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`
}

/**
 * Extract affected entity IDs from ops.
 */
function extractAffectedEntities(ops: Op[]): string[] {
	const entityIds = new Set<string>()

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
				// These only have the relation id, we'd need to look up the from entity
				// For now, skip - the entity changes are captured by entity ops
				break
			case "createValueRef":
				entityIds.add(idToUuid(op.entity))
				break
		}
	}

	return Array.from(entityIds)
}

/**
 * Extract value from PropertyValue based on data type.
 *
 * GRC-20 v2 decoded values have the format: {type: "text", value: "..."}
 * We need to convert this to our VersionedValue format.
 */
function propertyValueToVersionedValue(pv: {property: Id; value: unknown}, spaceId: string): VersionedValue {
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
			const dec = value as unknown as {exponent: number; mantissa: {type: string; value: bigint}}
			const mantissaValue = dec.mantissa.value
			result.decimal = (Number(mantissaValue) / 10 ** (-dec.exponent)).toString()
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
			result.point = pt.alt !== undefined
				? `${pt.lat},${pt.lon},${pt.alt}`
				: `${pt.lat},${pt.lon}`
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
function applyOpsToSnapshot(base: EntitySnapshot, ops: Op[], entityId: string, spaceId: string): EntitySnapshot {
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
				// Restore doesn't add values back - they need to be re-set
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
						spaceId,
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
				// Would need the original relation data to restore
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
function emptySnapshot(entityId: string): EntitySnapshot {
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
	proposalId: string,
	spaceId: string,
	cursorStr?: string,
	limit = 50,
): Effect.Effect<PaginatedProposalDiff, ProposalDiffError> {
	return Effect.gen(function* () {
		// 1. Get proposal with publish action
		const data = yield* getProposalWithPublishAction(db, proposalId)
		if (!data) {
			return yield* Effect.fail(new ProposalNotFoundError(proposalId))
		}

		const {proposal, contentUri} = data
		const status = getProposalStatus(proposal)

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
		if (cursorStr) {
			const cursor = decodeCursor(cursorStr)
			if (cursor === null) {
				return yield* Effect.fail(new InvalidCursorError(cursorStr))
			}
			startIndex = cursor.entityIndex
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
		const entityIds = extractAffectedEntities(ops).sort()

		// 7. Paginate using the already-validated cursor
		const pageEntityIds = entityIds.slice(startIndex, startIndex + limit)

		// 8. Batch fetch base states (values and relations for affected entities)
		let baseStates: Map<string, EntitySnapshot>
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

		// 10. Build pagination info
		const nextIndex = startIndex + limit
		const hasMore = nextIndex < entityIds.length

		return {
			proposalId,
			spaceId,
			proposalStatus: status,
			entities: diffs,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex}) : null,
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
