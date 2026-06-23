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
	AffectedEntityLimitError,
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
	GroupedEntityDiff,
	GroupedEntitySnapshot,
	GroupedProposalDiffMode,
	ProposalStatus,
	RelationChange,
} from "../types"
import {enrichWithMediaUrls} from "./enrich"
import {enrichBlocks} from "./enrich-blocks"
import {enrichNames} from "./enrich-names"
import {
	type BlockParentEntry,
	type BlocksReifiedEntry,
	batchGetBlockParentsAtVersion,
	batchGetBlocksRelationsByReifiedIdAtVersion,
	batchGetLiveBlockParents,
	batchGetLiveBlocksRelationsByReifiedId,
	batchGetMediaUrls,
	batchGetMediaUrlsAtVersion,
} from "./queries"
import type {
	EntityDiffV2,
	MediaEntity,
	PaginatedGroupedProposalDiffV2,
	PaginatedProposalDiffV2,
	PaginatedReviewDiffV2,
	PaginationV2,
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
	/** entities whose IMAGE_URL the edit *unset* (so the media url is removed). */
	unsetImageUrls: Set<NormalizedUuid>
	/** reified BLOCKS-relation id (from the edit) → its parent + data block. */
	blocksReified: Map<NormalizedUuid, BlocksReifiedEntry>
}

function indexOps(ops: Op[]): OpsIndex {
	const blockParents = new Map<NormalizedUuid, Set<NormalizedUuid>>()
	const mediaTyped = new Map<NormalizedUuid, "image" | "video">()
	const proposedImageUrls = new Map<NormalizedUuid, string>()
	const unsetImageUrls = new Set<NormalizedUuid>()
	const blocksReified = new Map<NormalizedUuid, BlocksReifiedEntry>()

	const noteImageUrl = (entityId: NormalizedUuid, values: {property: unknown; value: unknown}[]) => {
		for (const pv of values) {
			if (normalizeUuid(idToUuid(pv.property as never)) !== IMAGE_URL_PROPERTY_ID) continue
			const v = pv.value as {type?: string; value?: unknown}
			if (v?.type === "text" && typeof v.value === "string") {
				// Last write wins: a set supersedes any earlier unset of this entity's url.
				proposedImageUrls.set(entityId, v.value)
				unsetImageUrls.delete(entityId)
			}
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
				// The relation's reified entity id (op.id) carries the data block's config.
				blocksReified.set(normalizeUuid(idToUuid(op.id)), {parentId: parent, dataBlockId: child})
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
			for (const u of op.unset) {
				if (normalizeUuid(idToUuid(u.property as never)) === IMAGE_URL_PROPERTY_ID) {
					const id = normalizeUuid(idToUuid(op.id))
					// Last write wins: an unset supersedes any earlier set of this entity's url.
					unsetImageUrls.add(id)
					proposedImageUrls.delete(id)
				}
			}
		}
	}
	return {blockParents, mediaTyped, proposedImageUrls, unsetImageUrls, blocksReified}
}

/** Composite key for per-(parent, block) config lookup. */
const cfgKey = (parent: NormalizedUuid, block: NormalizedUuid) => `${parent}|${block}`

/** Wrap a flat proposal EntitySnapshot as a GroupedEntitySnapshot. */
function toGrouped(s: EntitySnapshot): GroupedEntitySnapshot {
	return {id: s.id, values: s.values, relations: s.relations, blocks: s.blocks, groupKeys: [], groups: {}}
}

/** Fold a reified BLOCKS-relation (config) entity's values/relations into a data
 *  block snapshot. `enrichBlocks` then strips headline values + TYPES/BLOCKS
 *  relations, leaving the data-block config (view/columns/sort) on the block. */
function mergeConfig(block: BlockSnapshot, config: EntitySnapshot | undefined): BlockSnapshot {
	if (!config) return block
	return {
		...block,
		values: [...block.values, ...config.values],
		relations: [...block.relations, ...config.relations],
	}
}

interface RootContext {
	opsIndex: OpsIndex
	/** affected entities classified as block children (folded under a parent). */
	blockChildren: Set<NormalizedUuid>
	/** affected entities classified as media-property children (dropped + inlined). */
	mediaChildren: Set<NormalizedUuid>
	/** `${parentId}|${dataBlockId}` → its reified BLOCKS-relation (config) entity id,
	 *  when the edit touched the config. Keyed per (parent, block) because the same
	 *  data block can be embedded under multiple parents, each with its own config. */
	configByParentBlock: Map<string, NormalizedUuid>
	/** media type of DB-typed media entities (existing IMAGE/VIDEO), so a proposal
	 *  that only updates an already-typed entity's URL still inlines correctly. */
	mediaTypeById: Map<NormalizedUuid, "image" | "video">
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

		const [dbParents, dbMedia, dbReified] = yield* Effect.all([
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
			// Reified BLOCKS-relation (config) entities among the affected set.
			fetchBaseData(
				status,
				baseVersionKey,
				() => batchGetLiveBlocksRelationsByReifiedId(db, affected, spaceId),
				(vk) => batchGetBlocksRelationsByReifiedIdAtVersion(db, affected, vk, spaceId),
				() => new Map<NormalizedUuid, BlocksReifiedEntry>(),
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
		const mediaTypeById = new Map<NormalizedUuid, "image" | "video">()
		for (const [id, m] of dbMedia) mediaTypeById.set(id, m.mediaType)
		for (const id of affected) {
			if (dbMedia.has(id) || opsIndex.mediaTyped.has(id)) mediaChildren.add(id)
		}

		// Config entities: affected reified BLOCKS-relation entities (DB + edit ops).
		// They are folded into their data block and never surface on their own.
		const reified = new Map<NormalizedUuid, BlocksReifiedEntry>(dbReified)
		for (const [id, entry] of opsIndex.blocksReified) reified.set(id, entry)
		const configEntities = new Set<NormalizedUuid>()
		const configByParentBlock = new Map<string, NormalizedUuid>()
		for (const id of affected) {
			const entry = reified.get(id)
			if (!entry) continue
			configEntities.add(id)
			configByParentBlock.set(cfgKey(entry.parentId, entry.dataBlockId), id)
			// The config's data block folds under its parent; make sure both are placed.
			blockChildren.add(entry.dataBlockId)
			parentRoots.add(entry.parentId)
		}

		const roots = new Set<NormalizedUuid>()
		for (const id of affected) {
			if (!blockChildren.has(id) && !mediaChildren.has(id) && !configEntities.has(id)) roots.add(id)
		}
		// Orphan parents (a block/config changed but its parent wasn't in the affected set).
		for (const p of parentRoots) {
			if (!blockChildren.has(p) && !mediaChildren.has(p) && !configEntities.has(p)) roots.add(p)
		}

		return {
			roots: Array.from(roots).sort(),
			ctx: {opsIndex, blockChildren, mediaChildren, configByParentBlock, mediaTypeById},
		}
	})
}

/**
 * Inline media URLs that the edit set on the relation's after side.
 *
 * Covers media entities typed in the edit (`opsIndex.mediaTyped`) AND existing
 * DB-typed media entities whose IMAGE_URL the edit *updated* (`ctx.mediaTypeById`).
 * The edit-set URL is the authoritative after-side value, so it overrides any
 * base-version URL `enrichWithMediaUrls` may have inlined (which, for proposals,
 * is resolved at the base version and would otherwise be stale).
 */
function inlineProposedMedia(
	diff: GroupedEntityDiff & {relations: RelationChangeV2[]},
	ctx: RootContext,
): GroupedEntityDiff & {relations: RelationChangeV2[]} {
	const {mediaTyped, proposedImageUrls, unsetImageUrls} = ctx.opsIndex
	if (proposedImageUrls.size === 0 && unsetImageUrls.size === 0) return diff
	const apply = (r: RelationChange): RelationChangeV2 => {
		if (!r.after) return r
		const target = r.after.toEntityId
		const mt = mediaTyped.get(target) ?? ctx.mediaTypeById.get(target)
		if (!mt) return r
		const url = proposedImageUrls.get(target)
		// The edit set a new URL → it's the authoritative after-side value.
		if (url) return {...r, after: {...r.after, ...(mt === "video" ? {videoUrl: url} : {imageUrl: url})}}
		// The edit removed the URL → strip any base-version URL enrichWithMediaUrls inlined.
		const after = r.after as {imageUrl?: string | null; videoUrl?: string | null}
		if (unsetImageUrls.has(target) && (after.imageUrl || after.videoUrl)) {
			return {...r, after: {...r.after, imageUrl: undefined, videoUrl: undefined}}
		}
		return r
	}
	// Mirror enrichWithMediaUrls: rewrite top-level, grouped, and block relations.
	return {
		...diff,
		relations: diff.relations.map(apply),
		groups: Object.fromEntries(
			Object.entries(diff.groups).map(([k, items]) => [
				k,
				items.map((item) =>
					"relations" in item && item.relations ? {...item, relations: item.relations.map(apply)} : item,
				),
			]),
		) as GroupedEntityDiff["groups"],
		blocks: diff.blocks.map((block) =>
			block.relations ? {...block, relations: block.relations.map(apply)} : block,
		),
	}
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
		// Data-block config is already merged into the block snapshots (proposal path),
		// so we rely on enrichBlocks to fold it rather than the DB-only enrichBlockConfig.
		const withBlocks = yield* enrichBlocks(raw, before, after)
		const named = yield* enrichNames(db, withBlocks, spaceId)
		const withMedia = yield* enrichWithMediaUrls(db, named, {fromVersionKey: vk, toVersionKey: vk, spaceId})
		// Override base-version media URLs with the edit's proposed URLs across top-level,
		// grouped, and block relations (same structures enrichWithMediaUrls traverses).
		const inlined = inlineProposedMedia(withMedia, ctx)
		const {groups, ...rest} = inlined
		return {...rest, ...groups} as unknown as EntityDiffV2
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

		// 2b. Data-block config entities (reified BLOCKS-relation entities), keyed per
		//     (parent, block) so a block shared across parents keeps each parent's own
		//     config. Fetch each config entity's base snapshot + proposed state so the
		//     config diff folds into the data block (before = base, after = proposed).
		const pageRootsSet = new Set(pageRoots)
		const configForPage = [...ctx.configByParentBlock.entries()].filter(([key]) =>
			pageRootsSet.has(key.split("|")[0] as NormalizedUuid),
		)
		const configIds = Array.from(new Set(configForPage.map(([, cfgId]) => cfgId)))
		const configBaseById = new Map<NormalizedUuid, EntitySnapshot>()
		if (configIds.length > 0) {
			const snaps = yield* fetchBaseData(
				status,
				baseVersionKey,
				() => batchGetLiveSnapshots(db, configIds, spaceId),
				(vk) => batchGetVersionedSnapshots(db, configIds, spaceId, vk),
				() => {
					const m = new Map<NormalizedUuid, EntitySnapshot>()
					for (const id of configIds) m.set(id, emptySnapshot(id))
					return m
				},
			)
			for (const [id, s] of snaps) configBaseById.set(id, s)
		}
		// (parent|block) → before/after config snapshot.
		const configBefore = new Map<string, EntitySnapshot>()
		const configAfter = new Map<string, EntitySnapshot>()
		for (const [key, cfgId] of configForPage) {
			const base = configBaseById.get(cfgId) ?? emptySnapshot(cfgId)
			configBefore.set(key, base)
			configAfter.set(key, applyOpsToSnapshot(base, ops, cfgId, spaceId, new Map()))
		}

		// 3. Attach base blocks to roots (with config folded in); build the
		//    relation→block map for op matching.
		const blocksRelationMap = new Map<NormalizedUuid, NormalizedUuid>()
		for (const rootId of pageRoots) {
			const entries = relMap.get(rootId) ?? []
			const baseState = baseStates.get(rootId)
			if (baseState) {
				baseState.blocks = entries.map((e) => {
					blocksRelationMap.set(e.relationId, e.blockEntityId)
					const base = blockBaseById.get(e.blockEntityId) ?? {id: e.blockEntityId, values: [], relations: []}
					return mergeConfig(base, configBefore.get(cfgKey(rootId, e.blockEntityId)))
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
				const blockSnap: BlockSnapshot = {
					id: proposed.id,
					values: proposed.values,
					relations: proposed.relations,
				}
				return mergeConfig(blockSnap, configAfter.get(cfgKey(rootId, b.id)))
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

/**
 * Shared core: diff a set of GRC-20 ops against base snapshots and return the
 * v2-enriched, folded, root-paginated entity diffs + pagination.
 *
 * This is the single engine behind all three front doors — single-proposal diff,
 * grouped (multi-proposal) diff, and the review endpoint (unpublished local ops).
 * Callers supply the ops (decoded from an IPFS edit blob, the concatenation of a
 * group's edits, or a request body) plus the base selector:
 *   - `status === "active"` → diff against current live state (`baseVersionKey` ignored);
 *   - otherwise → diff against the versioned state at `baseVersionKey`.
 */
export function computeEnrichedOpsDiff(
	db: Database,
	ops: Op[],
	spaceId: NormalizedUuid,
	status: ProposalStatus,
	baseVersionKey: bigint | null,
	cursorStr?: string,
	limit = 50,
	maxAffected?: number,
): Effect.Effect<{entities: EntityDiffV2[]; pagination: PaginationV2}, ProposalDiffError> {
	return Effect.gen(function* () {
		let startIndex = 0
		let expectedTotalEntities: number | undefined
		if (cursorStr) {
			const cursor = decodeCursor(cursorStr)
			if (cursor === null) return yield* Effect.fail(new InvalidCursorError(cursorStr))
			startIndex = cursor.entityIndex
			expectedTotalEntities = cursor.totalEntities
		}

		const affected = (yield* extractAffectedEntities(db, ops)).sort()
		// Bound per-request work for callers that opt in (review). Checked here, before
		// the resolveRoots batch-query fan-out runs over the full affected set.
		if (maxAffected !== undefined && affected.length > maxAffected) {
			return yield* Effect.fail(new AffectedEntityLimitError(maxAffected, affected.length))
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
			entities,
			pagination: {
				cursor: hasMore ? encodeCursor({entityIndex: nextIndex, totalEntities: roots.length}) : null,
				hasMore,
				totalEntities: roots.length,
			},
		}
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

		const cacheResult = yield* getIpfsCacheData(db, contentUri)
		if (!cacheResult) return yield* Effect.fail(new EditBlobNotCachedError(contentUri))
		if (cacheResult.isErrored) return yield* Effect.fail(new EditBlobDecodeFailedError(contentUri))
		if (!cacheResult.data) return yield* Effect.fail(new EditBlobNotCachedError(contentUri))

		const ops = yield* Effect.tryPromise({
			try: async () => (await decodeEditAuto(cacheResult.data as Uint8Array)).ops,
			catch: (error) => new EditDecodeError(error),
		})

		let baseVersionKey: bigint | null = null
		if (status !== "active") {
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, proposal.executedAt ?? proposal.endTime)
		}

		const {entities, pagination} = yield* computeEnrichedOpsDiff(
			db,
			ops,
			spaceId,
			status,
			baseVersionKey,
			cursorStr,
			limit,
		)
		return {proposalId, spaceId, proposalStatus: status, entities, pagination}
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

		let baseVersionKey: bigint | null = null
		const firstEdit = decodedEdits[0]
		if (mode === "historical" && firstEdit) {
			baseVersionKey = yield* resolveVersionKeyBeforeTimestamp(db, editCreatedAtToSeconds(firstEdit.createdAt))
		}
		const fetchStatus: ProposalStatus = mode === "active" ? "active" : "closed"

		const {entities, pagination} = yield* computeEnrichedOpsDiff(
			db,
			allOps,
			spaceId,
			fetchStatus,
			baseVersionKey,
			cursorStr,
			limit,
		)
		return {proposalIds, spaceId, mode, entities, pagination}
	}).pipe(
		Effect.withSpan("proposal-diff-v2.computeGroupedProposalDiffV2", {
			attributes: {proposalCount: proposalIds.length, spaceId, limit},
		}),
	)
}

/**
 * Max entities a single review edit may affect. Bounds the resolveRoots batch-query
 * fan-out per request (proposals are unbounded — a governance edit can legitimately
 * touch thousands — but interactive local-edit review should be small).
 */
const MAX_REVIEW_AFFECTED_ENTITIES = 500

/**
 * v2 review diff: diff a space's UNPUBLISHED local edit ops against current live
 * state, returning the same enriched `EntityDiffV2[]` shape as the proposal diff.
 *
 * Same engine as the proposal endpoints (`computeEnrichedOpsDiff`), but the ops
 * come from the request body rather than a published edit, and the base is always
 * current live state (`status = "active"`, `baseVersionKey = null`). The caller is
 * responsible for decoding the ops (e.g. `decodeEditAuto` on the encoded edit
 * blob) so review == published diff. Non-mutating.
 */
export function computeReviewDiffV2(
	db: Database,
	ops: Op[],
	spaceId: NormalizedUuid,
	cursorStr?: string,
	limit = 50,
): Effect.Effect<PaginatedReviewDiffV2, ProposalDiffError> {
	return Effect.gen(function* () {
		const {entities, pagination} = yield* computeEnrichedOpsDiff(
			db,
			ops,
			spaceId,
			"active",
			null,
			cursorStr,
			limit,
			MAX_REVIEW_AFFECTED_ENTITIES,
		)
		return {spaceId, entities, pagination}
	}).pipe(
		Effect.withSpan("proposal-diff-v2.computeReviewDiffV2", {
			attributes: {spaceId, limit, opCount: ops.length},
		}),
	)
}
