/**
 * v2 enrichment: data-block config merge (A2).
 *
 * A data block's view / columns / sort config lives on the *reified BLOCKS
 * relation entity* (the "block relation entity"), reached via VIEW_PROPERTY /
 * SHOWN_COLUMNS / PROPERTIES relations — NOT on the data block itself. This
 * enricher discovers that config entity (the BLOCKS relation's relation_id),
 * diffs its config relations + non-headline values across versions, and folds
 * them into the parent data block's `relations` / `values` so the client sees
 * one change instead of a separate invisible entity.
 *
 * Run after enrichBlocks (which creates block.values/relations) and before
 * enrichNames (so the merged config gets names too).
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {type NormalizedUuid, normalizeUuid} from "../../utils/uuid"
import {diffRelations, diffValues} from "../diff"
import {batchGetBlockSnapshotsAtVersion, type QueryError} from "../queries"
import type {BlockSnapshot, GroupedEntityDiff} from "../types"
import {getBlockConfigEntityIds} from "./queries"

type Database = Parameters<typeof getBlockConfigEntityIds>[0]

const VIEW_PROPERTY = normalizeUuid(SystemIds.VIEW_PROPERTY)
const SHOWN_COLUMNS = normalizeUuid(SystemIds.SHOWN_COLUMNS)
const PROPERTIES = normalizeUuid(SystemIds.PROPERTIES)
const NAME_PROPERTY = normalizeUuid(SystemIds.NAME_PROPERTY)
const MARKDOWN_CONTENT = normalizeUuid(SystemIds.MARKDOWN_CONTENT)

const CONFIG_RELATION_TYPES = new Set<NormalizedUuid>([VIEW_PROPERTY, SHOWN_COLUMNS, PROPERTIES])
const emptySnap = (id: NormalizedUuid): BlockSnapshot => ({id, values: [], relations: []})

export function enrichBlockConfig(
	db: Database,
	diff: GroupedEntityDiff,
	parentId: NormalizedUuid,
	fromVersionKey: bigint | null,
	toVersionKey: bigint,
	spaceId: NormalizedUuid,
): Effect.Effect<GroupedEntityDiff, QueryError> {
	return Effect.gen(function* () {
		const dataBlockIds = diff.blocks.filter((b) => b.type === "dataBlock").map((b) => b.id)
		if (dataBlockIds.length === 0) return diff

		// Map each data block → its config entity (BLOCKS relation's relation_id),
		// checking both versions (covers added/removed/persistent blocks).
		const [toMap, fromMap] = yield* Effect.all([
			getBlockConfigEntityIds(db, parentId, dataBlockIds, toVersionKey, spaceId),
			fromVersionKey === null
				? Effect.succeed(new Map<NormalizedUuid, NormalizedUuid>())
				: getBlockConfigEntityIds(db, parentId, dataBlockIds, fromVersionKey, spaceId),
		])
		const configByBlock = new Map<NormalizedUuid, NormalizedUuid>()
		for (const [b, cfg] of fromMap) configByBlock.set(b, cfg)
		for (const [b, cfg] of toMap) configByBlock.set(b, cfg) // `to` takes precedence

		const configIds = Array.from(new Set(configByBlock.values()))
		if (configIds.length === 0) return diff

		const [beforeSnaps, afterSnaps] = yield* Effect.all([
			fromVersionKey === null
				? Effect.succeed([] as BlockSnapshot[])
				: batchGetBlockSnapshotsAtVersion(db, configIds, fromVersionKey, spaceId),
			batchGetBlockSnapshotsAtVersion(db, configIds, toVersionKey, spaceId),
		])
		const beforeById = new Map(beforeSnaps.map((s) => [s.id, s]))
		const afterById = new Map(afterSnaps.map((s) => [s.id, s]))

		const blocks = yield* Effect.forEach(diff.blocks, (block) =>
			Effect.gen(function* () {
				if (block.type !== "dataBlock") return block
				const configId = configByBlock.get(block.id)
				if (!configId) return block

				const b = beforeById.get(configId) ?? emptySnap(configId)
				const a = afterById.get(configId) ?? emptySnap(configId)

				const [configRelations, configValues] = yield* Effect.all([
					diffRelations(
						b.relations.filter((r) => CONFIG_RELATION_TYPES.has(r.typeId)),
						a.relations.filter((r) => CONFIG_RELATION_TYPES.has(r.typeId)),
					),
					diffValues(
						b.values.filter((v) => v.propertyId !== NAME_PROPERTY && v.propertyId !== MARKDOWN_CONTENT),
						a.values.filter((v) => v.propertyId !== NAME_PROPERTY && v.propertyId !== MARKDOWN_CONTENT),
					),
				])

				if (configRelations.length === 0 && configValues.length === 0) return block
				return {
					...block,
					values: [...(block.values ?? []), ...configValues],
					relations: [...(block.relations ?? []), ...configRelations],
				}
			}),
		)

		return {...diff, blocks}
	}).pipe(Effect.withSpan("enrich-v2.enrichBlockConfig"))
}
