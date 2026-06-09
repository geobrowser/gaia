/**
 * v2 enrichment.
 *
 * Inlines `imageUrl` / `videoUrl` on RelationChange.before / .after when the
 * target is an IMAGE_TYPE or VIDEO_TYPE entity. Applied to relations at the top
 * level AND nested under dynamic groups.
 *
 * Before/after are resolved at their respective versions (before → `from`,
 * after → `to`) so swaps and removals show the correct historical URL on each
 * side, falling back to live state for entities with no versioned rows.
 *
 * Replaces the client-side `mediaPropertyEntityUrls` + `resolveMediaUrl` lookup
 * in `postProcessDiffs` for the property-style media case (image/video via a
 * non-BLOCKS relation).
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import type {NormalizedUuid} from "../../utils/uuid"
import type {QueryError} from "../queries"
import type {GroupedEntityDiff, RelationChange} from "../types"
import {batchGetMediaUrls, batchGetMediaUrlsAtVersion} from "./queries"
import type {MediaEntity, RelationChangeV2} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

export interface MediaEnrichVersions {
	fromVersionKey: bigint
	toVersionKey: bigint
	spaceId: NormalizedUuid
}

/** Collect target entity IDs per side, across top-level + grouped relations. */
function collectSideIds(diff: GroupedEntityDiff): {before: Set<NormalizedUuid>; after: Set<NormalizedUuid>} {
	const before = new Set<NormalizedUuid>()
	const after = new Set<NormalizedUuid>()
	const visit = (rels: RelationChange[]) => {
		for (const r of rels) {
			if (r.before?.toEntityId) before.add(r.before.toEntityId)
			if (r.after?.toEntityId) after.add(r.after.toEntityId)
		}
	}
	visit(diff.relations)
	for (const items of Object.values(diff.groups)) {
		for (const item of items) {
			if ("relations" in item && item.relations) visit(item.relations)
		}
	}
	// Rich block relations (added by enrichBlocks) can also target media entities.
	for (const block of diff.blocks) {
		if (block.relations) visit(block.relations)
	}
	return {before, after}
}

const mediaFields = (m: MediaEntity | undefined) =>
	m ? (m.mediaType === "image" ? {imageUrl: m.url} : {videoUrl: m.url}) : {}

/**
 * Enrich a grouped entity diff with media URLs on relation before/afters
 * (top-level + grouped). Before resolved at `fromVersionKey`, after at
 * `toVersionKey`, with a live fallback.
 */
export function enrichWithMediaUrls(
	db: Database,
	diff: GroupedEntityDiff,
	versions: MediaEnrichVersions,
): Effect.Effect<GroupedEntityDiff & {relations: RelationChangeV2[]}, QueryError> {
	return Effect.gen(function* () {
		const {before, after} = collectSideIds(diff)
		if (before.size === 0 && after.size === 0) {
			return diff as GroupedEntityDiff & {relations: RelationChangeV2[]}
		}

		const [fromMap, toMap] = yield* Effect.all([
			batchGetMediaUrlsAtVersion(db, Array.from(before), versions.fromVersionKey, versions.spaceId),
			batchGetMediaUrlsAtVersion(db, Array.from(after), versions.toVersionKey, versions.spaceId),
		])

		// Live fallback only for ids the versioned lookup didn't resolve (e.g. entities
		// with no versioned rows). In prod the indexer writes versioned rows, so this set
		// is usually empty and the extra live query is skipped entirely.
		const unresolved = new Set<NormalizedUuid>()
		for (const id of before) if (!fromMap.has(id)) unresolved.add(id)
		for (const id of after) if (!toMap.has(id)) unresolved.add(id)
		const liveMap =
			unresolved.size > 0 ? yield* batchGetMediaUrls(db, Array.from(unresolved), versions.spaceId) : new Map()

		const beforeMedia = (id: NormalizedUuid) => fromMap.get(id) ?? liveMap.get(id)
		const afterMedia = (id: NormalizedUuid) => toMap.get(id) ?? liveMap.get(id)

		const attach = (r: RelationChange): RelationChangeV2 => {
			const bm = r.before ? beforeMedia(r.before.toEntityId) : undefined
			const am = r.after ? afterMedia(r.after.toEntityId) : undefined
			if (!bm && !am) return r
			return {
				...r,
				before: r.before ? {...r.before, ...mediaFields(bm)} : r.before,
				after: r.after ? {...r.after, ...mediaFields(am)} : r.after,
			}
		}

		return {
			...diff,
			relations: diff.relations.map(attach),
			// Grouped relations carry the same media fields; cast since the v1
			// group-item type doesn't model the optional imageUrl/videoUrl.
			groups: Object.fromEntries(
				Object.entries(diff.groups).map(([k, items]) => [
					k,
					items.map((item) =>
						"relations" in item && item.relations ? {...item, relations: item.relations.map(attach)} : item,
					),
				]),
			) as GroupedEntityDiff["groups"],
			blocks: diff.blocks.map((block) =>
				block.relations ? {...block, relations: block.relations.map(attach)} : block,
			),
		}
	}).pipe(Effect.withSpan("enrich-v2.enrichWithMediaUrls"))
}
