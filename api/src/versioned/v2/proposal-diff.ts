/**
 * v2 proposal diff — enriched, context-aware variants of the proposal endpoints.
 *
 * Mounted at /v2/versioned. Reuses v1's proposal state-construction wholesale
 * (proposal load, edit decode, base-state fetch, op application, mode logic,
 * errors) and replaces the diff+enrich tail with the option-A pipeline:
 *
 *   1. Resolve the *root* (renderable) entities: classify each affected entity
 *      as a block child (BLOCKS backlink, DB + edit ops) or a media-property
 *      child (IMAGE/VIDEO typed), and treat their parents as roots — including
 *      parents that weren't themselves changed (e.g. editing a block's text).
 *   2. Paginate over roots (not the flat affected list).
 *   3. Per root, fold each block child under `blocks[]` with its own proposed
 *      values/relations/config, drop media children and inline their URL onto
 *      the parent's relation, and run the shared enrichment chain
 *      (enrichBlocks → enrichBlockConfig → enrichNames → enrichWithMediaUrls).
 *
 * v1 is unchanged; this is purely additive.
 */

import {decodeEditAuto, type Op} from "@geoprotocol/grc-20"
import {SystemIds} from "@graphprotocol/grc-20"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import {type NormalizedUuid, normalizeUuid} from "../../utils/uuid"
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
	idToUuid,
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
import {
	type BlockParentEntry,
	batchGetBlockParentsAtVersion,
	batchGetLiveBlockParents,
	batchGetMediaUrls,
	batchGetMediaUrlsAtVersion,
} from "./queries"
import type {
	EntityDiffV2,
	MediaEntity,
	PaginatedGroupedProposalDiffV2,
	PaginatedProposalDiffV2,
	RelationChangeV2,
} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

const BLOCKS_TYPE_ID = normalizeUuid(SystemIds.BLOCKS)
const TYPES_PROPERTY_ID = normalizeUuid(SystemIds.TYPES_PROPERTY)
const IMAGE_TYPE_ID = normalizeUuid(SystemIds.IMAGE_TYPE)
const VIDEO_TYPE_ID = normalizeUuid(SystemIds.VIDEO_TYPE)
const IMAGE_URL_PROPERTY_ID = normalizeUuid(SystemIds.IMAGE_URL_PROPERTY)

/** What the edit's ops tell us about parentage / media that the DB can't (new entities). */
interface OpsIndex {
	/** child block id → set of parent ids (from createRelation BLOCKS ops). */
	blockParents: Map<NormalizedUuid, Set<NormalizedUuid>>
	/** entity id → media type, for entities the edit types as IMAGE/VIDEO. */
	mediaTyped: Map<NormalizedUuid, "image" | "video">
	/** entity id → IMAGE_URL set by the edit (proposed-side media url). */
	proposedImageUrls: Map<NormalizedUuid, string>
}

function indexOps(ops: Op[]): OpsIndex {
	const blockParents = new Map<NormalizedUuid, Set<NormalizedUuid>>()
	const mediaTyped = new Map<NormalizedUuid, "image" | "video">()
	const proposedImageUrls = new Map<NormalizedUuid, string>()

	const noteImageUrl = (entityId: NormalizedUuid, values: {property: unknown; value: unknown}[]) => {
		for (const pv of values) {
			if (normalizeUuid(idToUuid(pv.property as never)) !== IMAGE_URL_PROPERTY_ID) continue
			const v = pv.value as {type?: string; value?: unknown}
			if (v?.type === "text" && typeof v.value === "string") proposedImageUrls.set(entityId, v.value)
		}
	}

	for (const op of ops) {
		if (op.type === "createRelation") {
			const typeId = normalizeUuid(idToUuid(op.relationType))
			if (typeId === BLOCKS_TYPE_ID) {
				const child = normalizeUuid(idToUuid(op.to))
				const parent = normalizeUuid(idToUuid(op.from))
				const set = blockParents.get(child) ?? new Set<NormalizedUuid>()
				set.add(parent)
				blockParents.set(child, set)
			} else if (typeId === TYPES_PROPERTY_ID) {
				const to = normalizeUuid(idToUuid(op.to))
				if (to === IMAGE_TYPE_ID || to === VIDEO_TYPE_ID) {
					mediaTyped.set(normalizeUuid(idToUuid(op.from)), to === VIDEO_TYPE_ID ? "video" : "image")
				}
			}
		} else if (op.type === "createEntity") {
			noteImageUrl(normalizeUuid(idToUuid(op.id)), op.values)
		} else if (op.type === "updateEntity") {
			noteImageUrl(normalizeUuid(idToUuid(op.id)), op.set)
		}
	}
	return {blockParents, mediaTyped, proposedImageUrls}
}

/** Wrap a flat proposal EntitySnapshot as a GroupedEntitySnapshot. */
function toGrouped(s: EntitySnapshot): GroupedEntitySnapshot {
	return {id: s.id, values: s.values, relations: s.relations, blocks: s.blocks, groupKeys: [], groups: {}}
}

interface RootContext {
	opsIndex: OpsIndex
	/** affected entities classified as block children (folded under a parent). */
	blockChildren: Set<NormalizedUuid>
	/** affected entities classified as media-property children (dropped + inlined). */
	mediaChildren: Set<NormalizedUuid>
}

/**
 * Resolve the renderable root entities for a proposal's affected set, plus the
 * classification context needed to fold/drop children.
 */
function resolveRoots(
	db: Database,
	ops: Op[],
	affected: NormalizedUuid[],
	status: ProposalStatus,
	baseVersionKey: bigint | null,
	spaceId: NormalizedUuid,
): Effect.Effect<{roots: NormalizedUuid[]; ctx: RootContext}, QueryError> {
	return Effect.gen(function* () {
		const opsIndex = indexOps(ops)

		const [dbParents, dbMedia] = yield* Effect.all([
			fetchBaseData(
				status,
				baseVersionKey,
				() => batchGetLiveBlockParents(db, affected, spaceId),
				(vk) => batchGetBlockParentsAtVersion(db, affected, vk, spaceId),
				() => new Map<NormalizedUuid, BlockParentEntry[]>(),
			),
			fetchBaseData(
				status,
				baseVersionKey,
				() => batchGetMediaUrls(db, affected, spaceId),
				(vk) => batchGetMediaUrlsAtVersion(db, affected, vk, spaceId),
				() => new Map<NormalizedUuid, MediaEntity>(),
			),
		])

		// child → parents, merging the DB backlink with new BLOCKS ops from the edit.
		const childToParents = new Map<NormalizedUuid, Set<NormalizedUuid>>()
		for (const [child, entries] of dbParents) {
			childToParents.set(child, new Set(entries.map((e) => e.parentId)))
		}
		for (const [child, parents] of opsIndex.blockParents) {
			const set = childToParents.get(child) ?? new Set<NormalizedUuid>()
			for (const p of parents) set.add(p)
			childToParents.set(child, set)
		}

		const blockChildren = new Set<NormalizedUuid>()
		const parentRoots = new Set<NormalizedUuid>()
		for (const id of affected) {
			const parents = childToParents.get(id)
			if (parents && parents.size > 0) {
				blockChildren.add(id)
				for (const p of parents) parentRoots.add(p)
			}
		}

		const mediaChildren = new Set<NormalizedUuid>()
		for (const id of affected) {
			if (dbMedia.has(id) || opsIndex.mediaTyped.has(id)) mediaChildren.add(id)
		}

		const roots = new Set<NormalizedUuid>()
		for (const id of affected) {
			if (!blockChildren.has(id) && !mediaChildren.has(id)) roots.add(id)
		}
		// Orphan parents (a block changed but its parent wasn't in the affected set).
		for (const p of parentRoots) {
			if (!blockChildren.has(p) && !mediaChildren.has(p)) roots.add(p)
		}

		return {roots: Array.from(roots).sort(), ctx: {opsIndex, blockChildren, mediaChildren}}
	})
}

/** Inline proposed-side (edit-created) media URLs onto a root's relation afters. */
function inlineProposedMedia(diff: EntityDiffV2, ctx: RootContext): EntityDiffV2 {
	const {mediaTyped, proposedImageUrls} = ctx.opsIndex
	if (mediaTyped.size === 0) return diff
	const apply = (r: RelationChangeV2): RelationChangeV2 => {
		if (!r.after) return r
		const target = r.after.toEntityId
		const mt = mediaTyped.get(target)
		const url = proposedImageUrls.get(target)
		if (!mt || !url || r.after.imageUrl || r.after.videoUrl) return r
		return {...r, after: {...r.after, ...(mt === "video" ? {videoUrl: url} : {imageUrl: url})}}
	}
	return {...diff, relations: diff.relations.map(apply)}
}

/** Build the folded, enriched diff for one root entity. */
function buildRootDiff(
	db: Database,
	rootId: NormalizedUuid,
	before: GroupedEntitySnapshot,
	after: GroupedEntitySnapshot,
	baseVersionKey: bigint | null,
	spaceId: NormalizedUuid,
	ctx: RootContext,
): Effect.Effect<EntityDiffV2, QueryError> {
	return Effect.gen(function* () {
		const vk = baseVersionKey ?? 0n
		const raw = yield* diffGroupedEntitySnapshots(rootId, before, after)
		const withBlocks = yield* enrichBlocks(raw, before, after)
		const withConfig = yield* enrichBlockConfig(db, withBlocks, rootId, before, after, baseVersionKey, vk, spaceId)
		const named = yield* enrichNames(db, withConfig, spaceId)
		const withMedia = yield* enrichWithMediaUrls(db, named, {fromVersionKey: vk, toVersionKey: vk, spaceId})
		const {groups, ...rest} = withMedia
		const flattened = {...rest, ...groups} as unknown as EntityDiffV2
		return inlineProposedMedia(flattened, ctx)
	})
}

/** Build the folded, enriched entity diffs for a page of root entities. */
function buildFoldedPage(
	db: Database,
	ops: Op[],
	pageRoots: NormalizedUuid[],
	status: ProposalStatus,
	baseVersionKey: bigint | null,
	spaceId: NormalizedUuid,
	ctx: RootContext,
): Effect.Effect<EntityDiffV2[], QueryError> {
	return Effect.gen(function* () {
		if (pageRoots.length === 0) return []

		// 1. Base states (values + relations) for the roots.
		const baseStates = yield* fetchBaseData(
			status,
			baseVersionKey,
			() => batchGetLiveSnapshots(db, pageRoots, spaceId),
			(vk) => batchGetVersionedSnapshots(db, pageRoots, spaceId, vk),
			() => {
				const m = new Map<NormalizedUuid, EntitySnapshot>()
				for (const id of pageRoots) m.set(id, emptySnapshot(id))
				return m
			},
		)

		// 2. Forward BLOCKS relations for the roots (existing block children at base).
		const relMap = yield* fetchBaseData(
			status,
			baseVersionKey,
			() => batchGetLiveBlockRelationsForEntities(db, pageRoots, spaceId),
			(vk) => batchGetBlockRelationsForEntities(db, pageRoots, vk, spaceId),
			() => {
				const m = new Map<NormalizedUuid, BlockRelationEntry[]>()
				for (const id of pageRoots) m.set(id, [])
				return m
			},
		)

		// All block child ids that exist at base (to fetch their snapshots).
		const existingBlockIds = new Set<NormalizedUuid>()
		for (const entries of relMap.values()) for (const e of entries) existingBlockIds.add(e.blockEntityId)
		const existingBlockList = Array.from(existingBlockIds)
		const blockBaseById = new Map<NormalizedUuid, BlockSnapshot>()
		if (existingBlockList.length > 0) {
			const snaps = yield* fetchBaseData(
				status,
				baseVersionKey,
				() => batchGetLiveBlockSnapshots(db, existingBlockList, spaceId),
				(vk) => batchGetBlockSnapshotsAtVersion(db, existingBlockList, vk, spaceId),
				() => [] as BlockSnapshot[],
			)
			for (const b of snaps) blockBaseById.set(b.id, b)
		}

		// 3. Attach base blocks to roots; build the relation→block map for op matching.
		const blocksRelationMap = new Map<NormalizedUuid, NormalizedUuid>()
		for (const rootId of pageRoots) {
			const entries = relMap.get(rootId) ?? []
			const baseState = baseStates.get(rootId)
			if (baseState) {
				baseState.blocks = entries.map((e) => {
					blocksRelationMap.set(e.relationId, e.blockEntityId)
					return blockBaseById.get(e.blockEntityId) ?? {id: e.blockEntityId, values: [], relations: []}
				})
			}
		}

		// 4. Per root: compute proposed root + proposed block sub-snapshots, fold + enrich.
		const out: EntityDiffV2[] = []
		for (const rootId of pageRoots) {
			const baseState = baseStates.get(rootId) ?? emptySnapshot(rootId)
			const proposedRoot = applyOpsToSnapshot(baseState, ops, rootId, spaceId, blocksRelationMap)

			// Each block child's OWN proposed snapshot (applyOps with entity = block) —
			// applyOps on the parent only tracks block membership, not block content.
			const afterBlocks: BlockSnapshot[] = proposedRoot.blocks.map((b) => {
				const existing = blockBaseById.get(b.id)
				const blockBase: EntitySnapshot = existing ? {...existing, blocks: []} : emptySnapshot(b.id)
				const proposed = applyOpsToSnapshot(blockBase, ops, b.id, spaceId, new Map())
				return {id: proposed.id, values: proposed.values, relations: proposed.relations}
			})

			const before = toGrouped(baseState)
			const after = {...toGrouped(proposedRoot), blocks: afterBlocks}
			const enriched = yield* buildRootDiff(db, rootId, before, after, baseVersionKey, spaceId, ctx)
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

/** v2 single-proposal diff (folded, enriched, root-paginated). */
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

		const affected = (yield* extractAffectedEntities(db, ops)).sort()

		let baseVersionKey: bigint | null = null
		if (status !== "active") {
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, proposal.executedAt ?? proposal.endTime)
		}

		const {roots, ctx} = yield* resolveRoots(db, ops, affected, status, baseVersionKey, spaceId)
		// Pagination unit is the renderable roots, not the flat affected list.
		if (expectedTotalEntities !== undefined && expectedTotalEntities !== roots.length) {
			return yield* Effect.fail(new InvalidCursorError(cursorStr ?? ""))
		}
		const pageRoots = roots.slice(startIndex, startIndex + limit)
		const entities = yield* buildFoldedPage(db, ops, pageRoots, status, baseVersionKey, spaceId, ctx)

		const nextIndex = startIndex + limit
		const hasMore = nextIndex < roots.length
		return {
			proposalId,
			spaceId,
			proposalStatus: status,
			entities,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex, totalEntities: roots.length}) : null,
				hasMore,
				totalEntities: roots.length,
			},
		}
	}).pipe(Effect.withSpan("proposal-diff-v2.computeProposalDiffV2", {attributes: {proposalId, spaceId, limit}}))
}

/** v2 grouped (multi-proposal) diff (folded, enriched, root-paginated). */
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

		const affected = (yield* extractAffectedEntities(db, allOps)).sort()

		let baseVersionKey: bigint | null = null
		const firstEdit = decodedEdits[0]
		if (mode === "historical" && firstEdit) {
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, editCreatedAtToSeconds(firstEdit.createdAt))
		}
		const fetchStatus: ProposalStatus = mode === "active" ? "active" : "closed"

		const {roots, ctx} = yield* resolveRoots(db, allOps, affected, fetchStatus, baseVersionKey, spaceId)
		if (expectedTotalEntities !== undefined && expectedTotalEntities !== roots.length) {
			return yield* Effect.fail(new InvalidCursorError(cursorStr ?? ""))
		}
		const pageRoots = roots.slice(startIndex, startIndex + limit)
		const entities = yield* buildFoldedPage(db, allOps, pageRoots, fetchStatus, baseVersionKey, spaceId, ctx)

		const nextIndex = startIndex + limit
		const hasMore = nextIndex < roots.length
		return {
			proposalIds,
			spaceId,
			mode,
			entities,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex, totalEntities: roots.length}) : null,
				hasMore,
				totalEntities: roots.length,
			},
		}
	}).pipe(
		Effect.withSpan("proposal-diff-v2.computeGroupedProposalDiffV2", {
			attributes: {proposalCount: proposalIds.length, spaceId, limit},
		}),
	)
}
