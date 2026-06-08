/**
 * v2-specific queries.
 *
 * The v2 enrichment needs to know, for a given set of entity IDs, which ones
 * are image or video entities and what their URL is. This file owns that lookup.
 */

import {SystemIds} from "@graphprotocol/grc-20"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {sql} from "drizzle-orm"
import {Effect} from "effect"
import {QueryError} from "../queries"
import {normalizeUuid, type NormalizedUuid} from "../../utils/uuid"
import type {MediaEntity} from "./types"

const IMAGE_TYPE_ID = normalizeUuid(SystemIds.IMAGE_TYPE)
const VIDEO_TYPE_ID = normalizeUuid(SystemIds.VIDEO_TYPE)
const IMAGE_URL_PROPERTY_ID = normalizeUuid(SystemIds.IMAGE_URL_PROPERTY)
const TYPES_PROPERTY_ID = normalizeUuid(SystemIds.TYPES_PROPERTY)

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
): Effect.Effect<Map<NormalizedUuid, MediaEntity>, QueryError> {
	if (entityIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			const idsArray = `{${entityIds.join(",")}}`
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
				WHERE v.entity_id = ANY(${idsArray}::uuid[])
				  AND v.property_id = ${IMAGE_URL_PROPERTY_ID}::uuid
				  AND v.text IS NOT NULL
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
