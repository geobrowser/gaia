/**
 * The entity objects nested in a relations list (`toEntity`, `fromEntity`, `entity`) and the fields they select beyond
 * their id and name: `description`, `types`, `typeIds`, `spaceIds`, `valuesList`, `relationsList`, `values`,
 * `relations` (backlinks stay uncompared). The entity shadow used to skip all of them (`nested_entity`).
 *
 * With `GEODB_NESTED_ENTITY_SHADOW=on` they are judged against ONE batched card read of every nested entity the
 * answer names (`.args.ids`, geo-db ISS-625-317, at most 32 entities), across every Space geo-db has loaded: PostGraphile
 * takes these fields from any Space, so only an every-Space card can settle them.
 *
 *   - description: PostGraphile's value must be one geo-db holds (null: geo-db must hold none).
 *   - types, typeIds: the same set of Types as geo-db's Types statements.
 *   - spaceIds: every Space geo-db holds a statement in must be listed.
 *   - a values / relations list: each row PostGraphile gave must be one geo-db holds (in the list's Space, when the
 *     list is filtered to one, else in any). Rows only geo-db holds are not checked: a list is a page, and a card
 *     does not say which of its rows are values and which relations.
 *
 * A nested entity geo-db holds no statement about is `no_statements`, not a mismatch (the feeder leaves Spaces out).
 * A selection the plan cannot read (a field with an argument it does not know, a nested object in a row, backlinks) keeps
 * `nested_entity` uncompared for the whole operation; the fields it could read are still judged.
 */

import type {FieldNode, SelectionSetNode} from "graphql"

import {
	conformingId,
	diffCards,
	type GeoCardRow,
	type RelationRow,
	statementsFromPostgres,
	type ValueRow,
} from "../geodb/card"
import {MAX_CARD_IDS} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import {normalizeUuid} from "../utils/uuid"
import {
	argsOf,
	asRecord,
	DESCRIPTION_PROPERTY,
	type EntityShadowDeps,
	fieldsOf,
	isPlainField,
	isUnfilteredField,
	MAX_EVERY_SPACES,
	orNull,
	planColumns,
	RELATION_IGNORED,
	responseKeyOf,
	spaceFromFilter,
	TYPES_PROPERTY,
	VALUE_COLUMNS,
	VALUE_IGNORED,
} from "./entityShadow"
import type {UncomparedReason} from "./metrics"

/** PostGraphile's own page-size ceiling (paginationCapPlugin); a list this long may have been cut short. */
const PAGE_CAP = 1000

export type NestedObjectName = "toEntity" | "fromEntity" | "entity"

/** A `valuesList` / `relationsList` (an array) or `values` / `relations` (a connection with `nodes`) on a nested entity. */
export type NestedList = {
	kind: "values" | "relations"
	/** Response key of the list (or of the connection). */
	key: string
	/** For a connection, the response key of its `nodes`. */
	nodesKey: string | null
	first: number
	/** The PostGraphile Space the list is filtered to, or null for every Space. */
	space: string | null
	/** Response key of each column the comparison reads. */
	columns: Record<string, string>
}

/** Where the compared fields of one nested entity object sit in a row. */
export type NestedEntityPlan = {
	object: NestedObjectName
	/** Response key of the object in the row. */
	key: string
	idKey: string | null
	description: string | null
	spaceIds: string | null
	typeIds: string | null
	/** `types { id }`: the response key of the list and of each item's id. */
	types: {key: string; idKey: string} | null
	lists: NestedList[]
}

/** What planning a relations list found: the nested entity objects, and whether any selection was left unplanned. */
export type NestedEntityPlans = {plans: NestedEntityPlan[]; residual: boolean}

/** Collects the nested entity objects of a row while `planColumns` walks it. */
export type NestedCollector = NestedEntityPlans & {variables: Record<string, unknown>}

const LIST_FIELDS = {
	valuesList: {kind: "values", connection: false},
	values: {kind: "values", connection: true},
	relationsList: {kind: "relations", connection: false},
	relations: {kind: "relations", connection: true},
} as const

/** Built on use: entityShadow.ts and this module import each other, so no constant of it may be read at load. */
const listSpec = (kind: "values" | "relations") =>
	kind === "values"
		? {columns: ["propertyId", ...VALUE_COLUMNS], required: ["propertyId"], ignored: VALUE_IGNORED}
		: {columns: ["typeId", "toEntityId"], required: ["typeId", "toEntityId"], ignored: RELATION_IGNORED}

/** One list on a nested entity, or null when it cannot be read (the operation then keeps `nested_entity`). */
function planNestedList(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	variables: Record<string, unknown>,
): NestedList | null {
	const shape = LIST_FIELDS[field.name.value as keyof typeof LIST_FIELDS]
	if (!shape || !field.selectionSet) return null
	const args = argsOf(field, variables)
	for (const name of Object.keys(args)) if (name !== "first" && name !== "filter" && name !== "after") return null
	if (args.after !== undefined && args.after !== null) return null
	const first = args.first
	if (typeof first !== "number" || !Number.isInteger(first) || first < 1) return null
	const space = spaceFromFilter(args.filter)
	const spec = listSpec(shape.kind)
	const rowSpec = {columns: spec.columns, objects: {}, ignored: spec.ignored as readonly string[]}
	let rows: SelectionSetNode = field.selectionSet
	let nodesKey: string | null = null
	if (shape.connection) {
		let nodes: SelectionSetNode | null = null
		for (const child of fieldsOf(field.selectionSet, fragments)) {
			const name = child.name.value
			if (name === "__typename") continue
			if (name === "nodes" && child.selectionSet && !child.arguments?.length && !nodes) {
				nodes = child.selectionSet
				nodesKey = responseKeyOf(child)
			} else if (name === "pageInfo" && child.selectionSet && !child.arguments?.length) continue
			else return null
		}
		if (!nodes) return null
		rows = nodes
	}
	const planned = planColumns(rows, fragments, rowSpec, new Set())
	if (Object.keys(planned.nested).length > 0) return null
	if (!spec.required.every((column) => planned.columns[column] !== undefined)) return null
	return {kind: shape.kind, key: responseKeyOf(field), nodesKey, first, space, columns: planned.columns}
}

/**
 * Plans one nested entity object (`toEntity { .. }`) of a relations list's rows. Any field it cannot plan sets
 * `residual`, which keeps `nested_entity` uncompared. Never throws.
 */
export function planNestedEntity(
	object: NestedObjectName,
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	variables: Record<string, unknown>,
): {plan: NestedEntityPlan; residual: boolean} {
	const plan: NestedEntityPlan = {
		object,
		key: responseKeyOf(field),
		idKey: null,
		description: null,
		spaceIds: null,
		typeIds: null,
		types: null,
		lists: [],
	}
	let residual = false
	try {
		for (const inner of field.selectionSet ? fieldsOf(field.selectionSet, fragments) : []) {
			const name = inner.name.value
			const key = responseKeyOf(inner)
			if (name === "__typename" || name === "name") continue
			if (name === "id" && isPlainField(inner)) plan.idKey ??= key
			else if (name === "description" && isPlainField(inner) && !plan.description) plan.description = key
			else if (name === "spaceIds" && isPlainField(inner) && !plan.spaceIds) plan.spaceIds = key
			else if (name === "typeIds" && isPlainField(inner) && !plan.typeIds) plan.typeIds = key
			else if (name === "types" && !plan.types && inner.selectionSet && isUnfilteredField(inner, variables)) {
				const id = fieldsOf(inner.selectionSet, fragments).find((f) => f.name.value === "id" && isPlainField(f))
				if (id) plan.types = {key, idKey: responseKeyOf(id)}
				else residual = true
			} else if (name in LIST_FIELDS) {
				const list = planNestedList(inner, fragments, variables)
				if (list) plan.lists.push(list)
				else residual = true
			} else residual = true
		}
	} catch {
		residual = true
	}
	return {plan, residual}
}

/** A nested entity as PostGraphile answered it. `null` fields were not selected. */
export type NestedEntityPg = {
	id: string
	description?: string | null
	spaceIds?: string[]
	typeIds?: string[]
	lists: {list: NestedList; values: ValueRow[]; relations: RelationRow[]}[]
}

/** The nested entities of a card's rows; `incomplete` when some part is not compared (so `nested_entity` stays). */
export type NestedEntities = {items: NestedEntityPg[]; incomplete: boolean}

/** Reads the nested entity objects out of one relation row (`node`), pushing what it finds into `into`. */
export function collectNestedEntities(
	node: Record<string, unknown>,
	toEntityId: string,
	plans: NestedEntityPlans,
	into: NestedEntities,
): void {
	if (plans.residual) into.incomplete = true
	for (const plan of plans.plans) {
		const raw = node[plan.key]
		if (raw === null || raw === undefined) continue
		const object = asRecord(raw)
		if (!object) {
			into.incomplete = true
			continue
		}
		const id = plan.idKey ? object[plan.idKey] : plan.object === "toEntity" ? toEntityId : undefined
		if (typeof id !== "string") {
			into.incomplete = true
			continue
		}
		const item: NestedEntityPg = {id, lists: []}
		let readable = true
		if (plan.description) {
			const v = object[plan.description]
			item.description = typeof v === "string" ? v : null
		}
		const strings = (v: unknown): string[] | null =>
			Array.isArray(v) && v.every((s) => typeof s === "string") ? (v as string[]) : null
		if (plan.spaceIds) {
			const ids = strings(object[plan.spaceIds])
			if (ids) item.spaceIds = ids.map((s) => normalizeUuid(s))
			else readable = false
		}
		if (plan.typeIds) {
			const ids = strings(object[plan.typeIds])
			if (ids) item.typeIds = ids
			else readable = false
		}
		if (plan.types) {
			const nodes = object[plan.types.key]
			const ids = Array.isArray(nodes) ? nodes.map((n) => asRecord(n)?.[plan.types?.idKey as string]) : null
			if (ids?.every((s) => typeof s === "string")) {
				// `types` and `typeIds` on one object are the same statements: the union is what is judged.
				item.typeIds = [...new Set([...(item.typeIds ?? []), ...(ids as string[])])]
			} else readable = false
		}
		for (const list of plan.lists) {
			const holder = list.nodesKey ? asRecord(object[list.key]) : object
			const rows = list.nodesKey ? holder?.[list.nodesKey] : object[list.key]
			if (!Array.isArray(rows)) {
				readable = false
				continue
			}
			const entry = {list, values: [] as ValueRow[], relations: [] as RelationRow[]}
			let ok = true
			for (const raw of rows) {
				const row = asRecord(raw)
				const at = (column: string) => (row ? row[list.columns[column] as string] : undefined)
				if (!row) ok = false
				else if (list.kind === "values") {
					const propertyId = at("propertyId")
					if (typeof propertyId !== "string") ok = false
					else
						entry.values.push({
							property_id: propertyId,
							text: orNull(at("text")),
							integer: orNull(at("integer")),
							float: orNull(at("float")),
							decimal: orNull(at("decimal")),
							boolean: orNull(at("boolean")),
							datetime: orNull(at("datetime")),
							date: orNull(at("date")),
						})
				} else {
					const typeId = at("typeId")
					const to = at("toEntityId")
					if (typeof typeId !== "string" || typeof to !== "string") ok = false
					else entry.relations.push({type_id: typeId, to_entity_id: to, to_space_id: null})
				}
			}
			if (ok) item.lists.push(entry)
			else readable = false
		}
		if (readable) into.items.push(item)
		else into.incomplete = true
	}
}

export type NestedVerdict =
	| {read: false}
	| {
			read: true
			/** Per nested field that disagreed (`description`, `types`, `spaceIds`, `values`, `relations`), the count. */
			mismatches: Record<string, number>
			uncompared: UncomparedReason[]
			/** Every part of the nested selection was judged or counted: `nested_entity` is no longer uncompared. */
			settled: boolean
			entities: number
	  }

const setOf = (ids: string[]) => new Set(ids.map((id) => conformingId("entity", id)))

/**
 * Judges the nested entities PostGraphile named against one batched card read of them across every Space.
 * `{read: false}` when it cannot be read (too many entities, no every-Space reads, no batch client, an empty budget, a
 * failed read): nothing is counted and `nested_entity` stays. Never throws.
 */
export async function checkNestedEntities(
	deps: EntityShadowDeps,
	nested: NestedEntities,
	label: string,
): Promise<NestedVerdict> {
	const readCards = deps.geoDb.readCards?.bind(deps.geoDb)
	if (!deps.allGeoSpaces || !readCards || nested.items.length === 0) return {read: false}
	const ids = [...new Set(nested.items.map((item) => conformingId("entity", item.id)))]
	if (ids.length > MAX_CARD_IDS) return {read: false}
	try {
		const spaces = await deps.allGeoSpaces()
		if (spaces.length === 0 || spaces.length > MAX_EVERY_SPACES) return {read: false}
		if (deps.everySpaceBudget && !deps.everySpaceBudget.tryAcquire(1)) return {read: false}
		const started = performance.now()
		let rows: GeoCardRow[]
		try {
			rows = await readCards({geoSpaceIds: spaces, entityIds: ids})
			observeGeoDbRead((performance.now() - started) / 1000, "ok")
		} catch (error) {
			observeGeoDbRead((performance.now() - started) / 1000, "error")
			throw error
		}
		const byEntity = new Map<string, GeoCardRow[]>(ids.map((id) => [id, []]))
		for (const row of rows) {
			const held = row.entity ? byEntity.get(row.entity) : undefined
			// A single-Space line prints bare statement ids, so its rows name no Space.
			if (held) held.push({...row, space: row.space ?? (spaces.length === 1 ? spaces[0] : undefined)})
		}
		return await judgeNested(deps, nested, byEntity, spaces)
	} catch (error) {
		deps.log.warn(`${label} nested entity read failed`, {entities: ids.length, error: String(error)})
		return {read: false}
	}
}

async function judgeNested(
	deps: EntityShadowDeps,
	nested: NestedEntities,
	byEntity: Map<string, GeoCardRow[]>,
	loaded: string[],
): Promise<NestedVerdict> {
	const mismatches: Record<string, number> = {}
	const uncompared = new Set<UncomparedReason>()
	const flag = (field: string) => {
		mismatches[field] = (mismatches[field] ?? 0) + 1
	}
	const geoSpace = new Map<string, string | null>()
	const mapSpace = async (space: string): Promise<string | null> => {
		if (!geoSpace.has(space)) geoSpace.set(space, (await deps.geoSpaceOf(space)) ?? null)
		return geoSpace.get(space) ?? null
	}
	const known = new Set(loaded)
	let settled = !nested.incomplete
	for (const item of nested.items) {
		const geo = byEntity.get(conformingId("entity", item.id)) ?? []
		const anything =
			(item.description ?? null) !== null ||
			(item.spaceIds?.length ?? 0) > 0 ||
			(item.typeIds?.length ?? 0) > 0 ||
			item.lists.some((l) => l.values.length + l.relations.length > 0)
		if (geo.length === 0) {
			// geo-db holds nothing of this entity in any loaded Space: a gap the feeder owns, not a verdict on the fields.
			if (anything) uncompared.add("no_statements")
			continue
		}
		if (item.description !== undefined) {
			const held = geo.filter((row) => row.prop === DESCRIPTION_PROPERTY).map((row) => row.raw)
			// An empty text is no text to a reader (as the card diff skips it).
			const value = item.description === "" ? null : item.description
			if (value === null ? held.some((v) => v !== "") : !held.includes(value)) flag("description")
		}
		if (item.typeIds) {
			const postgres = setOf(item.typeIds)
			const held = new Set(geo.filter((row) => row.prop === TYPES_PROPERTY).map((row) => row.raw))
			if (postgres.size !== held.size || [...postgres].some((id) => !held.has(id))) flag("types")
		}
		if (item.spaceIds) {
			const listed = new Set<string>()
			let mapped = true
			for (const space of item.spaceIds) {
				const g = await mapSpace(space)
				if (g) listed.add(g)
				else mapped = false
			}
			const holding = new Set(
				geo.map((row) => row.space).filter((s): s is string => s !== undefined && known.has(s)),
			)
			// Every Space geo-db holds a statement in must be listed; a Space PostGraphile lists that geo-db lacks is a gap
			// the feeder owns. A listed Space with no geo-db mapping cannot be judged.
			if ([...holding].some((s) => !listed.has(s)) && mapped) flag("spaceIds")
			else if (!mapped) uncompared.add("cross_space")
		}
		for (const {list, values, relations} of item.lists) {
			let scoped = geo
			if (list.space !== null) {
				const g = await mapSpace(list.space)
				if (!g) {
					uncompared.add("cross_space")
					settled = false
					continue
				}
				scoped = geo.filter((row) => row.space === g)
			}
			const diff = diffCards(statementsFromPostgres(values, relations), scoped, {skipEmptyText: true})
			if (diff.onlyPostgres.length > 0) flag(list.kind)
			if (values.length + relations.length >= Math.min(list.first, PAGE_CAP)) uncompared.add("truncated")
		}
	}
	return {read: true, mismatches, uncompared: [...uncompared], settled, entities: nested.items.length}
}
