/**
 * Post-processing enrichment for entity diffs.
 *
 * Resolves human-readable names for entity IDs, property IDs, relation type IDs,
 * and relation target IDs. Applied as a final step before sending the response,
 * so both single-proposal and grouped-proposal endpoints produce display-ready diffs.
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import type {NormalizedUuid} from "../utils/uuid"
import {batchGetEntityNames, type QueryError} from "./queries"
import type {EntityDiff, RelationChange, ValueChange} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

/**
 * Enrich entity diffs with resolved names.
 *
 * Scans all diffs to collect entity IDs that need name resolution,
 * batch-fetches names in a single query, then maps them onto the diffs.
 * Missing names are set to null (graceful degradation).
 */
export function enrichEntityDiffs(db: Database, diffs: EntityDiff[]): Effect.Effect<EntityDiff[], QueryError> {
	return Effect.gen(function* () {
		if (diffs.length === 0) return diffs

		// 1. Collect all unique IDs that need name resolution
		const idsToResolve = new Set<NormalizedUuid>()

		for (const diff of diffs) {
			// Entity name might already be set, but we also want to resolve
			// property IDs, type IDs, and relation target IDs
			if (!diff.name) idsToResolve.add(diff.entityId)

			for (const v of diff.values) {
				idsToResolve.add(v.propertyId)
			}

			for (const r of diff.relations) {
				idsToResolve.add(r.typeId)
				if (r.before?.toEntityId) idsToResolve.add(r.before.toEntityId)
				if (r.after?.toEntityId) idsToResolve.add(r.after.toEntityId)
			}
		}

		if (idsToResolve.size === 0) return diffs

		// 2. Batch-fetch names
		const nameMap = yield* batchGetEntityNames(db, Array.from(idsToResolve))

		// 3. Map names onto diffs
		return diffs.map((diff) => ({
			...diff,
			name: diff.name ?? nameMap.get(diff.entityId) ?? null,
			values: diff.values.map(
				(v): ValueChange => ({
					...v,
					propertyName: nameMap.get(v.propertyId) ?? null,
				}),
			),
			relations: diff.relations.map(
				(r): RelationChange => ({
					...r,
					typeName: nameMap.get(r.typeId) ?? null,
					before: r.before
						? {
								...r.before,
								toEntityName: nameMap.get(r.before.toEntityId) ?? null,
							}
						: r.before,
					after: r.after
						? {
								...r.after,
								toEntityName: nameMap.get(r.after.toEntityId) ?? null,
							}
						: r.after,
				}),
			),
		}))
	}).pipe(Effect.withSpan("enrich.enrichEntityDiffs", {attributes: {diffCount: diffs.length}}))
}
