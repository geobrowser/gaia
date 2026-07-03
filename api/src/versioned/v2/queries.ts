/**
 * v2-specific queries.
 *
 * The v2 enrichment needs to know, for a given set of entity IDs, which ones
 * are image or video entities and what their URL is. This file owns that lookup.
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import {type NormalizedUuid, normalizeUuid} from "../../utils/uuid"
import {QueryError} from "../queries"
import type {MediaEntity} from "./types"

const IMAGE_TYPE_ID = normalizeUuid(SystemIds.IMAGE_TYPE)
const VIDEO_TYPE_ID = normalizeUuid(SystemIds.VIDEO_TYPE)
const IMAGE_URL_PROPERTY_ID = normalizeUuid(SystemIds.IMAGE_URL_PROPERTY)
const TYPES_PROPERTY_ID = normalizeUuid(SystemIds.TYPES_PROPERTY)
const BLOCKS_TYPE_ID = normalizeUuid(SystemIds.BLOCKS)

/** A BLOCKS relation pointing *at* a (block) entity: which parent owns it. */
export interface BlockParentEntry {
	parentId: NormalizedUuid
	relationId: NormalizedUuid
}

/** A reified BLOCKS-relation entity (the "block relation entity" that holds a
 *  data block's view/columns/sort config), resolved to its parent + data block. */
export interface BlocksReifiedEntry {
	parentId: NormalizedUuid
	dataBlockId: NormalizedUuid
}

/**
 * Resolve reified-BLOCKS-relation entities: for each candidate entity id, if it
 * is the reified `entity_id` of a BLOCKS relation, return that relation's parent
 * (`from`) and data block (`to`). Used by the proposal diff to fold a data
 * block's config (which lives on this reified entity) into the block.
 */
export function batchGetBlocksRelationsByReifiedIdAtVersion(
	db: NodePgDatabase<Record<string, unknown>>,
	reifiedIds: NormalizedUuid[],
	versionKey: bigint,
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, BlocksReifiedEntry>, QueryError> {
	if (reifiedIds.length === 0) return Effect.succeed(new Map())
	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${reifiedIds.join(",")}}`
			const vk = versionKey.toString()
			const result = await db.execute<{entity_id: string; from_entity_id: string; to_entity_id: string}>(sql`
				SELECT entity_id, from_entity_id, to_entity_id
				FROM relation_versions
				WHERE entity_id = ANY(${idsArray}::uuid[])
					AND type_id = ${BLOCKS_TYPE_ID}
					AND space_id = ${spaceId}
					AND valid_from_key <= ${vk}::bigint
					AND (valid_to_key IS NULL OR valid_to_key > ${vk}::bigint)
			`)
			const map = new Map<NormalizedUuid, BlocksReifiedEntry>()
			for (const row of result.rows) {
				map.set(normalizeUuid(row.entity_id), {
					parentId: normalizeUuid(row.from_entity_id),
					dataBlockId: normalizeUuid(row.to_entity_id),
				})
			}
			return map
		},
		catch: (error) => new QueryError("batchGetBlocksRelationsByReifiedIdAtVersion", error),
	}).pipe(
		Effect.withSpan("queries-v2.batchGetBlocksRelationsByReifiedIdAtVersion", {
			attributes: {count: reifiedIds.length},
		}),
	)
}

/** Live-state variant of {@link batchGetBlocksRelationsByReifiedIdAtVersion}. */
export function batchGetLiveBlocksRelationsByReifiedId(
	db: NodePgDatabase<Record<string, unknown>>,
	reifiedIds: NormalizedUuid[],
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, BlocksReifiedEntry>, QueryError> {
	if (reifiedIds.length === 0) return Effect.succeed(new Map())
	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${reifiedIds.join(",")}}`
			const result = await db.execute<{entity_id: string; from_entity_id: string; to_entity_id: string}>(sql`
				SELECT entity_id, from_entity_id, to_entity_id
				FROM relations
				WHERE entity_id = ANY(${idsArray}::uuid[])
					AND type_id = ${BLOCKS_TYPE_ID}
					AND space_id = ${spaceId}
			`)
			const map = new Map<NormalizedUuid, BlocksReifiedEntry>()
			for (const row of result.rows) {
				map.set(normalizeUuid(row.entity_id), {
					parentId: normalizeUuid(row.from_entity_id),
					dataBlockId: normalizeUuid(row.to_entity_id),
				})
			}
			return map
		},
		catch: (error) => new QueryError("batchGetLiveBlocksRelationsByReifiedId", error),
	}).pipe(
		Effect.withSpan("queries-v2.batchGetLiveBlocksRelationsByReifiedId", {attributes: {count: reifiedIds.length}}),
	)
}

/**
 * Backlink resolver: for each child entity id, find the parent(s) that link to
 * it via a BLOCKS relation at `versionKey`. Used by the proposal diff to fold a
 * block under its parent even when the parent itself wasn't otherwise changed
 * (e.g. editing a block's text doesn't touch the parent page).
 */
export function batchGetBlockParentsAtVersion(
	db: NodePgDatabase<Record<string, unknown>>,
	childIds: NormalizedUuid[],
	versionKey: bigint,
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, BlockParentEntry[]>, QueryError> {
	if (childIds.length === 0) return Effect.succeed(new Map())
	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${childIds.join(",")}}`
			const vk = versionKey.toString()
			const result = await db.execute<{to_entity_id: string; from_entity_id: string; relation_id: string}>(sql`
				SELECT to_entity_id, from_entity_id, relation_id
				FROM relation_versions
				WHERE to_entity_id = ANY(${idsArray}::uuid[])
					AND type_id = ${BLOCKS_TYPE_ID}
					AND space_id = ${spaceId}
					AND valid_from_key <= ${vk}::bigint
					AND (valid_to_key IS NULL OR valid_to_key > ${vk}::bigint)
			`)
			const map = new Map<NormalizedUuid, BlockParentEntry[]>()
			for (const row of result.rows) {
				const child = normalizeUuid(row.to_entity_id)
				const entry = {parentId: normalizeUuid(row.from_entity_id), relationId: normalizeUuid(row.relation_id)}
				const list = map.get(child)
				if (list) list.push(entry)
				else map.set(child, [entry])
			}
			return map
		},
		catch: (error) => new QueryError("batchGetBlockParentsAtVersion", error),
	}).pipe(Effect.withSpan("queries-v2.batchGetBlockParentsAtVersion", {attributes: {count: childIds.length}}))
}

/** Live-state variant of {@link batchGetBlockParentsAtVersion}. */
export function batchGetLiveBlockParents(
	db: NodePgDatabase<Record<string, unknown>>,
	childIds: NormalizedUuid[],
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, BlockParentEntry[]>, QueryError> {
	if (childIds.length === 0) return Effect.succeed(new Map())
	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${childIds.join(",")}}`
			const result = await db.execute<{to_entity_id: string; from_entity_id: string; id: string}>(sql`
				SELECT to_entity_id, from_entity_id, id
				FROM relations
				WHERE to_entity_id = ANY(${idsArray}::uuid[])
					AND type_id = ${BLOCKS_TYPE_ID}
					AND space_id = ${spaceId}
			`)
			const map = new Map<NormalizedUuid, BlockParentEntry[]>()
			for (const row of result.rows) {
				const child = normalizeUuid(row.to_entity_id)
				const entry = {parentId: normalizeUuid(row.from_entity_id), relationId: normalizeUuid(row.id)}
				const list = map.get(child)
				if (list) list.push(entry)
				else map.set(child, [entry])
			}
			return map
		},
		catch: (error) => new QueryError("batchGetLiveBlockParents", error),
	}).pipe(Effect.withSpan("queries-v2.batchGetLiveBlockParents", {attributes: {count: childIds.length}}))
}

/**
 * For a parent entity's BLOCKS relations at `versionKey`, map each target
 * (data) block id → the BLOCKS relation's reified-relation entity id (= the
 * "block relation entity" / config entity that holds view/columns/sort).
 */
export function getBlockConfigEntityIds(
	db: NodePgDatabase<Record<string, unknown>>,
	parentId: NormalizedUuid,
	blockIds: NormalizedUuid[],
	versionKey: bigint,
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, NormalizedUuid>, QueryError> {
	if (blockIds.length === 0) {
		return Effect.succeed(new Map())
	}
	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${blockIds.join(",")}}`
			const vk = versionKey.toString()
			const result = await db.execute<{relation_id: string; to_entity_id: string}>(sql`
				SELECT relation_id, to_entity_id
				FROM relation_versions
				WHERE from_entity_id = ${parentId}::uuid
				  AND type_id = ${BLOCKS_TYPE_ID}::uuid
				  AND to_entity_id = ANY(${idsArray}::uuid[])
				  AND space_id = ${spaceId}::uuid
				  AND valid_from_key <= ${vk}::bigint
				  AND (valid_to_key IS NULL OR valid_to_key > ${vk}::bigint)
			`)
			const out = new Map<NormalizedUuid, NormalizedUuid>()
			for (const row of result.rows) {
				out.set(normalizeUuid(row.to_entity_id), normalizeUuid(row.relation_id))
			}
			return out
		},
		catch: (error) => new QueryError("getBlockConfigEntityIds", error),
	}).pipe(Effect.withSpan("queries-v2.getBlockConfigEntityIds", {attributes: {count: blockIds.length}}))
}

/**
 * Batch-lookup media URLs for entity IDs.
 *
 * Reads from the live `values` and `relations` tables. Returns one entry per
 * entity that is typed as IMAGE_TYPE or VIDEO_TYPE AND has an IMAGE_URL_PROPERTY value.
 *
 * The live-state read mirrors `batchGetEntityNames` — close enough to "what the UI
 * would render right now" for diff preview purposes. A versioned variant can be
 * added later if a diff needs to show historical URLs.
 */
export function batchGetMediaUrls(
	db: NodePgDatabase<Record<string, unknown>>,
	entityIds: NormalizedUuid[],
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, MediaEntity>, QueryError> {
	if (entityIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${entityIds.join(",")}}`
			// Scope both the IMAGE_URL value and the TYPES relation to the request's
			// space — entity IDs are global, so an unscoped join can attach a URL or
			// type from a different space for the same entity ID.
			const result = await db.execute<{
				entity_id: string
				url: string
				type_id: string
			}>(sql`
				SELECT v.entity_id, v.text AS url, r.to_entity_id AS type_id
				FROM "values" v
				JOIN relations r
				  ON r.from_entity_id = v.entity_id
				 AND r.type_id = ${TYPES_PROPERTY_ID}::uuid
				 AND r.to_entity_id IN (${IMAGE_TYPE_ID}::uuid, ${VIDEO_TYPE_ID}::uuid)
				 AND r.space_id = ${spaceId}::uuid
				WHERE v.entity_id = ANY(${idsArray}::uuid[])
				  AND v.property_id = ${IMAGE_URL_PROPERTY_ID}::uuid
				  AND v.text IS NOT NULL
				  AND v.space_id = ${spaceId}::uuid
			`)

			const out = new Map<NormalizedUuid, MediaEntity>()
			for (const row of result.rows) {
				const entityId = normalizeUuid(row.entity_id)
				const mediaType = normalizeUuid(row.type_id) === VIDEO_TYPE_ID ? "video" : "image"
				out.set(entityId, {entityId, url: row.url, mediaType})
			}
			return out
		},
		catch: (error) => new QueryError("batchGetMediaUrls", error),
	}).pipe(
		Effect.withSpan("queries-v2.batchGetMediaUrls", {
			attributes: {count: entityIds.length},
		}),
	)
}

/**
 * Versioned variant of {@link batchGetMediaUrls}: resolves each entity's media
 * URL *as of* `versionKey`, reading `value_versions` + `relation_versions`. This
 * is what makes before/after media correct for swaps and removals — the before
 * side is read at the `from` version and the after side at the `to` version,
 * rather than always reflecting current live state.
 *
 * Callers should fall back to the live {@link batchGetMediaUrls} for IDs this
 * returns nothing for (e.g. entities with no versioned rows in test fixtures).
 */
export function batchGetMediaUrlsAtVersion(
	db: NodePgDatabase<Record<string, unknown>>,
	entityIds: NormalizedUuid[],
	versionKey: bigint,
	spaceId: NormalizedUuid,
): Effect.Effect<Map<NormalizedUuid, MediaEntity>, QueryError> {
	if (entityIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${entityIds.join(",")}}`
			const vk = versionKey.toString()
			const result = await db.execute<{
				entity_id: string
				url: string
				type_id: string
			}>(sql`
				SELECT v.entity_id, v.text AS url, r.to_entity_id AS type_id
				FROM value_versions v
				JOIN relation_versions r
				  ON r.from_entity_id = v.entity_id
				 AND r.type_id = ${TYPES_PROPERTY_ID}::uuid
				 AND r.to_entity_id IN (${IMAGE_TYPE_ID}::uuid, ${VIDEO_TYPE_ID}::uuid)
				 AND r.space_id = ${spaceId}::uuid
				 AND r.valid_from_key <= ${vk}::bigint
				 AND (r.valid_to_key IS NULL OR r.valid_to_key > ${vk}::bigint)
				WHERE v.entity_id = ANY(${idsArray}::uuid[])
				  AND v.property_id = ${IMAGE_URL_PROPERTY_ID}::uuid
				  AND v.text IS NOT NULL
				  AND v.space_id = ${spaceId}::uuid
				  AND v.valid_from_key <= ${vk}::bigint
				  AND (v.valid_to_key IS NULL OR v.valid_to_key > ${vk}::bigint)
			`)

			const out = new Map<NormalizedUuid, MediaEntity>()
			for (const row of result.rows) {
				const entityId = normalizeUuid(row.entity_id)
				const mediaType = normalizeUuid(row.type_id) === VIDEO_TYPE_ID ? "video" : "image"
				out.set(entityId, {entityId, url: row.url, mediaType})
			}
			return out
		},
		catch: (error) => new QueryError("batchGetMediaUrlsAtVersion", error),
	}).pipe(
		Effect.withSpan("queries-v2.batchGetMediaUrlsAtVersion", {
			attributes: {count: entityIds.length, versionKey: versionKey.toString()},
		}),
	)
}
