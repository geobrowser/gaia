/**
 * v2 enrichment: resolve human-readable names onto a grouped entity diff.
 *
 * Stamps `propertyName` on values and `typeName` / before+after `toEntityName`
 * on relations — at the top level, nested under dynamic groups, AND on rich
 * block `values` / `relations` (produced by enrichBlocks, so run after it).
 * Additive: no shape change, missing names degrade to null.
 *
 * Maps to the "name resolution" backend-enrichment ask (RFC: lowest-risk win).
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import type {NormalizedUuid} from "../../utils/uuid"
import {batchGetEntityNames, type QueryError} from "../queries"
import type {GroupedEntityDiff, RelationChange, ValueChange} from "../types"

type Database = NodePgDatabase<Record<string, unknown>>

export function enrichNames(db: Database, diff: GroupedEntityDiff): Effect.Effect<GroupedEntityDiff, QueryError> {
	return Effect.gen(function* () {
		const ids = new Set<NormalizedUuid>()
		const collectValues = (vals: ValueChange[]) => {
			for (const v of vals) ids.add(v.propertyId)
		}
		const collectRelations = (rels: RelationChange[]) => {
			for (const r of rels) {
				ids.add(r.typeId)
				if (r.before?.toEntityId) ids.add(r.before.toEntityId)
				if (r.after?.toEntityId) ids.add(r.after.toEntityId)
			}
		}

		if (!diff.name) ids.add(diff.entityId)
		collectValues(diff.values)
		collectRelations(diff.relations)
		for (const items of Object.values(diff.groups)) {
			for (const item of items) {
				if ("relations" in item) collectRelations(item.relations)
			}
		}
		for (const block of diff.blocks) {
			if (block.values) collectValues(block.values)
			if (block.relations) collectRelations(block.relations)
		}

		if (ids.size === 0) return diff

		const names = yield* batchGetEntityNames(db, Array.from(ids))

		const stampValues = (vals: ValueChange[]): ValueChange[] =>
			vals.map((v) => ({...v, propertyName: names.get(v.propertyId) ?? null}))
		const stampRelations = (rels: RelationChange[]): RelationChange[] =>
			rels.map((r) => ({
				...r,
				typeName: names.get(r.typeId) ?? null,
				before: r.before ? {...r.before, toEntityName: names.get(r.before.toEntityId) ?? null} : r.before,
				after: r.after ? {...r.after, toEntityName: names.get(r.after.toEntityId) ?? null} : r.after,
			}))

		return {
			...diff,
			name: diff.name ?? names.get(diff.entityId) ?? null,
			values: stampValues(diff.values),
			relations: stampRelations(diff.relations),
			groups: Object.fromEntries(
				Object.entries(diff.groups).map(([k, items]) => [
					k,
					items.map((item) => ("relations" in item ? {...item, relations: stampRelations(item.relations)} : item)),
				]),
			),
			blocks: diff.blocks.map((block) => ({
				...block,
				...(block.values ? {values: stampValues(block.values)} : {}),
				...(block.relations ? {relations: stampRelations(block.relations)} : {}),
			})),
		}
	}).pipe(Effect.withSpan("enrich-v2.enrichNames"))
}
