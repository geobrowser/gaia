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
 * Data blocks are discovered from the before/after SNAPSHOTS, not from
 * `diff.blocks` — a config-only change (e.g. editing a table's columns/filter
 * without renaming the block) never produces a headline change, so the block is
 * absent from the flat diff. For such blocks a dataBlock entry is synthesized
 * (`before === after` name) with the config folded in.
 *
 * Run after enrichBlocks (which creates block.values/relations) and before
 * enrichNames (so the merged config gets names too).
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {type NormalizedUuid, normalizeUuid} from "../../utils/uuid"
import {diffRelations, diffValues, getBlockName, getBlockType} from "../diff"
import {batchGetBlockSnapshotsAtVersion, type QueryError} from "../queries"
import type {BlockChange, BlockSnapshot, GroupedEntityDiff, GroupedEntitySnapshot} from "../types"
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
	before: GroupedEntitySnapshot,
	after: GroupedEntitySnapshot,
	fromVersionKey: bigint | null,
	toVersionKey: bigint,
	spaceId: NormalizedUuid,
): Effect.Effect<GroupedEntityDiff, QueryError> {
	return Effect.gen(function* () {
		// Discover data blocks from the snapshots — config-only changes don't
		// appear in diff.blocks, so we can't rely on it for the candidate set.
		const beforeBlockById = new Map(before.blocks.map((b) => [b.id, b]))
		const afterBlockById = new Map(after.blocks.map((b) => [b.id, b]))
		const dataBlockIds = new Set<NormalizedUuid>()
		for (const blk of [...before.blocks, ...after.blocks]) {
			if (getBlockType(blk) === "dataBlock") dataBlockIds.add(blk.id)
		}
		if (dataBlockIds.size === 0) return diff

		const dataBlockIdList = Array.from(dataBlockIds)

		// Map each data block → its config entity (BLOCKS relation's relation_id),
		// checking both versions (covers added/removed/persistent blocks).
		const [toMap, fromMap] = yield* Effect.all([
			getBlockConfigEntityIds(db, parentId, dataBlockIdList, toVersionKey, spaceId),
			fromVersionKey === null
				? Effect.succeed(new Map<NormalizedUuid, NormalizedUuid>())
				: getBlockConfigEntityIds(db, parentId, dataBlockIdList, fromVersionKey, spaceId),
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

		// Compute the config diff per data block (only blocks with real changes).
		const configByBlockId = new Map<
			NormalizedUuid,
			{values: GroupedEntityDiff["values"]; relations: GroupedEntityDiff["relations"]}
		>()
		for (const blockId of dataBlockIds) {
			const configId = configByBlock.get(blockId)
			if (!configId) continue
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
			if (configRelations.length > 0 || configValues.length > 0) {
				configByBlockId.set(blockId, {values: configValues, relations: configRelations})
			}
		}
		if (configByBlockId.size === 0) return diff

		// Fold config into existing data-block entries.
		const blocks = diff.blocks.map((block) => {
			if (block.type !== "dataBlock") return block
			const cfg = configByBlockId.get(block.id)
			if (!cfg) return block
			return {
				...block,
				values: [...(block.values ?? []), ...cfg.values],
				relations: [...(block.relations ?? []), ...cfg.relations],
			}
		})

		// Synthesize entries for data blocks whose ONLY change is config (so they
		// were absent from diff.blocks — headline + own values/relations unchanged).
		const existingIds = new Set(diff.blocks.map((b) => b.id))
		const synthesized: BlockChange[] = []
		for (const [blockId, cfg] of configByBlockId) {
			if (existingIds.has(blockId)) continue
			const name = getBlockName(afterBlockById.get(blockId) ?? beforeBlockById.get(blockId) ?? emptySnap(blockId))
			synthesized.push({
				id: blockId,
				type: "dataBlock",
				before: name,
				after: name,
				blockName: name,
				...(cfg.values.length > 0 ? {values: cfg.values} : {}),
				...(cfg.relations.length > 0 ? {relations: cfg.relations} : {}),
			})
		}

		return {...diff, blocks: [...blocks, ...synthesized]}
	}).pipe(Effect.withSpan("enrich-v2.enrichBlockConfig"))
}
