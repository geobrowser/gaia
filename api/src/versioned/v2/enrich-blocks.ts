/**
 * v2 enrichment: rich block shape.
 *
 * The flat diff emits each block as `{id, type, before, after}` (headline value
 * only). This enricher re-diffs each block entity's OWN values + relations from
 * the before/after snapshots and folds them onto the block as `values` /
 * `relations`, plus `blockName`. Lets the client render a data block's filter,
 * columns refs, collection items, etc. as one nested change instead of a
 * separate top-level entity.
 *
 * It ALSO surfaces blocks whose own values/relations changed even when the
 * headline (NAME / markdown / image URL) did NOT — the flat `diffBlocks` only
 * emits a block on a headline change, so without this those block-internal
 * changes are silently dropped from v2. Such blocks are synthesized with
 * `before === after` (headline unchanged) plus the changed `values`/`relations`.
 *
 * Headline properties are excluded from `values` (NAME → blockName + before/after,
 * MARKDOWN_CONTENT → text block content, IMAGE_URL → image block url), and the
 * TYPES relation is dropped (redundant with `type`). BLOCKS relations are already
 * excluded by the snapshot query.
 *
 * NOTE: the data-block *config* entity (view/columns/sort via VIEW_PROPERTY /
 * SHOWN_COLUMNS / PROPERTIES) lives on the BLOCKS *relation* entity, not on the
 * block itself, so it is folded in separately by enrichBlockConfig (A2).
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {type NormalizedUuid, normalizeUuid} from "../../utils/uuid"
import {diffRelations, diffValues, getBlockName, getBlockType, getImageUrl, getMarkdownContent} from "../diff"
import type {
	BlockChange,
	BlockSnapshot,
	GroupedEntityDiff,
	GroupedEntitySnapshot,
	RelationChange,
	ValueChange,
} from "../types"

const NAME_PROPERTY = normalizeUuid(SystemIds.NAME_PROPERTY)
const MARKDOWN_CONTENT = normalizeUuid(SystemIds.MARKDOWN_CONTENT)
const IMAGE_URL_PROPERTY = normalizeUuid(SystemIds.IMAGE_URL_PROPERTY)
const TYPES_PROPERTY = normalizeUuid(SystemIds.TYPES_PROPERTY)

const HEADLINE_PROPERTIES = new Set<NormalizedUuid>([NAME_PROPERTY, MARKDOWN_CONTENT, IMAGE_URL_PROPERTY])

const emptyBlock = (id: NormalizedUuid): BlockSnapshot => ({id, values: [], relations: []})

/** Re-diff a block's own values/relations, dropping headline props and the TYPES relation. */
function diffBlockInternals(
	b: BlockSnapshot,
	a: BlockSnapshot,
): Effect.Effect<{values: ValueChange[]; relations: RelationChange[]; blockName: string | null}, never> {
	return Effect.gen(function* () {
		const [allValues, allRelations] = yield* Effect.all([
			diffValues(b.values, a.values),
			diffRelations(b.relations, a.relations),
		])
		// Name from the snapshot (present even when unchanged), after preferred.
		const nameRow =
			a.values.find((v) => v.propertyId === NAME_PROPERTY) ?? b.values.find((v) => v.propertyId === NAME_PROPERTY)
		return {
			values: allValues.filter((v) => !HEADLINE_PROPERTIES.has(v.propertyId)),
			relations: allRelations.filter((r) => r.typeId !== TYPES_PROPERTY),
			blockName: nameRow?.text ?? null,
		}
	})
}

/**
 * Build a BlockChange for a block whose headline is UNCHANGED (before === after),
 * surfaced only because its own values/relations changed. Returns null for an
 * unknown block type.
 */
function synthesizeUnchangedBlock(
	id: NormalizedUuid,
	b: BlockSnapshot,
	a: BlockSnapshot,
	values: ValueChange[],
	relations: RelationChange[],
	blockName: string | null,
): BlockChange | null {
	const type = getBlockType(a) ?? getBlockType(b)
	if (!type) return null
	const rich = {
		blockName,
		...(values.length > 0 ? {values} : {}),
		...(relations.length > 0 ? {relations} : {}),
	}
	switch (type) {
		case "textBlock": {
			const content = getMarkdownContent(a)
			return {id, type, before: content, after: content, diff: [], ...rich}
		}
		case "imageBlock":
		case "videoBlock": {
			const url = getImageUrl(a)
			return {id, type, before: url, after: url, ...rich}
		}
		case "dataBlock": {
			const name = getBlockName(a)
			return {id, type, before: name, after: name, ...rich}
		}
	}
}

export function enrichBlocks(
	diff: GroupedEntityDiff,
	before: GroupedEntitySnapshot,
	after: GroupedEntitySnapshot,
): Effect.Effect<GroupedEntityDiff, never> {
	return Effect.gen(function* () {
		const beforeById = new Map(before.blocks.map((b) => [b.id, b]))
		const afterById = new Map(after.blocks.map((b) => [b.id, b]))

		// 1. Enrich blocks the flat diff already emitted (headline changed).
		const enriched = yield* Effect.forEach(diff.blocks, (block) =>
			Effect.gen(function* () {
				const b = beforeById.get(block.id) ?? emptyBlock(block.id)
				const a = afterById.get(block.id) ?? emptyBlock(block.id)
				const {values, relations, blockName} = yield* diffBlockInternals(b, a)
				return {
					...block,
					blockName,
					...(values.length > 0 ? {values} : {}),
					...(relations.length > 0 ? {relations} : {}),
				}
			}),
		)

		// 2. Surface blocks present in both snapshots whose OWN values/relations
		//    changed but whose headline did NOT (so diffBlocks skipped them).
		const inDiff = new Set(diff.blocks.map((b) => b.id))
		const candidateIds = new Set<NormalizedUuid>()
		for (const id of beforeById.keys()) if (!inDiff.has(id)) candidateIds.add(id)
		for (const id of afterById.keys()) if (!inDiff.has(id)) candidateIds.add(id)

		const synthesizedRaw = yield* Effect.forEach(Array.from(candidateIds), (id) =>
			Effect.gen(function* () {
				const b = beforeById.get(id) ?? emptyBlock(id)
				const a = afterById.get(id) ?? emptyBlock(id)
				const {values, relations, blockName} = yield* diffBlockInternals(b, a)
				if (values.length === 0 && relations.length === 0) return null
				return synthesizeUnchangedBlock(id, b, a, values, relations, blockName)
			}),
		)
		const synthesized = synthesizedRaw.filter((b): b is BlockChange => b !== null)

		return {...diff, blocks: [...enriched, ...synthesized]}
	}).pipe(Effect.withSpan("enrich-v2.enrichBlocks"))
}
