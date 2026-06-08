/**
 * v2 enrichment.
 *
 * Single concern (for this slice): inline `imageUrl` / `videoUrl` on
 * RelationChange.before and RelationChange.after when their target entity is
 * an IMAGE_TYPE or VIDEO_TYPE entity in the live KG.
 *
 * Replaces the client-side `mediaPropertyEntityUrls` + `resolveMediaUrl`
 * lookup in `postProcessDiffs` for the property-style media case (image/video
 * referenced via a non-BLOCKS relation, not as a page block).
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import type {QueryError} from "../versioned/queries"
import type {NormalizedUuid} from "../utils/uuid"
import type {GroupedEntityDiff, RelationChange} from "../versioned/types"
import {batchGetMediaUrls} from "./queries"
import type {RelationChangeV2} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

/**
 * Walk every relation in a grouped diff and gather every distinct toEntityId
 * referenced on before/after. Includes relations under dynamic groups since
 * those can also point at media entities.
 */
function collectTargetEntityIds(diff: GroupedEntityDiff): Set<NormalizedUuid> {
	const ids = new Set<NormalizedUuid>()
	const visit = (rels: RelationChange[]) => {
		for (const r of rels) {
			if (r.before?.toEntityId) ids.add(r.before.toEntityId)
			if (r.after?.toEntityId) ids.add(r.after.toEntityId)
		}
	}
	visit(diff.relations)
	for (const items of Object.values(diff.groups)) {
		for (const item of items) {
			if ("relations" in item) visit(item.relations)
		}
	}
	return ids
}

/**
 * Enrich a single grouped entity diff with media URLs on relation before/afters.
 * Returns the same shape with the v2-extended relations.
 */
export function enrichWithMediaUrls(
	db: Database,
	diff: GroupedEntityDiff,
): Effect.Effect<GroupedEntityDiff & {relations: RelationChangeV2[]}, QueryError> {
	return Effect.gen(function* () {
		const ids = collectTargetEntityIds(diff)
		if (ids.size === 0) return diff as GroupedEntityDiff & {relations: RelationChangeV2[]}

		const mediaMap = yield* batchGetMediaUrls(db, Array.from(ids))
		if (mediaMap.size === 0) return diff as GroupedEntityDiff & {relations: RelationChangeV2[]}

		const attach = (r: RelationChange): RelationChangeV2 => {
			const beforeMedia = r.before ? mediaMap.get(r.before.toEntityId) : undefined
			const afterMedia = r.after ? mediaMap.get(r.after.toEntityId) : undefined
			if (!beforeMedia && !afterMedia) return r
			return {
				...r,
				before: r.before
					? {
							...r.before,
							...(beforeMedia
								? beforeMedia.mediaType === "image"
									? {imageUrl: beforeMedia.url}
									: {videoUrl: beforeMedia.url}
								: {}),
						}
					: r.before,
				after: r.after
					? {
							...r.after,
							...(afterMedia
								? afterMedia.mediaType === "image"
									? {imageUrl: afterMedia.url}
									: {videoUrl: afterMedia.url}
								: {}),
						}
					: r.after,
			}
		}

		return {
			...diff,
			relations: diff.relations.map(attach),
		}
	}).pipe(Effect.withSpan("enrich-v2.enrichWithMediaUrls"))
}
