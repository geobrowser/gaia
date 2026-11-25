/**
 * The `totalCount`-only `relationsConnection` (counters with no `nodes`): compared by two caller-side routes, since geo-db
 * holds no count of a relation filter as such.
 *
 *   relationsConnection(filter: {typeId: {is: R}, toEntityId: {is | in: [T ..]}, spaceId?}) { totalCount }
 *     A target set. geo-db's relations count form (docs/api/03-queries.md "The relations count form") counts one relation
 *     property's statements naming one target, so the set is one read a target (at most MAX_BATCH_IDS over the operation;
 *     over that it stays `unsupported_selection{batch_size}`). `totalCount` counts relation ROWS, so it is compared with the
 *     summed `scanned` (the statements the walk read, one a row: count.zig folds every row of the target's `in` scan into it,
 *     an entity holding two statements to the target counted twice) and never with `members` (the distinct from-entities
 *     holding one, which a row count exceeds whenever an entity holds two). No Space is every Space (a `*` line, which needs
 *     `GEODB_EVERY_SPACE_READS`).
 *
 *   relationsConnection(filter: {fromEntityId: {is: E}, spaceId: {is: S}, typeId: {is | in: [..]}, toEntityId?}) { totalCount }
 *     A from entity. Its relations in one Space are its card's reference statements there: the card is read and its
 *     statements of the Types (and targets, when pinned) are counted, one a row.
 *
 * With `GEODB_RELATIONS_COUNT_SHADOW=on` only; off by default. It never changes a response. A filter either route cannot
 * state (a nested object, a combinator, an operator other than `is`/`equalTo`/`in`, an id list, a derived Type) stays
 * `unsupported_selection{count_only}`, as with the flag off. A serve without the relations count form refuses the read
 * (`count_refused`); a walk that did not finish is `count_incomplete`. Ids are never logged or labelled: a mismatch logs counts.
 */

import type {DocumentNode, FieldNode, SelectionSetNode} from "graphql"
import {valueFromASTUntyped} from "graphql"

import {conformingId, DERIVED_PROPERTIES} from "../geodb/card"
import {GeoDbError, type TypeCount} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import {askGeoDb} from "../geodb/service"
import {geoScopeOf, isCountRefusal, totalCountsOf} from "./countOnlyShadow"
import {type EntityShadowDeps, fieldsOf, parseOperation, Unsupported} from "./entityShadow"
import type {LightOutcome} from "./lightShadow"
import type {UnsupportedReason} from "./metrics"
import {MAX_BATCH_IDS, RELATIONS_CONNECTION_FIELD, relationFilter} from "./pointReadShadows"

export type RelationsCountRoot =
	| {
			/** The root's response key (its alias). */
			key: string
			route: "targets"
			/** `spaceId` (`is` or `in`): the Spaces counted in (gaia ids); null counts across every Space. */
			spaceIds: string[] | null
			/** The one relation Type. */
			typeId: string
			/** The targets, one count read each. */
			targets: string[]
	  }
	| {
			key: string
			route: "card"
			spaceId: string
			fromEntityId: string
			/** The relation Types counted (any of). */
			typeIds: string[]
			/** The targets counted (any of); null for every target. */
			targets: string[] | null
	  }

export type RelationsCountPlan = {roots: RelationsCountRoot[]}

/** `first`, `last`, `offset`, `after`, `before` and `orderBy` page the rows; `totalCount` counts the whole filtered set. */
const PAGING_ARGS = new Set(["filter", "first", "last", "offset", "after", "before", "orderBy"])

const refuse = (reason: UnsupportedReason): never => {
	throw new Unsupported(reason)
}

const isDerived = (typeId: string) => DERIVED_PROPERTIES.has(conformingId("entity", typeId))

function planRoot(root: FieldNode, fragments: Map<string, SelectionSetNode>, variables: Record<string, unknown>) {
	if (!root.selectionSet) return refuse("count_only")
	for (const field of fieldsOf(root.selectionSet, fragments)) {
		const name = field.name.value
		if (name === "__typename") continue
		if (name !== "totalCount" || field.arguments?.length || field.selectionSet) return refuse("count_only")
	}
	const args: Record<string, unknown> = {}
	for (const arg of root.arguments ?? []) args[arg.name.value] = valueFromASTUntyped(arg.value, variables)
	for (const name of Object.keys(args)) if (!PAGING_ARGS.has(name)) refuse("count_only")
	const notes = {partial: false}
	let filter: ReturnType<typeof relationFilter>
	try {
		filter = relationFilter(args.filter, true, notes)
	} catch {
		return refuse("count_only")
	}
	// A clause that only narrows Postgres' rows (a nested object, another operator) changes the count geo-db cannot see.
	if (notes.partial || filter.id || filter.entityId) return refuse("count_only")
	const key = root.alias?.value ?? root.name.value
	const typeIds = filter.typeId ?? []
	if (typeIds.length === 0 || typeIds.some(isDerived)) return refuse("count_only")
	if (filter.fromEntityId) {
		const [fromEntityId] = filter.fromEntityId
		const [spaceId] = filter.spaceId ?? []
		if (filter.fromEntityId.length !== 1 || filter.spaceId?.length !== 1 || !fromEntityId || !spaceId)
			return refuse("count_only")
		return {key, route: "card", spaceId, fromEntityId, typeIds, targets: filter.toEntityId ?? null} as const
	}
	const [typeId] = typeIds
	if (typeIds.length !== 1 || !typeId || !filter.toEntityId) return refuse("count_only")
	if (filter.toEntityId.length > MAX_BATCH_IDS) return refuse("batch_size")
	return {key, route: "targets", spaceIds: filter.spaceId ?? null, typeId, targets: filter.toEntityId} as const
}

/**
 * The operation's `relationsConnection` roots, every one `totalCount`-only, lowered to the two routes; other root fields are
 * not this shadow's. One root neither route can state leaves the whole operation `count_only`; more than MAX_BATCH_IDS geo-db
 * reads over the operation is `batch_size`.
 */
export function analyzeRelationsCount(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
): {ok: true; plan: RelationsCountPlan} | {ok: false; reason: UnsupportedReason} {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const matching = roots.filter((r) => r.name.value === RELATIONS_CONNECTION_FIELD)
		if (matching.length === 0) return {ok: false, reason: "count_only"}
		const planned: RelationsCountRoot[] = []
		const keys = new Set<string>()
		let reads = 0
		for (const root of matching) {
			const one = planRoot(root, fragments, resolved)
			if (keys.has(one.key)) return {ok: false, reason: "count_only"}
			keys.add(one.key)
			reads += one.route === "targets" ? one.targets.length : 1
			planned.push(one.route === "targets" ? {...one, targets: [...one.targets]} : {...one})
		}
		if (reads > MAX_BATCH_IDS) return {ok: false, reason: "batch_size"}
		return {ok: true, plan: {roots: planned}}
	} catch (error) {
		if (error instanceof Unsupported && error.reason === "batch_size") return {ok: false, reason: "batch_size"}
		return {ok: false, reason: "count_only"}
	}
}

type RootCount = {ok: true; rows: number; members?: number} | {ok: false; outcome: LightOutcome}

const throttled = (deps: EntityShadowDeps) => deps.budget !== undefined && !deps.budget.tryAcquire(1)

/** One count read a target, `scanned` summed (the rows), `members` summed beside it for the log. */
async function countTargets(
	deps: EntityShadowDeps,
	root: Extract<RelationsCountRoot, {route: "targets"}>,
	label: string,
): Promise<RootCount> {
	const read = deps.geoDb.readRelationCount?.bind(deps.geoDb)
	if (!read) return {ok: false, outcome: {result: "skip"}}
	const scope = await geoScopeOf(deps, root.spaceIds)
	if (!scope.ok) return scope
	const typeId = conformingId("entity", root.typeId)
	let rows = 0
	let members = 0
	for (const target of root.targets) {
		if (throttled(deps)) return {ok: false, outcome: {result: "unsupported", reason: "throttled"}}
		const started = performance.now()
		let count: TypeCount
		try {
			count = await read({geoSpaceIds: scope.geoSpaceIds, typeId, toEntityId: conformingId("entity", target)})
			observeGeoDbRead((performance.now() - started) / 1000, "ok")
		} catch (error) {
			observeGeoDbRead((performance.now() - started) / 1000, "error")
			if (isCountRefusal(error)) return {ok: false, outcome: {result: "unsupported", reason: "count_refused"}}
			deps.log.warn(`${label} relations count read failed`, {
				reason: error instanceof GeoDbError ? error.reason : "error",
				error: String(error),
			})
			return {ok: false, outcome: {result: "failed"}}
		}
		if (!count.complete) return {ok: false, outcome: {result: "unsupported", reason: "count_incomplete"}}
		rows += count.scanned
		members += count.members
	}
	return {ok: true, rows, members}
}

/** The from entity's card in the Space, its statements of the Types (and targets) counted. */
async function countCard(
	deps: EntityShadowDeps,
	root: Extract<RelationsCountRoot, {route: "card"}>,
	label: string,
): Promise<RootCount> {
	if (!(await deps.geoSpaceOf(root.spaceId)))
		return {ok: false, outcome: {result: "unsupported", reason: "unmapped_space"}}
	if (throttled(deps)) return {ok: false, outcome: {result: "unsupported", reason: "throttled"}}
	try {
		const card = await askGeoDb(deps, conformingId("entity", root.fromEntityId), root.spaceId)
		const types = new Set(root.typeIds.map((id) => conformingId("entity", id)))
		const targets = root.targets ? new Set(root.targets.map((id) => conformingId("entity", id))) : null
		const rows = card.filter((row) => types.has(row.prop) && (!targets || targets.has(row.raw))).length
		return {ok: true, rows}
	} catch (error) {
		deps.log.warn(`${label} card read failed`, {
			reason: error instanceof GeoDbError ? error.reason : "error",
			error: String(error),
		})
		return {ok: false, outcome: {result: "failed"}}
	}
}

/**
 * Compares each root's `totalCount` with geo-db's row count by its route. A mismatch of any root is a mismatch; a root that
 * could not be read leaves the operation `unsupported`/`failed` unless another root already differed. Never throws.
 */
export async function checkRelationsCount(
	deps: EntityShadowDeps,
	plan: RelationsCountPlan,
	body: unknown,
	label: string,
): Promise<LightOutcome> {
	const postgres = totalCountsOf(body, plan)
	if (!postgres) return {result: "skip"}
	const differing: {
		root: number
		route: RelationsCountRoot["route"]
		postgres: number
		geoDb: number
		members?: number
	}[] = []
	let unread: LightOutcome | null = null
	for (const [at, root] of plan.roots.entries()) {
		const counted =
			root.route === "targets" ? await countTargets(deps, root, label) : await countCard(deps, root, label)
		if (!counted.ok) {
			unread ??= counted.outcome
			continue
		}
		const expected = postgres.get(root.key) as number
		if (counted.rows !== expected)
			differing.push({
				root: at,
				route: root.route,
				postgres: expected,
				geoDb: counted.rows,
				...(counted.members !== undefined ? {members: counted.members} : {}),
			})
	}
	if (differing.length > 0)
		return {result: "mismatch", uncompared: [], detail: {relationsCount: true, roots: plan.roots.length, differing}}
	if (unread) return unread
	return {result: "match", uncompared: [], detail: {relationsCount: true, roots: plan.roots.length}}
}
