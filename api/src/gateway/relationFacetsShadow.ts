/**
 * The grouped distinct-count facet of a `relationsConnection` (the facet menu's `RelationFacetByFilter`):
 *
 *   relationsConnection(filter: {typeId: {is: R}, fromEntityId: {is | in: [E ..]}, spaceId?}) {
 *     groupedAggregates(groupBy: [TO_ENTITY_ID]) { keys distinctCount { fromEntityId } }
 *   }
 *
 * Postgres answers it with a grouped `COUNT(DISTINCT from_entity_id)` over the whole filtered set (sampled slow logs up to
 * 11 s). geo-db answers it in one read: the grouped relations form (docs/api/03-queries.md "The grouped relations form",
 * geo-db d69965d241), one row a target holding the distinct entities among `.from` that name it, bounded whatever the scope.
 * One read a root, so one a response, never one a target. Each target's count is compared with the facet's.
 *
 * With `GEODB_RELATION_GROUPED_FACETS_SHADOW=on` only (independent of `GEODB_RELATION_FACETS_SHADOW`; off by default). It never changes a
 * response. The form takes a Type, 1 to 32 from entities and the grouping by target and nothing else: any other filter (a
 * target, a nested object, a combinator), grouping, aggregate or selection stays `unsupported_selection{count_only}`, as with
 * the flag off. A serve below geo-db d69965d241 refuses the document (`client_failed`, `MalformedRequest`): counted
 * `unsupported_selection{count_refused}`, the response untouched, which is where the shape was counted before. A page of
 * targets that came back full may have more and is `count_incomplete`. Ids are never logged or labelled: a mismatch logs counts.
 */

import type {DocumentNode, FieldNode, SelectionSetNode} from "graphql"
import {valueFromASTUntyped} from "graphql"

import {conformingId, DERIVED_PROPERTIES} from "../geodb/card"
import {GeoDbError, MAX_CARD_IDS, RELATION_GROUPS_FIRST, type RelationGroup} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import {geoScopeOf, isCountRefusal} from "./countOnlyShadow"
import {type EntityShadowDeps, fieldsOf, parseOperation, Unsupported} from "./entityShadow"
import type {LightOutcome} from "./lightShadow"
import type {UnsupportedReason} from "./metrics"
import {RELATIONS_CONNECTION_FIELD, relationFilter} from "./pointReadShadows"

export type RelationFacetsRoot = {
	/** The root's response key (its alias). */
	key: string
	/** The response key of its `groupedAggregates`. */
	aggregatesKey: string
	/** `spaceId` (`is` or `in`): the Spaces counted in (gaia ids); null counts across every Space. */
	spaceIds: string[] | null
	typeId: string
	/** The from entities counted (gaia ids), 1 to `MAX_CARD_IDS`. */
	fromEntityIds: string[]
}

export type RelationFacetsPlan = {roots: RelationFacetsRoot[]}

/** `first`, `last`, `offset`, `after`, `before` and `orderBy` page the rows, which an aggregate does not read. */
const PAGING_ARGS = new Set(["filter", "first", "last", "offset", "after", "before", "orderBy"])

const refuse = (reason: UnsupportedReason): never => {
	throw new Unsupported(reason)
}

const isDerived = (typeId: string) => DERIVED_PROPERTIES.has(conformingId("entity", typeId))

/** The aggregate's response key, when `set` selects `groupedAggregates(groupBy: [TO_ENTITY_ID]) { keys distinctCount { fromEntityId } }` and nothing else. */
function planAggregates(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	variables: Record<string, unknown>,
): string {
	let aggregatesKey: string | null = null
	for (const field of fieldsOf(set, fragments)) {
		const name = field.name.value
		if (name === "__typename") continue
		if (name !== "groupedAggregates" || aggregatesKey !== null || !field.selectionSet) return refuse("count_only")
		const args = field.arguments ?? []
		const [groupBy] = args
		if (args.length !== 1 || groupBy?.name.value !== "groupBy") return refuse("count_only")
		const grouping = valueFromASTUntyped(groupBy.value, variables)
		const columns = Array.isArray(grouping) ? grouping : [grouping]
		if (columns.length !== 1 || columns[0] !== "TO_ENTITY_ID") return refuse("count_only")
		let keys = false
		let counted = false
		for (const inner of fieldsOf(field.selectionSet, fragments)) {
			const innerName = inner.name.value
			if (innerName === "__typename") continue
			if (innerName === "keys" && !inner.selectionSet && !inner.arguments?.length) keys = true
			else if (innerName === "distinctCount" && !counted && !inner.arguments?.length && inner.selectionSet) {
				const columns = fieldsOf(inner.selectionSet, fragments).filter((f) => f.name.value !== "__typename")
				if (columns.length !== 1 || columns[0]?.name.value !== "fromEntityId" || columns[0].selectionSet)
					return refuse("count_only")
				counted = true
			} else return refuse("count_only")
		}
		if (!keys || !counted) return refuse("count_only")
		aggregatesKey = field.alias?.value ?? name
	}
	return aggregatesKey ?? refuse("count_only")
}

function planRoot(
	root: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	variables: Record<string, unknown>,
): RelationFacetsRoot {
	if (!root.selectionSet) return refuse("count_only")
	const aggregatesKey = planAggregates(root.selectionSet, fragments, variables)
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
	// A clause that only narrows Postgres' rows changes the counts geo-db cannot see; a target, an id or an own entity is no part of the form.
	if (notes.partial || filter.id || filter.entityId || filter.toEntityId) return refuse("count_only")
	const [typeId] = filter.typeId ?? []
	if (filter.typeId?.length !== 1 || !typeId || isDerived(typeId)) return refuse("count_only")
	if (!filter.fromEntityId) return refuse("count_only")
	if (filter.fromEntityId.length > MAX_CARD_IDS) return refuse("batch_size")
	return {
		key: root.alias?.value ?? root.name.value,
		aggregatesKey,
		spaceIds: filter.spaceId ?? null,
		typeId,
		fromEntityIds: [...filter.fromEntityId],
	}
}

/**
 * The operation's `relationsConnection` roots, every one the grouped distinct-count facet and nothing else, lowered to one
 * grouped read each; another root field is not this shadow's. One root the form cannot state leaves the whole operation
 * `count_only`.
 */
export function analyzeRelationFacets(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
): {ok: true; plan: RelationFacetsPlan} | {ok: false; reason: UnsupportedReason} {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const matching = roots.filter((r) => r.name.value === RELATIONS_CONNECTION_FIELD)
		if (matching.length === 0) return {ok: false, reason: "count_only"}
		const planned: RelationFacetsRoot[] = []
		const keys = new Set<string>()
		for (const root of matching) {
			const one = planRoot(root, fragments, resolved)
			if (keys.has(one.key)) return {ok: false, reason: "count_only"}
			keys.add(one.key)
			planned.push(one)
		}
		return {ok: true, plan: {roots: planned}}
	} catch (error) {
		if (error instanceof Unsupported && error.reason === "batch_size") return {ok: false, reason: "batch_size"}
		return {ok: false, reason: "count_only"}
	}
}

/** PostGraphile's facet of each root as geo-db ids (target to count), or null where the answer is no such list (an error, a null, a count that is no integer). */
function facetsOf(body: unknown, plan: RelationFacetsPlan): Map<string, Map<string, number>> | null {
	if (typeof body !== "object" || body === null) return null
	const answer = body as {errors?: unknown; data?: unknown}
	if (answer.errors !== undefined || typeof answer.data !== "object" || answer.data === null) return null
	const data = answer.data as Record<string, unknown>
	const out = new Map<string, Map<string, number>>()
	for (const root of plan.roots) {
		const held = data[root.key]
		const list =
			typeof held === "object" && held !== null ? (held as Record<string, unknown>)[root.aggregatesKey] : null
		if (!Array.isArray(list)) return null
		const counts = new Map<string, number>()
		for (const row of list) {
			const {keys, distinctCount} = (row ?? {}) as {keys?: unknown; distinctCount?: {fromEntityId?: unknown}}
			const target = Array.isArray(keys) ? keys[0] : undefined
			const raw = distinctCount?.fromEntityId
			// PostGraphile's BigInt is a string.
			const count = typeof raw === "string" && /^\d+$/.test(raw) ? Number(raw) : raw
			if (
				typeof target !== "string" ||
				!Array.isArray(keys) ||
				keys.length !== 1 ||
				typeof count !== "number" ||
				!Number.isInteger(count)
			)
				return null
			counts.set(conformingId("entity", target), count)
		}
		out.set(root.key, counts)
	}
	return out
}

/** The differing targets of one root: a count either side holds differently, or a target only one side holds. */
function differing(postgres: Map<string, number>, geoDb: RelationGroup[]): number {
	const held = new Map(geoDb.map((g) => [g.target, g.count]))
	let n = 0
	for (const [target, count] of postgres) if (held.get(target) !== count) n++
	for (const target of held.keys()) if (!postgres.has(target)) n++
	return n
}

/**
 * Compares each root's facet with geo-db's grouped read, one a root. A mismatch of any root is a mismatch; a root that could
 * not be read leaves the operation `unsupported`/`failed` unless another root already differed. Never throws.
 */
export async function checkRelationFacets(
	deps: EntityShadowDeps,
	plan: RelationFacetsPlan,
	body: unknown,
	label: string,
): Promise<LightOutcome> {
	const postgres = facetsOf(body, plan)
	if (!postgres) return {result: "skip"}
	const read = deps.geoDb.readRelationGroups?.bind(deps.geoDb)
	if (!read) return {result: "skip"}
	const wrong: {root: number; targets: number; postgres: number; geoDb: number}[] = []
	let unread: LightOutcome | null = null
	for (const [at, root] of plan.roots.entries()) {
		const scope = await geoScopeOf(deps, root.spaceIds)
		if (!scope.ok) {
			unread ??= scope.outcome
			continue
		}
		if (deps.budget && !deps.budget.tryAcquire(1)) {
			unread ??= {result: "unsupported", reason: "throttled"}
			continue
		}
		const started = performance.now()
		let groups: RelationGroup[]
		try {
			groups = await read({
				geoSpaceIds: scope.geoSpaceIds,
				typeId: conformingId("entity", root.typeId),
				fromEntityIds: [...new Set(root.fromEntityIds.map((id) => conformingId("entity", id)))],
				first: RELATION_GROUPS_FIRST,
			})
			observeGeoDbRead((performance.now() - started) / 1000, "ok")
		} catch (error) {
			observeGeoDbRead((performance.now() - started) / 1000, "error")
			if (isCountRefusal(error)) {
				unread ??= {result: "unsupported", reason: "count_refused"}
				continue
			}
			deps.log.warn(`${label} grouped relations read failed`, {
				reason: error instanceof GeoDbError ? error.reason : "error",
				error: String(error),
			})
			unread ??= {result: "failed"}
			continue
		}
		const expected = postgres.get(root.key) as Map<string, number>
		// A full page may have more targets, and a facet that long was cut by neither side's page.
		if (groups.length >= RELATION_GROUPS_FIRST) {
			unread ??= {result: "unsupported", reason: "count_incomplete"}
			continue
		}
		const targets = differing(expected, groups)
		if (targets > 0) wrong.push({root: at, targets, postgres: expected.size, geoDb: groups.length})
	}
	if (wrong.length > 0)
		return {
			result: "mismatch",
			uncompared: [],
			detail: {relationFacets: true, roots: plan.roots.length, differing: wrong},
		}
	if (unread) return unread
	return {result: "match", uncompared: [], detail: {relationFacets: true, roots: plan.roots.length}}
}
