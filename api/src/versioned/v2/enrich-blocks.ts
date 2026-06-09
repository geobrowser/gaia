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
 * Headline properties are excluded from `values` (NAME → blockName + before/after,
 * MARKDOWN_CONTENT → text block content, IMAGE_URL → image block url), and the
 * TYPES relation is dropped (redundant with `type`). BLOCKS relations are already
 * excluded by the snapshot query.
 *
 * NOTE: the data-block *config* entity (view/columns/sort via VIEW_PROPERTY /
 * SHOWN_COLUMNS / PROPERTIES) lives on the BLOCKS *relation* entity, not on the
 * block itself, so it is not folded in here yet — see A2.
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {diffRelations, diffValues} from "../diff"
import {normalizeUuid, type NormalizedUuid} from "../../utils/uuid"
import type {BlockSnapshot, GroupedEntityDiff, GroupedEntitySnapshot} from "../types"

const NAME_PROPERTY = normalizeUuid(SystemIds.NAME_PROPERTY)
const MARKDOWN_CONTENT = normalizeUuid(SystemIds.MARKDOWN_CONTENT)
const IMAGE_URL_PROPERTY = normalizeUuid(SystemIds.IMAGE_URL_PROPERTY)
const TYPES_PROPERTY = normalizeUuid(SystemIds.TYPES_PROPERTY)

const HEADLINE_PROPERTIES = new Set<NormalizedUuid>([NAME_PROPERTY, MARKDOWN_CONTENT, IMAGE_URL_PROPERTY])

const emptyBlock = (id: NormalizedUuid): BlockSnapshot => ({id, values: [], relations: []})

export function enrichBlocks(
	diff: GroupedEntityDiff,
	before: GroupedEntitySnapshot,
	after: GroupedEntitySnapshot,
): Effect.Effect<GroupedEntityDiff, never> {
	return Effect.gen(function* () {
		if (diff.blocks.length === 0) return diff

		const beforeById = new Map(before.blocks.map((b) => [b.id, b]))
		const afterById = new Map(after.blocks.map((b) => [b.id, b]))

		const richBlocks = yield* Effect.forEach(diff.blocks, (block) =>
			Effect.gen(function* () {
				const b = beforeById.get(block.id) ?? emptyBlock(block.id)
				const a = afterById.get(block.id) ?? emptyBlock(block.id)

				const [allValues, allRelations] = yield* Effect.all([
					diffValues(b.values, a.values),
					diffRelations(b.relations, a.relations),
				])

				// Name from the snapshot (present even when unchanged), after preferred.
				const nameRow =
					a.values.find((v) => v.propertyId === NAME_PROPERTY) ??
					b.values.find((v) => v.propertyId === NAME_PROPERTY)
				const blockName = nameRow?.text ?? null

				const values = allValues.filter((v) => !HEADLINE_PROPERTIES.has(v.propertyId))
				const relations = allRelations.filter((r) => r.typeId !== TYPES_PROPERTY)

				return {
					...block,
					blockName,
					...(values.length > 0 ? {values} : {}),
					...(relations.length > 0 ? {relations} : {}),
				}
			}),
		)

		return {...diff, blocks: richBlocks}
	}).pipe(Effect.withSpan("enrich-v2.enrichBlocks"))
}
