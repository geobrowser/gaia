/**
 * v2 proposal diff — enriched, context-aware variants of the proposal endpoints.
 *
 * Mounted at /v2/versioned. Reuses v1's proposal state-construction wholesale
 * (proposal load, edit decode, base-state fetch, op application, pagination,
 * mode logic, errors) and swaps the diff+enrich tail: each affected entity is
 * diffed with the GROUPED diff and run through the same enrichment chain as the
 * entity-diff endpoint (blocks → blockConfig → names → media URLs), so relations
 * carry resolved names + media URLs and blocks carry their folded values/config.
 *
 * v1 is unchanged; this is purely additive.
 *
 * NOTE: this slice keeps v1's pagination unit (the flat affected-entity list).
 * Cross-parent block folding + media-property filtering (which require paginating
 * over root entities) are the follow-up (Track B phases 2–3).
 */

import {decodeEditAuto, type Op} from "@geoprotocol/grc-20"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import type {NormalizedUuid} from "../../utils/uuid"
import {diffGroupedEntitySnapshots} from "../diff"
import {
	applyOpsToSnapshot,
	batchGetLiveSnapshots,
	batchGetProposalsWithPublishActions,
	batchGetVersionedSnapshots,
	compareGroupedEdits,
	DuplicateProposalError,
	decodeCursor,
	EditBlobDecodeFailedError,
	EditBlobNotCachedError,
	EditDecodeError,
	editCreatedAtToSeconds,
	emptySnapshot,
	encodeCursor,
	extractAffectedEntities,
	fetchBaseData,
	type GroupedProposalDiffError,
	GroupSizeLimitError,
	getIpfsCacheData,
	getProposalStatus,
	getProposalWithPublishAction,
	InvalidCursorError,
	MAX_GROUP_SIZE,
	MissingPublishActionError,
	MixedModeError,
	type ProposalDiffError,
	ProposalNotFoundError,
	resolveVersionKeyBeforeTimestamp,
	SpaceMismatchError,
} from "../proposal-diff"
import {
	type BlockRelationEntry,
	batchGetBlockRelationsForEntities,
	batchGetBlockSnapshotsAtVersion,
	batchGetLiveBlockRelationsForEntities,
	batchGetLiveBlockSnapshots,
	type QueryError,
} from "../queries"
import type {
	BlockSnapshot,
	EntitySnapshot,
	GroupedEntitySnapshot,
	GroupedProposalDiffMode,
	ProposalStatus,
} from "../types"
import {enrichWithMediaUrls} from "./enrich"
import {enrichBlockConfig} from "./enrich-block-config"
import {enrichBlocks} from "./enrich-blocks"
import {enrichNames} from "./enrich-names"
import type {EntityDiffV2, PaginatedGroupedProposalDiffV2, PaginatedProposalDiffV2} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

/** Wrap a flat proposal EntitySnapshot as a GroupedEntitySnapshot.
 *  Proposal snapshots already separate BLOCKS children into `.blocks`; there are
 *  no dynamic (non-BLOCKS) groups in the proposal path, so `groups` is empty. */
function toGrouped(s: EntitySnapshot): GroupedEntitySnapshot {
	return {id: s.id, values: s.values, relations: s.relations, blocks: s.blocks, groupKeys: [], groups: {}}
}

/** Run the v2 enrichment chain on a single entity's before/after grouped snapshots. */
function enrichEntity(
	db: Database,
	entityId: NormalizedUuid,
	before: GroupedEntitySnapshot,
	after: GroupedEntitySnapshot,
	baseVersionKey: bigint | null,
	spaceId: NormalizedUuid,
): Effect.Effect<EntityDiffV2, QueryError> {
	return Effect.gen(function* () {
		// Proposals have no single "to" version (the proposed side is the edit
		// applied in memory). Use the base version key where a DB lookup is needed;
		// when null (active proposal) a 0 key makes the versioned lookup miss so the
		// enrichers fall back to live state.
		const vk = baseVersionKey ?? 0n
		const raw = yield* diffGroupedEntitySnapshots(entityId, before, after)
		const withBlocks = yield* enrichBlocks(raw, before, after)
		const withConfig = yield* enrichBlockConfig(
			db,
			withBlocks,
			entityId,
			before,
			after,
			baseVersionKey,
			vk,
			spaceId,
		)
		const named = yield* enrichNames(db, withConfig, spaceId)
		const withMedia = yield* enrichWithMediaUrls(db, named, {fromVersionKey: vk, toVersionKey: vk, spaceId})
		// Spread dynamic group keys at the entity level (matches DiffResponseV2).
		const {groups, ...rest} = withMedia
		return {...rest, ...groups} as unknown as EntityDiffV2
	})
}

/**
 * Shared core: given the decoded ops + the page's entity ids, build the base
 * states, apply the ops, and produce enriched grouped diffs for each entity.
 * Reuses the exact base-state/block fetching v1 uses.
 */
function buildEnrichedEntities(
	db: Database,
	ops: Op[],
	pageEntityIds: NormalizedUuid[],
	status: ProposalStatus,
	baseVersionKey: bigint | null,
	spaceId: NormalizedUuid,
): Effect.Effect<EntityDiffV2[], QueryError> {
	return Effect.gen(function* () {
		// 1. Base states (values + relations) — live / versioned / empty, per v1.
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

		// 2. Discover BLOCKS relations + block snapshots for the page entities.
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

		const allBlockIds = new Set<NormalizedUuid>()
		for (const entries of blockRelationsMap.values()) {
			for (const entry of entries) allBlockIds.add(entry.blockEntityId)
		}
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

		// 3. Attach blocks to base states; build relation→block map for op matching.
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

		// 4. Per entity: apply ops → proposed, diff (grouped), enrich.
		const out: EntityDiffV2[] = []
		for (const entityId of pageEntityIds) {
			const baseState = baseStates.get(entityId) ?? emptySnapshot(entityId)
			const proposedState = applyOpsToSnapshot(baseState, ops, entityId, spaceId, blocksRelationMap)
			// Cheap pre-check on the flat diff to skip unchanged entities before enrichment.
			const before = toGrouped(baseState)
			const after = toGrouped(proposedState)
			const enriched = yield* enrichEntity(db, entityId, before, after, baseVersionKey, spaceId)
			if (
				enriched.values.length > 0 ||
				enriched.relations.length > 0 ||
				enriched.blocks.length > 0 ||
				enriched.groupKeys.length > 0
			) {
				out.push(enriched)
			}
		}
		return out
	})
}

/** v2 single-proposal diff. */
export function computeProposalDiffV2(
	db: Database,
	proposalId: NormalizedUuid,
	spaceId: NormalizedUuid,
	cursorStr?: string,
	limit = 50,
): Effect.Effect<PaginatedProposalDiffV2, ProposalDiffError> {
	return Effect.gen(function* () {
		const data = yield* getProposalWithPublishAction(db, proposalId)
		if (!data) return yield* Effect.fail(new ProposalNotFoundError(proposalId))
		const {proposal, contentUri} = data
		const status = getProposalStatus(proposal)
		if (proposal.spaceId !== spaceId) {
			return yield* Effect.fail(new SpaceMismatchError(proposal.spaceId, spaceId))
		}
		if (!contentUri) {
			return {
				proposalId,
				spaceId,
				proposalStatus: status,
				entities: [],
				pagination: {cursor: null, hasMore: false, totalEntities: 0},
			}
		}

		let startIndex = 0
		let expectedTotalEntities: number | undefined
		if (cursorStr) {
			const cursor = decodeCursor(cursorStr)
			if (cursor === null) return yield* Effect.fail(new InvalidCursorError(cursorStr))
			startIndex = cursor.entityIndex
			expectedTotalEntities = cursor.totalEntities
		}

		const cacheResult = yield* getIpfsCacheData(db, contentUri)
		if (!cacheResult) return yield* Effect.fail(new EditBlobNotCachedError(contentUri))
		if (cacheResult.isErrored) return yield* Effect.fail(new EditBlobDecodeFailedError(contentUri))
		if (!cacheResult.data) return yield* Effect.fail(new EditBlobNotCachedError(contentUri))

		const ops = yield* Effect.tryPromise({
			try: async () => (await decodeEditAuto(cacheResult.data as Uint8Array)).ops,
			catch: (error) => new EditDecodeError(error),
		})

		const entityIds = (yield* extractAffectedEntities(db, ops)).sort()
		if (expectedTotalEntities !== undefined && expectedTotalEntities !== entityIds.length) {
			return yield* Effect.fail(new InvalidCursorError(cursorStr ?? ""))
		}
		const pageEntityIds = entityIds.slice(startIndex, startIndex + limit)

		let baseVersionKey: bigint | null = null
		if (status !== "active") {
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, proposal.executedAt ?? proposal.endTime)
		}

		const entities = yield* buildEnrichedEntities(db, ops, pageEntityIds, status, baseVersionKey, spaceId)

		const nextIndex = startIndex + limit
		const hasMore = nextIndex < entityIds.length
		return {
			proposalId,
			spaceId,
			proposalStatus: status,
			entities,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex, totalEntities: entityIds.length}) : null,
				hasMore,
				totalEntities: entityIds.length,
			},
		}
	}).pipe(Effect.withSpan("proposal-diff-v2.computeProposalDiffV2", {attributes: {proposalId, spaceId, limit}}))
}

/** v2 grouped (multi-proposal) diff. */
export function computeGroupedProposalDiffV2(
	db: Database,
	proposalIds: NormalizedUuid[],
	spaceId: NormalizedUuid,
	cursorStr?: string,
	limit = 50,
): Effect.Effect<PaginatedGroupedProposalDiffV2, GroupedProposalDiffError> {
	return Effect.gen(function* () {
		if (proposalIds.length > MAX_GROUP_SIZE) {
			return yield* Effect.fail(new GroupSizeLimitError(MAX_GROUP_SIZE, proposalIds.length))
		}
		const seen = new Set<NormalizedUuid>()
		const duplicates: string[] = []
		for (const id of proposalIds) {
			if (seen.has(id)) duplicates.push(id)
			seen.add(id)
		}
		if (duplicates.length > 0) return yield* Effect.fail(new DuplicateProposalError(duplicates))

		const proposalsMap = yield* batchGetProposalsWithPublishActions(db, proposalIds)
		for (const id of proposalIds) {
			const d = proposalsMap.get(id)
			if (!d) return yield* Effect.fail(new ProposalNotFoundError(id))
			if (d.proposal.spaceId !== spaceId)
				return yield* Effect.fail(new SpaceMismatchError(spaceId, d.proposal.spaceId))
			if (!d.contentUri) return yield* Effect.fail(new MissingPublishActionError(id))
		}

		let activeCount = 0
		let nonActiveCount = 0
		for (const id of proposalIds) {
			const d = proposalsMap.get(id)
			if (!d) continue
			if (getProposalStatus(d.proposal) === "active") activeCount++
			else nonActiveCount++
		}
		if (activeCount > 0 && nonActiveCount > 0) {
			return yield* Effect.fail(new MixedModeError(activeCount, nonActiveCount))
		}
		const mode: GroupedProposalDiffMode = activeCount > 0 ? "active" : "historical"

		let startIndex = 0
		let expectedTotalEntities: number | undefined
		if (cursorStr) {
			const cursor = decodeCursor(cursorStr)
			if (cursor === null) return yield* Effect.fail(new InvalidCursorError(cursorStr))
			startIndex = cursor.entityIndex
			expectedTotalEntities = cursor.totalEntities
		}

		const blobs = yield* Effect.all(
			proposalIds.map((id) => {
				const contentUri = proposalsMap.get(id)?.contentUri ?? ""
				return Effect.gen(function* () {
					const cacheResult = yield* getIpfsCacheData(db, contentUri)
					if (!cacheResult)
						return yield* Effect.fail(new EditBlobNotCachedError(contentUri) as GroupedProposalDiffError)
					if (cacheResult.isErrored)
						return yield* Effect.fail(new EditBlobDecodeFailedError(contentUri) as GroupedProposalDiffError)
					if (!cacheResult.data)
						return yield* Effect.fail(new EditBlobNotCachedError(contentUri) as GroupedProposalDiffError)
					return {proposalId: id, blob: cacheResult.data}
				})
			}),
			{concurrency: "unbounded"},
		)

		const decodedEdits: {proposalId: NormalizedUuid; ops: Op[]; createdAt: bigint}[] = []
		for (const {proposalId, blob} of blobs) {
			const edit = yield* Effect.tryPromise({
				try: async () => decodeEditAuto(blob),
				catch: (error) => new EditDecodeError(error),
			})
			decodedEdits.push({proposalId, ops: edit.ops, createdAt: edit.createdAt})
		}
		decodedEdits.sort(compareGroupedEdits)
		const allOps: Op[] = decodedEdits.flatMap((e) => e.ops)

		const entityIds = (yield* extractAffectedEntities(db, allOps)).sort()
		if (expectedTotalEntities !== undefined && expectedTotalEntities !== entityIds.length) {
			return yield* Effect.fail(new InvalidCursorError(cursorStr ?? ""))
		}
		const pageEntityIds = entityIds.slice(startIndex, startIndex + limit)

		let baseVersionKey: bigint | null = null
		const firstEdit = decodedEdits[0]
		if (mode === "historical" && firstEdit) {
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, editCreatedAtToSeconds(firstEdit.createdAt))
		}
		const fetchStatus: ProposalStatus = mode === "active" ? "active" : "closed"

		const entities = yield* buildEnrichedEntities(db, allOps, pageEntityIds, fetchStatus, baseVersionKey, spaceId)

		const nextIndex = startIndex + limit
		const hasMore = nextIndex < entityIds.length
		return {
			proposalIds,
			spaceId,
			mode,
			entities,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex, totalEntities: entityIds.length}) : null,
				hasMore,
				totalEntities: entityIds.length,
			},
		}
	}).pipe(
		Effect.withSpan("proposal-diff-v2.computeGroupedProposalDiffV2", {
			attributes: {proposalCount: proposalIds.length, spaceId, limit},
		}),
	)
}
