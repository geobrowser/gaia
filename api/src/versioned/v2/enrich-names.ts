/**
 * v2 enrichment: resolve human-readable names onto a grouped entity diff.
 *
 * Stamps `propertyName` on values and `typeName` / before+after `toEntityName`
 * on relations — including relations nested under dynamic groups (which the v1
 * proposal-path `enrichEntityDiffs` does not walk). Additive: no shape change,
 * missing names degrade to null.
 *
 * Maps to the "name resolution" backend-enrichment ask (RFC: lowest-risk win).
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import type {NormalizedUuid} from "../../utils/uuid"
import {batchGetEntityNames, type QueryError} from "../queries"
import type {GroupedEntityDiff, RelationChange} from "../types"

type Database = NodePgDatabase<Record<string, unknown>>

export function enrichNames(db: Database, diff: GroupedEntityDiff): Effect.Effect<GroupedEntityDiff, QueryError> {
	return Effect.gen(function* () {
		const ids = new Set<NormalizedUuid>()
		const collect = (rels: RelationChange[]) => {
			for (const r of rels) {
				ids.add(r.typeId)
				if (r.before?.toEntityId) ids.add(r.before.toEntityId)
				if (r.after?.toEntityId) ids.add(r.after.toEntityId)
			}
		}

		if (!diff.name) ids.add(diff.entityId)
		for (const v of diff.values) ids.add(v.propertyId)
		collect(diff.relations)
		for (const items of Object.values(diff.groups)) {
			for (const item of items) {
				if ("relations" in item) collect(item.relations)
			}
		}

		if (ids.size === 0) return diff

		const names = yield* batchGetEntityNames(db, Array.from(ids))

		const stamp = (rels: RelationChange[]): RelationChange[] =>
			rels.map((r) => ({
				...r,
				typeName: names.get(r.typeId) ?? null,
				before: r.before ? {...r.before, toEntityName: names.get(r.before.toEntityId) ?? null} : r.before,
				after: r.after ? {...r.after, toEntityName: names.get(r.after.toEntityId) ?? null} : r.after,
			}))

		return {
			...diff,
			name: diff.name ?? names.get(diff.entityId) ?? null,
			values: diff.values.map((v) => ({...v, propertyName: names.get(v.propertyId) ?? null})),
			relations: stamp(diff.relations),
			groups: Object.fromEntries(
				Object.entries(diff.groups).map(([k, items]) => [
					k,
					items.map((item) => ("relations" in item ? {...item, relations: stamp(item.relations)} : item)),
				]),
			),
		}
	}).pipe(Effect.withSpan("enrich-v2.enrichNames"))
}
