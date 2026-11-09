/**
 * The `totalCount`-only `entitiesConnection` (the topic page's counters): compared with geo-db's entities count form.
 *
 * `entitiesConnection(filter: {and: [{typeIds: {overlaps: [T]}}, {relations: {some: {typeId: {is: R}, toEntityId: {in: [..]}}}}]})
 * { totalCount }` returns no entity to judge, so it was counted `unsupported_selection{count_only}`. geo-db answers it
 * exactly (docs/api/03-queries.md "The count form", DR-154 item 8): the `entities` root with `.type = T`, `.count = true` and
 * the reference term `.where = .{ R = .{ .in = targets } }` is one row of three integers a page (`members`, `scanned`,
 * `complete`); the tool fetches the pages from the cursor and the client sums them. `members` is compared with the
 * response's `totalCount`, one read a root (an operation of several aliased counters is several reads, at most 8).
 *
 * With `GEODB_COUNT_ONLY_SHADOW=on` only; off by default. It never changes a response. Needs a serve whose store has the count
 * form with `where`: an older one refuses the document (`unsupported_selection{count_refused}`, never a failed request).
 * A root with no Space argument is counted across every Space (a `*` line, which needs `GEODB_EVERY_SPACE_READS`).
 * A count the walk did not finish is `count_incomplete`. Ids are never logged or labelled: a mismatch logs the counts.
 */

import {conformingId} from "../geodb/card"
import {GeoDbError, MAX_LINE_SPACES, type TypeCount} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import type {EntityShadowDeps} from "./entityShadow"
import type {CountOnlyPlan, CountOnlyRoot, LightOutcome} from "./lightShadow"

/** A serve that predates the count form (or its `.where`) refuses the document; that refusal is the only one counted as unsupported. */
export const isCountRefusal = (error: unknown): boolean =>
	error instanceof GeoDbError && error.reason === "client_failed" && /\bMalformedRequest\b/.test(error.message)

/** PostGraphile's `totalCount` of each root, or null when the response holds none of them (an error, a null, a non-integer). */
export function totalCountsOf(body: unknown, plan: CountOnlyPlan): Map<string, number> | null {
	if (typeof body !== "object" || body === null) return null
	const root = body as {errors?: unknown; data?: unknown}
	if (root.errors !== undefined || typeof root.data !== "object" || root.data === null) return null
	const data = root.data as Record<string, unknown>
	const counts = new Map<string, number>()
	for (const {key} of plan.roots) {
		const held = data[key]
		const total =
			typeof held === "object" && held !== null ? (held as {totalCount?: unknown}).totalCount : undefined
		if (typeof total !== "number" || !Number.isInteger(total) || total < 0) return null
		counts.set(key, total)
	}
	return counts
}

/** Reads one root's count from geo-db. */
async function readRoot(
	deps: EntityShadowDeps,
	root: CountOnlyRoot,
	label: string,
): Promise<{ok: true; count: TypeCount} | {ok: false; outcome: LightOutcome}> {
	const readTypeCount = deps.geoDb.readTypeCount?.bind(deps.geoDb)
	if (!readTypeCount) return {ok: false, outcome: {result: "skip"}}
	let geoSpaceIds: string[] | "every"
	if (root.spaceIds === null) {
		// Every Space the session reads, one `*` line; the same flag as the every-Space card reads.
		if (!deps.allGeoSpaces) return {ok: false, outcome: {result: "unsupported", reason: "no_space"}}
		geoSpaceIds = "every"
	} else {
		if (root.spaceIds.length > MAX_LINE_SPACES)
			return {ok: false, outcome: {result: "unsupported", reason: "many_spaces"}}
		if (root.spaceIds.length > 1 && deps.multiSpace !== true)
			return {ok: false, outcome: {result: "unsupported", reason: "no_space"}}
		const mapped: string[] = []
		for (const space of root.spaceIds) {
			const geo = await deps.geoSpaceOf(space)
			if (!geo) return {ok: false, outcome: {result: "unsupported", reason: "unmapped_space"}}
			mapped.push(geo)
		}
		geoSpaceIds = [...new Set(mapped)]
	}
	if (deps.budget && !deps.budget.tryAcquire(1))
		return {ok: false, outcome: {result: "unsupported", reason: "throttled"}}
	const started = performance.now()
	try {
		const count = await readTypeCount({
			geoSpaceIds,
			typeId: conformingId("entity", root.typeId),
			...(root.edge
				? {
						where: {
							property: conformingId("entity", root.edge.typeId),
							targets: [...new Set(root.edge.targets.map((id) => conformingId("entity", id)))],
						},
					}
				: {}),
		})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
		return {ok: true, count}
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		if (isCountRefusal(error)) return {ok: false, outcome: {result: "unsupported", reason: "count_refused"}}
		deps.log.warn(`${label} count read failed`, {
			reason: error instanceof GeoDbError ? error.reason : "error",
			error: String(error),
		})
		return {ok: false, outcome: {result: "failed"}}
	}
}

/**
 * Compares the response's `totalCount` of each lowered root with geo-db's count. A mismatch of any root is a mismatch; a root that
 * could not be read leaves the operation `unsupported`/`failed` unless another root already differed. Never throws.
 */
export async function checkCountOnly(
	deps: EntityShadowDeps,
	plan: CountOnlyPlan,
	body: unknown,
	label: string,
): Promise<LightOutcome> {
	const postgres = totalCountsOf(body, plan)
	if (!postgres) return {result: "skip"}
	const differing: {root: number; postgres: number; geoDb: number; scanned: number}[] = []
	let unread: LightOutcome | null = null
	for (const [at, root] of plan.roots.entries()) {
		const read = await readRoot(deps, root, label)
		if (!read.ok) {
			unread ??= read.outcome
			continue
		}
		if (!read.count.complete) {
			unread ??= {result: "unsupported", reason: "count_incomplete"}
			continue
		}
		const expected = postgres.get(root.key) as number
		if (read.count.members !== expected)
			differing.push({root: at, postgres: expected, geoDb: read.count.members, scanned: read.count.scanned})
	}
	if (differing.length > 0)
		return {result: "mismatch", uncompared: [], detail: {countOnly: true, roots: plan.roots.length, differing}}
	if (unread) return unread
	return {result: "match", uncompared: [], detail: {countOnly: true, roots: plan.roots.length}}
}
