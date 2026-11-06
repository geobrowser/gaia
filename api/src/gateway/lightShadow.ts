/**
 * The "light" entity selection: what the news app (and other readers that list an entity's values and
 * relations across every space) ask for, and what geo-db can settle of it.
 *
 *   entity(id: <uuid>) {                       entities(filter: {typeIds: .., relations: ..}) {
 *     id name description spaceIds               id name description spaceIds
 *     values(first: 50) { nodes { property { id name } text datetime } }
 *     rels: relations(first: 200, filter: {typeId: {in: [..]}}) {
 *       nodes { spaceId type { id name } toEntity { id name values(first: 5) {..} relations(first: 5) {..} } } }
 *   }
 *
 * The card shadow (entityShadow.ts) needs both lists of one space, complete. These selections give neither: the
 * lists are unfiltered by space, filtered by type, cut by `first`, and select only some value columns. What a
 * card can still settle exactly is containment: every row PostGraphile returned must be a statement geo-db
 * holds (in the row's own space when it names one, else in any of the entity's spaces). Rows geo-db holds that
 * PostGraphile's filtered or cut list did not return are not checked (`partial_values`), and the derived
 * fields (name, description, types, spaceIds, nested names) are judged exactly as the card shadow judges them,
 * over the union of the entity's space cards. The spaces read are the entity's own, from its `spaceIds`.
 *
 * For `entities` the same check runs on a sample of the returned entities (`sampled_rows`). A filter
 * `typeIds: {anyEqualTo}`, `relations: {some: {typeId, toEntityId}}` or `id: {in}` is checked for soundness on
 * each sampled entity (geo-db must hold the statement the filter promises); which entities the filter should
 * have returned is not checkable from a card (`filter_completeness`).
 */

import {type DocumentNode, type FieldNode, Kind, print, type SelectionSetNode, visit} from "graphql"

import {
	conformingId,
	DERIVED_PROPERTIES,
	diffCards,
	type GeoCardRow,
	type RelationRow,
	statementsFromPostgres,
	type ValueRow,
} from "../geodb/card"
import {GeoDbError} from "../geodb/client"
import {askGeoDbCards} from "../geodb/service"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import {type BacklinkRowsPlan, compareBacklinks, extractBacklinkRows} from "./backlinksShadow"
import {
	argsOf,
	asRecord,
	type CardCheck,
	compareDerived,
	ENTITY_FIELD,
	ENTITY_TOLERATED_ROOTS,
	type EntityCardPlan,
	type EntityShadowDeps,
	fieldsOf,
	isPlainField,
	type ListPlan,
	orNull,
	type PlannedColumns,
	type PostgresCard,
	PROPERTY_OBJECT,
	parseOperation,
	planColumns,
	planEntitySelection,
	RELATION_IGNORED,
	RELATION_OBJECTS,
	read,
	readEverySpace,
	readName,
	readWide,
	responseKeyOf,
	secondsOrNull,
	TYPES_PROPERTY,
	Unsupported,
	uuidArg,
	VALUE_COLUMNS,
	VALUE_IGNORED,
} from "./entityShadow"
import {recordGatewayUnsupportedShape, type UncomparedReason, type UnsupportedReason} from "./metrics"
import {checkOrderedPage} from "./orderPickShadow"

/** Most distinct relation targets whose own values one entity's comparison reads (two batch reads). */
const MAX_TARGET_CARDS = 64
/** Most returned entities one `entities` operation compares; the rest are `sampled_rows`. */
export const MAX_SAMPLED_ENTITIES = 3
/** Most returned entities one every-Space `entities` operation compares: each reads the whole space map. */
export const MAX_EVERY_SPACE_SAMPLED = 1

const refuse = (reason: UnsupportedReason): never => {
	throw new Unsupported(reason)
}

type LightList = {
	kind: "values" | "relations"
	/** Where the row objects sit: the entity object's key, then (connection) `nodes`'s key. */
	key: string
	nodesKey: string | null
	rows: ListPlan
	spaceKey: string | null
	/**
	 * An unfiltered relations list that selects `spaceId` and no edge (`allRelationsList { spaceId }`): geo-db can
	 * say which Spaces hold the entity, not which relations each holds, so its rows are never compared
	 * (`membership_only`).
	 */
	projection?: boolean
	/**
	 * A relations list filtered by `typeId` (`is` or `in`) that selects no type: the filter's Types, one of which each
	 * row has. Its rows are checked as "held under one of these Types" (`typeless`), not as exact statements.
	 */
	typeFilter?: string[]
	/**
	 * A values list filtered by `propertyId` (`is` or `in`) that selects no property: the filter's Properties, one of
	 * which each row has. Its rows are checked as "held under one of these Properties" (`untypedValues`).
	 */
	propertyFilter?: string[]
	/**
	 * A relations list that selects each row's type and a `toEntity` object with no id (only fields the card cannot
	 * settle, such as its `valuesList`): the rows carry no target to look up, so each is checked as "geo-db holds a
	 * relation of this type", and the target's own fields are `nested_entity`.
	 */
	typeOnly?: boolean
	/**
	 * A relations list that selects each row's target by id and the target's own `valuesList`: where the target's values
	 * sit in a row (the `toEntity` key, then the nested list) and their columns. Checked against geo-db's card of the
	 * target when the batch read is on, else `nested_entity`.
	 */
	target?: {key: string; values: LightList}
}

const noted = new Set<string>()
/** Deep enough to show the keys inside `and([relations(some(typeId(is),toEntityId(is)))])`. */
const MAX_SHAPE_DEPTH = 6
/** Distinct shapes given their own metric label per process; later new ones count as `other`. */
const MAX_SHAPES = 150
const shapesSeen = new Set<string>()
const MAX_NOTED = 200

/** A value's shape: an object's keys (sorted), each with its own shape; a list's distinct element shapes. No values. */
function describeShape(value: unknown, depth: number): string {
	if (depth > MAX_SHAPE_DEPTH) return "..."
	if (Array.isArray(value)) {
		const parts = [...new Set(value.map((v) => describeShape(v, depth + 1)).filter(Boolean))].sort()
		return parts.length ? `[${parts.join("|")}]` : ""
	}
	if (value === null || typeof value !== "object") return ""
	return Object.keys(value)
		.sort()
		.map((key) => {
			const inner = describeShape((value as Record<string, unknown>)[key], depth + 1)
			return inner ? `${key}(${inner})` : key
		})
		.join(",")
}

/**
 * The argument shape of an operation's `field` root: its argument names (sorted) and, for `filter`, the filter's
 * keys and operators, e.g. `args=filter,first;filter=fromEntityId(is),spaceId(is),typeId(is)`. Never a value.
 */
export function shapeOfRoot(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
	field: string,
): string {
	try {
		const {variables: resolved, roots} = parseOperation(document, operationName, variables)
		const root = roots.find((r) => r.name.value === field)
		if (!root) return `roots=${[...new Set(roots.map((r) => r.name.value))].sort().join(",")}`
		const args = argsOf(root, resolved)
		const names = Object.keys(args).sort()
		const filter = describeShape(args.filter, 0)
		const shape = `args=${names.join(",")}${filter ? `;filter=${filter}` : ""}`
		return shape.length > 300 ? "long" : shape
	} catch {
		return "unparsed"
	}
}

const MAX_QUERY_LOGGED = 4000

/**
 * The operation's text with every literal value (strings, numbers, lists, objects) replaced by a placeholder, so
 * the selection can be read off a log line and replayed offline without any value in it. Capped.
 */
export function redactedQuery(operation: DocumentNode["definitions"][number]): string {
	const redacted = visit(operation, {
		StringValue: () => ({kind: Kind.STRING, value: "_"}),
		IntValue: (node) => (node.value.length <= 4 ? node : {kind: Kind.INT, value: "0"}),
		ListValue: () => ({kind: Kind.LIST, values: []}),
	})
	return print(redacted).replace(/\s+/g, " ").slice(0, MAX_QUERY_LOGGED)
}

/**
 * Counts the argument shape of an operation that could not be shadowed (`gaia_api_gateway_shadow_unsupported_shape_total`)
 * and logs, once per process for each (field, reason, operation name), its root arguments and the top-level fields
 * it selects. No values, so nothing private is logged. It is how the real query mix behind
 * `gaia_api_gateway_shadow_unsupported_total` is ranked. At most MAX_NOTED lines.
 */
export function noteUnsupported(
	deps: Pick<EntityShadowDeps, "log">,
	field: string,
	reason: UnsupportedReason,
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown> = {},
): void {
	try {
		let shape = shapeOfRoot(document, operationName, variables, field)
		// Bound the metric's label cardinality: a shape seen before is free, a new one needs room.
		if (!shapesSeen.has(shape)) {
			if (shapesSeen.size >= MAX_SHAPES) shape = "other"
			else shapesSeen.add(shape)
		}
		recordGatewayUnsupportedShape(field, reason, shape)
		// The shape is in the key so anonymous operations (no name) of different shapes are each logged once.
		const key = `${field}\t${reason}\t${operationName ?? ""}\t${shape}`
		if (noted.has(key) || noted.size >= MAX_NOTED) return
		noted.add(key)
		const operation = document.definitions.find(
			(d) => d.kind === "OperationDefinition" && (!operationName || d.name?.value === operationName),
		)
		if (!operation || operation.kind !== "OperationDefinition") return
		const roots = operation.selectionSet.selections.flatMap((selection) =>
			selection.kind === "Field"
				? [
						{
							field: selection.name.value,
							args: (selection.arguments ?? []).map((a) => a.name.value),
							selects: (selection.selectionSet?.selections ?? []).flatMap((c) =>
								c.kind === "Field" ? [c.name.value] : [],
							),
						},
					]
				: [],
		)
		// warn: the api runs at warn, and this is capped at MAX_NOTED lines per process.
		deps.log.warn("gateway shadow unsupported", {
			field,
			reason,
			operationName,
			shape,
			roots,
			query: redactedQuery(operation),
		})
	} catch {
		// Observability only.
	}
}

/** Soundness of an `entities` filter, for the forms geo-db can check on one entity's card. */
/** `type`: the entity holds a Type statement for at least one of `typeIds`. */
export type FilterClaim = {kind: "type"; typeIds: string[]} | {kind: "edge"; typeId: string; toEntityId: string}

export type LightPlan = {
	card: EntityCardPlan
	lists: LightList[]
	uncompared: UncomparedReason[]
}

/**
 * `everySpace`: the answer will not say which Spaces the entity lives in (no `spaceIds`, no row spaces), so the
 * comparison reads every Space geo-db has loaded (`readEverySpace`). Such a plan's lists are empty, and a null
 * answer is compared too: geo-db must hold nothing about the entity.
 */
export type LightEntityPlan = LightPlan & {kind: "entity"; key: string; entityId: string; everySpace: boolean}
export type LightEntitiesPlan = LightPlan & {
	kind: "entities"
	key: string
	claims: FilterClaim[]
	/** The ids the filter asked for (`id: {in}`), if that was the filter. */
	ids: string[] | null
	/** `entities(spaceId:)` or `entities(spaceIds: {in})`: the entity must hold a statement in one of these spaces. */
	spaceIds: string[] | null
	/**
	 * No lists, no `spaceIds`, no Space argument (`EntityNames`: `entities(filter: {id: {in}}) { id name }`): each
	 * sampled entity is read across every loaded Space, as an `entity` plan's `everySpace`.
	 */
	everySpace: boolean
	/**
	 * The page's order when it is a single key geo-db can place by (`orderBy: CREATED_AT_*` or `UPDATED_AT_*`, alone), else
	 * null. `first` and `offset` are the root's, null when absent.
	 */
	order: EntitiesOrder | null
	/** The filter and root arguments say nothing but the plan's `claims` (no clause or system-type argument was left out). */
	exactFilter: boolean
	first: number | null
	offset: number | null
	/** `entitiesConnection`: the entities sit under the root's `nodes` field (its response key), not in the root's own list. */
	nodesKey?: string | null
}

export type EntitiesOrder = {property: "createdAt" | "updatedAt"; direction: "asc" | "desc"}

export type LightAnalysis<P> = {ok: true; plan: P} | {ok: false; reason: UnsupportedReason}

// Built on use, not at load: entityShadow.ts imports this file and this file imports entityShadow.ts.
const valueSpec = () => ({
	columns: ["propertyId", "spaceId", ...VALUE_COLUMNS],
	objects: {property: PROPERTY_OBJECT},
	ignored: VALUE_IGNORED,
})
const relationSpec = () => ({
	columns: ["typeId", "toEntityId", "spaceId"],
	objects: RELATION_OBJECTS,
	ignored: RELATION_IGNORED,
})

/** One list field of the light selection: its row columns, where they sit, and what it cannot settle. */
/** The Types a relations list's filter allows (`typeId: {is}` or `{in}`), or null when it names none that way. */
function typeFilterOf(filter: unknown, key: "typeId" | "propertyId" = "typeId"): string[] | null {
	const clause = asRecord(asRecord(filter)?.[key])
	if (!clause) return null
	const ids = typeof clause.is === "string" ? [clause.is] : Array.isArray(clause.in) ? clause.in : null
	if (!ids || ids.length === 0 || ids.length > MAX_TYPE_FILTER) return null
	if (!ids.every((id) => typeof id === "string" && isValidUuid(id))) return null
	return [...new Set((ids as string[]).map((id) => normalizeUuid(id)))]
}

/** Most Types a typeless relations list's filter may name; a wider one is not planned. */
const MAX_TYPE_FILTER = 20

/** Whether the relation rows select a `toEntity` object (in the row, or in a connection's `nodes`). */
function selectsTarget(field: FieldNode, fragments: Map<string, SelectionSetNode>): boolean {
	const rowsOf = (set: SelectionSetNode | undefined): FieldNode[] => (set ? fieldsOf(set, fragments) : [])
	const direct = rowsOf(field.selectionSet)
	const nodes = direct.find((child) => child.name.value === "nodes")
	return (nodes ? rowsOf(nodes.selectionSet) : direct).some(
		(child) => child.name.value === "toEntity" && !!child.selectionSet,
	)
}

function planLightList(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	resolved: Record<string, unknown>,
	uncompared: Set<UncomparedReason>,
	options: {projections?: boolean; relationsTypeFilter?: boolean} = {},
): LightList {
	const name = field.name.value
	const kind = name === "values" || name === "valuesList" ? "values" : "relations"
	const connection = name === "values" || name === "relations"
	const args = argsOf(field, resolved)
	for (const arg of Object.keys(args))
		if (!["first", "filter", "after", "offset", "orderBy"].includes(arg)) refuse("args")
	// An empty `orderBy: []` (what the frontend's list helpers send) asks for no order; a real order is not checked here.
	if (
		args.orderBy !== undefined &&
		args.orderBy !== null &&
		!(Array.isArray(args.orderBy) && args.orderBy.length === 0)
	)
		refuse("args")
	if (args.after !== undefined && args.after !== null) refuse("args")
	if (args.offset !== undefined && args.offset !== null && args.offset !== 0) refuse("args")
	const filter = args.filter
	// A filter only narrows the rows PostGraphile returned, and each returned row is checked: its shape is moot.
	if (filter !== undefined && filter !== null && !asRecord(filter)) refuse("filter")
	if (!field.selectionSet) return refuse("selection")
	const spec = kind === "values" ? valueSpec() : relationSpec()
	const first = typeof args.first === "number" && Number.isInteger(args.first) && args.first > 0 ? args.first : 0
	let rows: PlannedColumns
	let nodesKey: string | null = null
	if (!connection) {
		rows = planColumns(field.selectionSet, fragments, spec, uncompared)
	} else {
		let planned: PlannedColumns | null = null
		for (const child of fieldsOf(field.selectionSet, fragments)) {
			const childName = child.name.value
			if (childName === "__typename") continue
			if (childName === "totalCount" && isPlainField(child)) uncompared.add("relation_count")
			else if (childName === "pageInfo" && child.selectionSet && !child.arguments?.length) continue
			else if (childName === "nodes" && child.selectionSet && !child.arguments?.length && !planned) {
				nodesKey = responseKeyOf(child)
				planned = planColumns(child.selectionSet, fragments, spec, uncompared)
			} else refuse("selection")
		}
		if (!planned) return refuse("selection")
		rows = planned
	}
	const has = (column: string) => rows.columns[column] !== undefined || rows.nested[column] !== undefined
	if (kind === "values") {
		if (!has("propertyId")) {
			// The filter says which Properties the rows hold; the answer does not say which one each does.
			const propertyFilter = options.relationsTypeFilter ? typeFilterOf(filter, "propertyId") : null
			if (!propertyFilter) refuse("selection")
			uncompared.add("filter_completeness")
			uncompared.add("partial_values")
			return {
				kind,
				key: responseKeyOf(field),
				nodesKey,
				rows: {key: responseKeyOf(field), first, ...rows, nodesKey, hasNext: null},
				spaceKey: rows.columns.spaceId ?? null,
				propertyFilter: propertyFilter as string[],
			}
		}
		if (!VALUE_COLUMNS.every(has)) uncompared.add("partial_values")
	} else if (!has("typeId") && has("toEntityId") && options.relationsTypeFilter && typeFilterOf(filter)) {
		// The filter says which Types the rows hold; the answer does not say which one each does.
		uncompared.add("filter_completeness")
		return {
			kind,
			key: responseKeyOf(field),
			nodesKey,
			rows: {key: responseKeyOf(field), first, ...rows, nodesKey, hasNext: null},
			spaceKey: rows.columns.spaceId ?? null,
			typeFilter: typeFilterOf(filter) as string[],
		}
	} else if (has("typeId") && !has("toEntityId") && selectsTarget(field, fragments)) {
		uncompared.add("nested_entity")
		return {
			kind,
			key: responseKeyOf(field),
			nodesKey,
			rows: {key: responseKeyOf(field), first, ...rows, nodesKey, hasNext: null},
			spaceKey: rows.columns.spaceId ?? null,
			typeOnly: true,
		}
	} else if (!has("typeId") || !has("toEntityId")) {
		const unfiltered = filter === undefined || filter === null
		// Rows that name nothing but their Type and target (`relationsList { type { name } toEntity { name } }`): the
		// answer says neither which Space a row is in nor which statement it is, and the names come from any Space, so
		// no row can be held to a statement. The list is counted uncompared; the rest of the entity is compared.
		const namesOnly = !rows.columns.spaceId && (rows.names.type !== undefined || rows.names.toEntity !== undefined)
		if (!(options.projections && unfiltered && !connection && !has("typeId") && !has("toEntityId")))
			refuse("selection")
		if (!rows.columns.spaceId && !namesOnly) refuse("selection")
		uncompared.add(namesOnly ? "relation_names" : "membership_only")
		return {
			kind,
			key: responseKeyOf(field),
			nodesKey: null,
			rows: {key: responseKeyOf(field), first, ...rows, nodesKey: null, hasNext: null},
			spaceKey: rows.columns.spaceId ?? null,
			projection: true,
		}
	}
	const target = kind === "relations" ? planTargetValues(field, fragments, resolved) : null
	return {
		kind,
		key: responseKeyOf(field),
		nodesKey,
		rows: {key: responseKeyOf(field), first, ...rows, nodesKey, hasNext: null},
		spaceKey: rows.columns.spaceId ?? null,
		...(target ? {target} : {}),
	}
}

/**
 * The `valuesList` a relations list selects on each row's `toEntity` (in the row, or in a connection's `nodes`), planned
 * as a values list of its own. Null when there is none or it cannot be planned: the target's fields then stay
 * `nested_entity`.
 */
function planTargetValues(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	resolved: Record<string, unknown>,
): LightList["target"] | null {
	const rowsOf = (set: SelectionSetNode | undefined): FieldNode[] => (set ? fieldsOf(set, fragments) : [])
	const direct = rowsOf(field.selectionSet)
	const nodes = direct.find((child) => child.name.value === "nodes")
	const toEntity = (nodes ? rowsOf(nodes.selectionSet) : direct).find((child) => child.name.value === "toEntity")
	const values = rowsOf(toEntity?.selectionSet).find((child) => child.name.value === "valuesList")
	if (!toEntity || !values) return null
	try {
		const list = planLightList(values, fragments, resolved, new Set())
		return list.kind === "values" && !list.propertyFilter ? {key: responseKeyOf(toEntity), values: list} : null
	} catch (error) {
		if (error instanceof Unsupported) return null
		throw error
	}
}

function planLightEntity(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	resolved: Record<string, unknown>,
	uncompared: Set<UncomparedReason>,
	options: {
		everySpace?: boolean
		/** The root argument names Spaces, which do not scope the nested lists: space-less rows are read every-Space. */
		rootSpaces?: boolean
		projections?: boolean
		relationsTypeFilter?: boolean
		derivedFields?: boolean
		/** `entity(id) { spaceIds }` alone is judged (every-Space read). */
		spaceIdsOnly?: boolean
	} = {},
): {card: EntityCardPlan; lists: LightList[]; everySpace: boolean} {
	const lists: LightList[] = []
	const card = planEntitySelection(
		set,
		fragments,
		resolved,
		uncompared,
		(field) => {
			lists.push(
				planLightList(field, fragments, resolved, uncompared, {
					projections: options.projections,
					relationsTypeFilter: options.relationsTypeFilter,
				}),
			)
		},
		{derivedFields: options.derivedFields},
	)
	// No lists and nothing to say where the entity lives: an `entity(id)` is read in every loaded Space, which
	// settles whether it exists (`{ id }`) and its name, description and types from all its Spaces.
	if (options.everySpace && lists.length === 0 && !card.spaceIds) {
		if (card.name || card.description || card.types) uncompared.add("partial_values")
		return {card, lists, everySpace: true}
	}
	// `entity(id) { spaceIds }` and nothing else: the Spaces the entity lives in are all there is to judge. It is read
	// in every loaded Space and the Spaces holding statements must be among the ones PostGraphile names.
	if (
		options.spaceIdsOnly &&
		options.everySpace &&
		lists.length === 0 &&
		card.spaceIds &&
		!card.name &&
		!card.description &&
		!card.types &&
		!card.createdAt &&
		!card.updatedAt
	) {
		uncompared.add("partial_values")
		return {card, lists, everySpace: true}
	}
	// No lists: the derived fields (name, description, types) of the entity's own spaces are all that is judged.
	// A root that names Spaces and asks for a creation or update time judges those derived times in them (the
	// ordered page of `orderBy: CREATED_AT_DESC` is read separately).
	const timed = options.rootSpaces === true && (card.createdAt || card.updatedAt)
	if (lists.length === 0 && !(card.spaceIds && (card.name || card.description || card.types)) && !timed)
		refuse("selection")
	// Rows with no space and no `spaceIds` leave nothing to say which cards to read: where the caller allows it
	// (no Space argument), the entity is read across every loaded Space and each row need only be held by one.
	if (!card.spaceIds && !lists.some((list) => list.spaceKey)) {
		if (!options.everySpace && !options.rootSpaces) refuse("no_space")
		uncompared.add("partial_values")
		return {card, lists, everySpace: true}
	}
	uncompared.add("partial_values")
	return {card, lists, everySpace: false}
}

/** `entity(id)` with the light selection. Same roots as the card shadow (`relations` is tolerated). */
export function analyzeLightEntity(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
	options: {relationsTypeFilter?: boolean} = {},
): LightAnalysis<LightEntityPlan> {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const uncompared = new Set<UncomparedReason>()
		const entityRoots = roots.filter((f) => f.name.value === ENTITY_FIELD)
		const root = entityRoots[0]
		if (entityRoots.length !== 1 || !root) return {ok: false, reason: "not_single_root"}
		for (const other of roots) {
			if (other === root) continue
			if (!(ENTITY_TOLERATED_ROOTS as readonly string[]).includes(other.name.value))
				return {ok: false, reason: "not_single_root"}
			uncompared.add("root_relations")
		}
		const args = argsOf(root, resolved)
		if (Object.keys(args).length !== 1 || !("id" in args)) return {ok: false, reason: "args"}
		const entityId = uuidArg(args.id)
		if (!root.selectionSet) return {ok: false, reason: "selection"}
		const {card, lists, everySpace} = planLightEntity(root.selectionSet, fragments, resolved, uncompared, {
			everySpace: true,
			spaceIdsOnly: true,
			relationsTypeFilter: options.relationsTypeFilter === true,
		})
		return {
			ok: true,
			plan: {
				kind: "entity",
				key: responseKeyOf(root),
				entityId,
				card,
				lists,
				everySpace,
				uncompared: [...uncompared],
			},
		}
	} catch (error) {
		return {ok: false, reason: error instanceof Unsupported ? error.reason : "selection"}
	}
}

/** The soundness claims of an `entities` filter, and the ids it lists. Anything else is only uncompared. */
function readFilter(
	filter: unknown,
	uncompared: Set<UncomparedReason>,
): Pick<LightEntitiesPlan, "claims" | "ids"> & {exact: boolean} {
	const claims: FilterClaim[] = []
	// Every clause was read into a claim or the id list, so the filter holds nothing the claims do not say.
	let exact = true
	let ids: string[] | null = null
	const object = filter === undefined || filter === null ? {} : asRecord(filter)
	if (!object) return refuse("filter")
	uncompared.add("filter_completeness")
	const intersect = (listed: string[]) => {
		ids = ids ? ids.filter((id) => listed.includes(id)) : listed
	}
	const uuids = (list: unknown[] | null): string[] | null =>
		list?.every((id) => typeof id === "string" && isValidUuid(id))
			? (list as string[]).map((id) => normalizeUuid(id))
			: null
	// The clauses of one filter object. Its keys are ANDed, and so are the elements of an `and` list, so a clause read
	// out of either holds for every returned row. (`or` and `not` are not descended into: they narrow nothing alone.)
	const read = (node: Record<string, unknown>, depth: number) => {
		for (const [key, clause] of Object.entries(node)) {
			const c = asRecord(clause)
			const single = c && Object.keys(c).length === 1 ? c : null
			if (key === "and" && Array.isArray(clause) && depth < 4) {
				for (const element of clause) {
					const inner = asRecord(element)
					if (inner) read(inner, depth + 1)
					else exact = false
				}
			} else if (key === "typeIds" && single) {
				const listed = uuids(
					typeof single.anyEqualTo === "string"
						? [single.anyEqualTo]
						: Array.isArray(single.overlaps) && single.overlaps.length > 0
							? single.overlaps
							: null,
				)
				if (listed) claims.push({kind: "type", typeIds: [...new Set(listed)]})
				else exact = false
			} else if (key === "id" && single) {
				const listed = uuids(
					typeof single.is === "string" ? [single.is] : Array.isArray(single.in) ? single.in : null,
				)
				if (listed) intersect(listed)
				else exact = false
			} else if (key === "relations" && single) {
				const some = asRecord(single.some)
				const type = asRecord(some?.typeId)
				const to = asRecord(some?.toEntityId)
				if (
					some &&
					Object.keys(some).length === 2 &&
					typeof type?.is === "string" &&
					isValidUuid(type.is) &&
					typeof to?.is === "string" &&
					isValidUuid(to.is)
				)
					claims.push({kind: "edge", typeId: normalizeUuid(type.is), toEntityId: normalizeUuid(to.is)})
				else exact = false
			} else exact = false
		}
	}
	read(object, 0)
	return {claims, ids, exact}
}

const ENTITIES_ARGS = [
	"filter",
	"orderBy",
	"first",
	"offset",
	"spaceId",
	"spaceIds",
	"typeId",
	"typeIds",
	"systemTypeId",
	"systemTypeIds",
]

/** The built-in sorts of the entities connection the frontends send, in either direction. */
const ENTITIES_ORDER_BY = /^(CREATED_AT|UPDATED_AT|RANKING_SCORE|QUALITY_SCORE|ID)_(ASC|DESC)$/

/**
 * `orderBy: [CREATED_AT_DESC, ID_ASC]` (one enum or a list). The sort decides which entities a page holds and in
 * what order, neither of which a card can settle, so a named ordering only adds `order_completeness`; each
 * returned entity is still judged on its own card. Anything but those enum names is refused.
 */
function readOrderBy(value: unknown, uncompared: Set<UncomparedReason>): EntitiesOrder | null {
	if (value === undefined || value === null) return null
	const names = Array.isArray(value) ? value : [value]
	if (names.length === 0 || names.length > 4) refuse("args")
	for (const name of names) if (typeof name !== "string" || !ENTITIES_ORDER_BY.test(name)) refuse("args")
	uncompared.add("order_completeness")
	const only = names.length === 1 ? /^(CREATED_AT|UPDATED_AT)_(ASC|DESC)$/.exec(names[0] as string) : null
	if (!only) return null
	return {
		property: only[1] === "CREATED_AT" ? "createdAt" : "updatedAt",
		direction: only[2] === "ASC" ? "asc" : "desc",
	}
}

/** `{in: [<uuid> ..]}` and nothing else (the shape of the `spaceIds`/`typeIds` arguments); the ids, distinct. */
function inList(value: unknown): string[] {
	const clause = asRecord(value)
	if (!clause || Object.keys(clause).length !== 1 || !Array.isArray(clause.in) || clause.in.length === 0)
		return refuse("args")
	return [...new Set(clause.in.map((id) => uuidArg(id)))]
}

/**
 * The root arguments that narrow `entities` by Space or Type (gaia's entitySpaceFilterPlugin): `spaceId`,
 * `spaceIds: {in}` (the entity lives in one of them), `typeId`, `typeIds: {in}` (it has one of them as a Type).
 * Space narrowing is returned, type narrowing becomes a claim; the system-type forms are only uncompared.
 * Null arguments are absent.
 */
function readRootArgs(
	args: Record<string, unknown>,
	claims: FilterClaim[],
	uncompared: Set<UncomparedReason>,
): string[] | null {
	const given = (name: string) => args[name] !== undefined && args[name] !== null
	let spaces: string[] | null = null
	if (given("spaceId")) spaces = [uuidArg(args.spaceId)]
	if (given("spaceIds")) {
		const listed = inList(args.spaceIds)
		spaces = spaces ? spaces.filter((id) => listed.includes(id)) : listed
		if (spaces.length === 0) return refuse("args")
	}
	if (given("typeId")) claims.push({kind: "type", typeIds: [uuidArg(args.typeId)]})
	if (given("typeIds")) claims.push({kind: "type", typeIds: inList(args.typeIds)})
	if (given("systemTypeId") || given("systemTypeIds")) uncompared.add("filter_completeness")
	return spaces
}

export const ORDERED_ROOT = "entitiesOrderedByProperty"
const ORDERED_ARGS = [
	"propertyId",
	"spaceIds",
	"sortDirection",
	"dataType",
	"typeIds",
	"includeWithoutValue",
	"votedBy",
	"votedByKinds",
	"votedByTypes",
	"entityIds",
	"first",
	"offset",
]

/**
 * `entitiesOrderedByProperty(propertyId, spaceIds, typeIds, ...)`: a database function ordering entities by a property's
 * value, answering a plain list. Its order and its vote/entity-id narrowing are a ranking no card settles, so they are
 * uncompared (`order_completeness`, `filter_completeness`); the Space and Type lists narrow like `entities`' own, as plain
 * lists rather than `{in: ..}`. Every returned entity is still judged on its own card.
 */
function readOrderedArgs(
	args: Record<string, unknown>,
	claims: FilterClaim[],
	uncompared: Set<UncomparedReason>,
): string[] | null {
	const given = (name: string) => args[name] !== undefined && args[name] !== null
	if (typeof args.propertyId !== "string") refuse("args")
	uuidArg(args.propertyId)
	uncompared.add("order_completeness")
	let spaces: string[] | null = null
	if (given("spaceIds")) {
		if (!Array.isArray(args.spaceIds)) return refuse("args")
		if (args.spaceIds.length > 0) spaces = [...new Set(args.spaceIds.map((id) => uuidArg(id)))]
	}
	if (given("typeIds")) {
		if (!Array.isArray(args.typeIds)) return refuse("args")
		if (args.typeIds.length > 0)
			claims.push({kind: "type", typeIds: [...new Set(args.typeIds.map((id) => uuidArg(id)))]})
	}
	for (const name of ["votedBy", "votedByKinds", "votedByTypes", "entityIds", "includeWithoutValue"])
		if (given(name)) uncompared.add("filter_completeness")
	return spaces
}

/**
 * `entities(filter, first, offset, spaceId, spaceIds, typeId, typeIds)` with the light selection, compared on a sample of
 * what it returns. With `connection`, the root is `entitiesConnection` (the same arguments plus `after`): its entities are
 * the `nodes` field, `totalCount` is uncompared (`total_count`) and `pageInfo` is not read. `edges` is not supported.
 */
export function analyzeLightEntities(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
	options: {
		entitiesBatch?: boolean
		relationsTypeFilter?: boolean
		derivedFields?: boolean
		connection?: boolean
		/** The root is `entitiesOrderedByProperty` (a database function answering a plain list; see readOrderedArgs). */
		ordered?: boolean
	} = {},
): LightAnalysis<LightEntitiesPlan> {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const root = roots[0]
		const rootName = options.ordered ? ORDERED_ROOT : options.connection ? "entitiesConnection" : "entities"
		if (roots.length !== 1 || !root || root.name.value !== rootName) return {ok: false, reason: "not_single_root"}
		if (!root.selectionSet) return {ok: false, reason: "selection"}
		const args = argsOf(root, resolved)
		for (const arg of Object.keys(args))
			if (options.ordered) {
				if (!ORDERED_ARGS.includes(arg)) refuse("args")
			} else if (!ENTITIES_ARGS.includes(arg) && !(options.connection && arg === "after")) refuse("args")
		const after = args.after
		if (after !== undefined && after !== null && typeof after !== "string") refuse("args")
		const uncompared = new Set<UncomparedReason>()
		let entitySet = root.selectionSet
		let nodesKey: string | null = null
		if (options.connection) {
			let nodes: FieldNode | null = null
			for (const child of fieldsOf(root.selectionSet, fragments)) {
				const name = child.name.value
				if (name === "__typename") continue
				if (name === "totalCount" && isPlainField(child)) uncompared.add("total_count")
				else if (name === "pageInfo" && child.selectionSet && !child.arguments?.length) continue
				else if (name === "nodes" && child.selectionSet && !child.arguments?.length && !nodes) nodes = child
				else refuse("selection")
			}
			if (!nodes?.selectionSet) return {ok: false, reason: "selection"}
			entitySet = nodes.selectionSet
			nodesKey = responseKeyOf(nodes)
		}
		for (const arg of ["first", "offset"])
			if (args[arg] !== undefined && args[arg] !== null && !Number.isInteger(args[arg])) refuse("args")
		const {claims, ids, exact} = options.ordered
			? {claims: [] as FilterClaim[], ids: null, exact: false}
			: readFilter(args.filter, uncompared)
		// A page after a cursor starts where PostGraphile's cursor says, so its place in the order is not checked.
		const readOrder = readOrderBy(args.orderBy, uncompared)
		const order = after === undefined || after === null ? readOrder : null
		const spaceIds = options.ordered
			? readOrderedArgs(args, claims, uncompared)
			: readRootArgs(args, claims, uncompared)
		const {card, lists, everySpace} = planLightEntity(entitySet, fragments, resolved, uncompared, {
			everySpace: spaceIds === null,
			rootSpaces: spaceIds !== null,
			projections: options.entitiesBatch === true,
			relationsTypeFilter: options.relationsTypeFilter === true,
			derivedFields: options.derivedFields === true,
		})
		if (!card.idKey) return {ok: false, reason: "selection"}
		uncompared.add("sampled_rows")
		return {
			ok: true,
			plan: {
				kind: "entities",
				key: responseKeyOf(root),
				card,
				lists,
				claims,
				ids,
				spaceIds,
				everySpace,
				order,
				exactFilter: exact && args.systemTypeId == null && args.systemTypeIds == null,
				first: typeof args.first === "number" ? args.first : null,
				offset: typeof args.offset === "number" ? args.offset : null,
				...(nodesKey !== null ? {nodesKey} : {}),
				uncompared: [...uncompared],
			},
		}
	} catch (error) {
		return {ok: false, reason: error instanceof Unsupported ? error.reason : "selection"}
	}
}

// ---------------------------------------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------------------------------------

/** One entity object of PostGraphile's answer, read for the light plan. */
type LightCard = {
	card: NonNullable<PostgresCard>
	/** The row spaces, parallel to `card.values` and `card.relations` (null: the row did not say). */
	valueSpaces: (string | null)[]
	relationSpaces: (string | null)[]
	/** The row spaces of the membership-only relation projections (`allRelationsList { spaceId }`). */
	projectionSpaces: (string | null)[]
	/** Rows of a `typeId`-filtered relations list that selects no type: the Types the filter allowed, the target, the row space. */
	typeless: {types: string[]; to: string; space: string | null}[]
	/** Rows of a relations list that selects a type and a target with no id: held when geo-db has a relation of the type. */
	typeOnly: {type: string; space: string | null}[]
	/** A relation target's own value rows, as the answer gave them: the target and the rows. */
	targetValues: {to: string; values: ValueRow[]}[]
	/** Value rows of a `propertyId`-filtered list that selects no property: held under one of `props`. */
	untypedValues: {props: string[]; row: ValueRow; space: string | null}[]
}

const push = (
	nested: NonNullable<NonNullable<PostgresCard>["nested"]>,
	object: "property" | "type" | "toEntity",
	ref: {id: string; name: string | null},
) => {
	const list = nested[object] ?? []
	list.push(ref)
	nested[object] = list
}

const spaceOf = (node: Record<string, unknown>, key: string | null): string | null => {
	const v = key ? node[key] : null
	return typeof v === "string" && isValidUuid(v) ? normalizeUuid(v) : null
}

/** A relation row's target's own value rows, or null when the answer does not show the list (then it is not compared). */
function targetValuesOf(node: Record<string, unknown>, target: NonNullable<LightList["target"]>): ValueRow[] | null {
	const held = asRecord(node[target.key])
	const inner = target.values
	const holder = inner.nodesKey ? asRecord(held?.[inner.key]) : held
	const rows = inner.nodesKey ? holder?.[inner.nodesKey] : held?.[inner.key]
	if (!Array.isArray(rows)) return null
	const own: ValueRow[] = []
	for (const row of rows) {
		const value = asRecord(row)
		const propertyId = value ? read(value, inner.rows, "propertyId") : null
		if (!value || typeof propertyId !== "string") return null
		own.push({
			property_id: propertyId,
			text: orNull(read(value, inner.rows, "text")),
			integer: orNull(read(value, inner.rows, "integer")),
			float: orNull(read(value, inner.rows, "float")),
			decimal: orNull(read(value, inner.rows, "decimal")),
			boolean: orNull(read(value, inner.rows, "boolean")),
			datetime: orNull(read(value, inner.rows, "datetime")),
			date: orNull(read(value, inner.rows, "date")),
		})
	}
	return own
}

/** Null when the object is not a plain answer for the plan. */
export function lightCardFromEntity(entity: unknown, plan: LightPlan): LightCard | null {
	const object = asRecord(entity)
	if (!object) return null
	const values: ValueRow[] = []
	const relations: RelationRow[] = []
	const valueSpaces: (string | null)[] = []
	const relationSpaces: (string | null)[] = []
	const projectionSpaces: (string | null)[] = []
	const typeless: LightCard["typeless"] = []
	const typeOnly: LightCard["typeOnly"] = []
	const targetValues: LightCard["targetValues"] = []
	const untypedValues: LightCard["untypedValues"] = []
	const nested: NonNullable<NonNullable<PostgresCard>["nested"]> = {}
	for (const list of plan.lists) {
		if (list.projection) {
			const nodes = object[list.key]
			if (Array.isArray(nodes))
				for (const raw of nodes) projectionSpaces.push(spaceOf(asRecord(raw) ?? {}, list.spaceKey))
			continue
		}
		const holder = list.nodesKey ? asRecord(object[list.key]) : object
		const nodes = list.nodesKey ? holder?.[list.nodesKey] : object[list.key]
		if (!Array.isArray(nodes)) return null
		for (const raw of nodes) {
			const node = asRecord(raw)
			if (!node) return null
			const space = spaceOf(node, list.spaceKey)
			if (list.kind === "values" && list.propertyFilter) {
				untypedValues.push({
					props: list.propertyFilter,
					row: {
						property_id: "",
						text: orNull(read(node, list.rows, "text")),
						integer: orNull(read(node, list.rows, "integer")),
						float: orNull(read(node, list.rows, "float")),
						decimal: orNull(read(node, list.rows, "decimal")),
						boolean: orNull(read(node, list.rows, "boolean")),
						datetime: orNull(read(node, list.rows, "datetime")),
						date: orNull(read(node, list.rows, "date")),
					},
					space,
				})
			} else if (list.kind === "values") {
				const propertyId = read(node, list.rows, "propertyId")
				if (typeof propertyId !== "string") return null
				values.push({
					property_id: propertyId,
					text: orNull(read(node, list.rows, "text")),
					integer: orNull(read(node, list.rows, "integer")),
					float: orNull(read(node, list.rows, "float")),
					decimal: orNull(read(node, list.rows, "decimal")),
					boolean: orNull(read(node, list.rows, "boolean")),
					datetime: orNull(read(node, list.rows, "datetime")),
					date: orNull(read(node, list.rows, "date")),
				})
				valueSpaces.push(space)
				if (list.rows.names.property)
					push(nested, "property", {
						id: propertyId,
						name: readName(node, list.rows.names.property) ?? null,
					})
			} else {
				const typeId = list.typeFilter ? null : read(node, list.rows, "typeId")
				if (list.typeOnly) {
					if (typeof typeId !== "string") return null
					typeOnly.push({type: typeId, space})
					continue
				}
				const toEntityId = read(node, list.rows, "toEntityId")
				if (typeof toEntityId !== "string") return null
				if (list.typeFilter) {
					typeless.push({types: list.typeFilter, to: toEntityId, space})
					if (list.rows.names.toEntity)
						push(nested, "toEntity", {
							id: toEntityId,
							name: readName(node, list.rows.names.toEntity) ?? null,
						})
					continue
				}
				if (typeof typeId !== "string") return null
				const own = list.target ? targetValuesOf(node, list.target) : null
				if (own) targetValues.push({to: toEntityId, values: own})
				relations.push({type_id: typeId, to_entity_id: toEntityId, to_space_id: null})
				relationSpaces.push(space)
				if (list.rows.names.type)
					push(nested, "type", {id: typeId, name: readName(node, list.rows.names.type) ?? null})
				if (list.rows.names.toEntity)
					push(nested, "toEntity", {
						id: toEntityId,
						name: readName(node, list.rows.names.toEntity) ?? null,
					})
			}
		}
	}
	const card: NonNullable<PostgresCard> = {values, relations, truncated: false}
	const text = (key: string | null) => (key && typeof object[key] === "string" ? (object[key] as string) : null)
	if (plan.card.name) card.name = text(plan.card.name)
	if (plan.card.updatedAt) card.updatedAt = secondsOrNull(object[plan.card.updatedAt])
	if (plan.card.createdAt) card.createdAt = secondsOrNull(object[plan.card.createdAt])
	if (plan.card.description) card.description = text(plan.card.description)
	if (plan.card.spaceIds) {
		const ids = object[plan.card.spaceIds]
		if (!Array.isArray(ids) || !ids.every((id) => typeof id === "string" && isValidUuid(id))) return null
		card.spaceIds = (ids as string[]).map((id) => normalizeUuid(id))
	}
	if (plan.card.types) {
		const nodes = object[plan.card.types.key]
		if (!Array.isArray(nodes)) return null
		const ids: string[] = []
		const names: (string | null)[] = []
		for (const node of nodes) {
			const id = plan.card.types.plain
				? node
				: plan.card.types.idKey
					? asRecord(node)?.[plan.card.types.idKey]
					: undefined
			const namesOnly = !plan.card.types.plain && !plan.card.types.idKey
			if (!namesOnly && (typeof id !== "string" || (plan.card.types.plain && !isValidUuid(id)))) return null
			if (typeof id === "string") ids.push(id)
			const name = plan.card.types.nameKey ? asRecord(node)?.[plan.card.types.nameKey] : undefined
			if (namesOnly && typeof name !== "string") return null
			names.push(typeof name === "string" ? name : null)
		}
		if (plan.card.types.plain || plan.card.types.idKey) card.types = ids
		if (plan.card.types.nameKey) card.typeNames = names
	}
	if (Object.keys(nested).length > 0) card.nested = nested
	return {card, valueSpaces, relationSpaces, projectionSpaces, typeless, typeOnly, targetValues, untypedValues}
}

/** The spaces to read: the entity's own, else the spaces its rows name. */
function lightSpaces(light: LightCard, extra: string[] | null): string[] | null {
	const spaces = new Set<string>(light.card.spaceIds ?? [])
	if (!light.card.spaceIds)
		for (const s of [...light.valueSpaces, ...light.relationSpaces, ...light.typeOnly.map((row) => row.space)])
			if (s) spaces.add(s)
	for (const s of extra ?? []) spaces.add(s)
	return spaces.size > 0 ? [...spaces] : null
}

export type LightCheck = CardCheck

/**
 * Whether the answer shows the entity in `spaces` only through derived properties: at least one of its rows sits in
 * them, and every such value property and relation type is derived. A membership-only relation row there counts as
 * derived only when the typed relations in that Space account for it.
 */
function onlyDerivedIn(light: LightCard, spaces: string[]): boolean {
	const wanted = new Set<string>(spaces.map((s) => normalizeUuid(s)))
	const inWanted = (space: string | null) => space !== null && wanted.has(space)
	let seen = 0
	for (const [at, row] of light.card.values.entries()) {
		if (!inWanted(light.valueSpaces[at] ?? null)) continue
		if (!DERIVED_PROPERTIES.has(conformingId("entity", row.property_id))) return false
		seen++
	}
	const typed = new Map<string, number>()
	for (const [at, row] of light.card.relations.entries()) {
		const space = light.relationSpaces[at] ?? null
		if (!space || !inWanted(space)) continue
		if (!DERIVED_PROPERTIES.has(conformingId("entity", row.type_id))) return false
		typed.set(space, (typed.get(space) ?? 0) + 1)
		seen++
	}
	const projected = new Map<string, number>()
	for (const space of light.projectionSpaces)
		if (space && inWanted(space)) projected.set(space, (projected.get(space) ?? 0) + 1)
	for (const [space, count] of projected) if (count > (typed.get(space) ?? 0)) return false
	return seen > 0
}

/**
 * Judges one entity's light answer against geo-db's cards of its spaces. `claims` are an `entities` filter's
 * promises about this entity, `spaceId` an `entities(spaceId:)` argument.
 */
export async function checkLight(
	deps: EntityShadowDeps,
	plan: LightPlan,
	light: LightCard,
	entityId: string,
	label: string,
	options: {claims?: FilterClaim[]; spaceIds?: string[] | null; everySpace?: boolean} = {},
): Promise<LightCheck> {
	const spaces = options.everySpace ? null : lightSpaces(light, options.spaceIds ?? null)
	if (!spaces && !options.everySpace) return {result: "unsupported", reason: "no_space"}
	const read = spaces ? await readWide(deps, entityId, spaces, label) : await readEverySpace(deps, entityId, label)
	if (!read.ok) return read.result === "failed" ? {result: "failed"} : {result: "unsupported", reason: read.reason}

	// Containment, in the row's own space when it names one, else against every space read.
	const missing: {space: string | null; prop: string; norm: string}[] = []
	const groups = new Map<string | null, {values: ValueRow[]; relations: RelationRow[]}>()
	const group = (space: string | null) => {
		const found = groups.get(space)
		if (found) return found
		const created = {values: [], relations: []}
		groups.set(space, created)
		return created
	}
	const seen = new Set<string>()
	light.card.values.forEach((row, at) => {
		const space = light.valueSpaces[at] ?? null
		const key = `v\t${space}\t${JSON.stringify(row)}`
		if (!seen.has(key)) group(space).values.push(row)
		seen.add(key)
	})
	light.card.relations.forEach((row, at) => {
		const space = light.relationSpaces[at] ?? null
		const key = `r\t${space}\t${row.type_id}\t${row.to_entity_id}`
		if (!seen.has(key)) group(space).relations.push(row)
		seen.add(key)
	})
	let skipped = 0
	for (const [space, rows] of groups) {
		const pool: GeoCardRow[] = space ? (read.bySpace.get(space) ?? []) : read.rows
		const diff = diffCards(statementsFromPostgres(rows.values, rows.relations), pool, {skipEmptyText: true})
		skipped += diff.skipped
		for (const s of diff.onlyPostgres) missing.push({space, prop: s.prop, norm: s.norm})
	}
	// A typeless row (filtered by `typeId`, no type selected) is held when its target sits under one of the filter's Types.
	const untyped = light.typeless.filter((row) => {
		const pool: GeoCardRow[] = row.space ? (read.bySpace.get(row.space) ?? []) : read.rows
		const types = new Set(row.types.map((id) => conformingId("entity", id)))
		// geo-db never holds Postgres-derived relations, so a filter of derived Types only is not judged.
		if ([...types].every((id) => DERIVED_PROPERTIES.has(id))) return false
		const to = conformingId("entity", row.to)
		return !pool.some((held) => types.has(held.prop) && held.raw === to)
	})
	for (const row of untyped) missing.push({space: row.space, prop: row.types.join("|"), norm: row.to})
	// A relation row whose target was not selected by id is held when geo-db has a relation of its type (derived
	// relation types are never in geo-db, so they are not judged).
	for (const row of light.typeOnly) {
		const type = conformingId("entity", row.type)
		if (DERIVED_PROPERTIES.has(type)) continue
		const pool: GeoCardRow[] = row.space ? (read.bySpace.get(row.space) ?? []) : read.rows
		if (!pool.some((held) => held.prop === type)) missing.push({space: row.space, prop: type, norm: ""})
	}
	// A value row filtered by `propertyId` with no property selected is held when geo-db has it under one of the filter's Properties.
	for (const row of light.untypedValues) {
		const pool: GeoCardRow[] = row.space ? (read.bySpace.get(row.space) ?? []) : read.rows
		const held = row.props.some((prop) => {
			const one = {...row.row, property_id: prop}
			return diffCards(statementsFromPostgres([one], []), pool, {skipEmptyText: true}).onlyPostgres.length === 0
		})
		if (!held) missing.push({space: row.space, prop: row.props.join("|"), norm: String(row.row.text ?? "")})
	}
	// A relation target's own values, held in geo-db's card of the target: all the targets in one batch read.
	let targetsCompared = false
	if (deps.nestedTargetValues === true && spaces && light.targetValues.length > 0 && deps.geoDb.readCards) {
		const targets = [...new Set(light.targetValues.map((row) => conformingId("entity", row.to)))]
		if (targets.length <= MAX_TARGET_CARDS && (!deps.budget || deps.budget.tryAcquire(1))) {
			try {
				const cards = await askGeoDbCards(deps, targets, spaces, {})
				targetsCompared = true
				const seenTargets = new Set<string>()
				for (const row of light.targetValues) {
					const target = conformingId("entity", row.to)
					const key = `${target}\t${JSON.stringify(row.values)}`
					if (seenTargets.has(key)) continue
					seenTargets.add(key)
					const diff = diffCards(statementsFromPostgres(row.values, []), cards.get(target) ?? [], {
						skipEmptyText: true,
					})
					skipped += diff.skipped
					for (const s of diff.onlyPostgres) missing.push({space: null, prop: s.prop, norm: s.norm})
				}
			} catch (error) {
				const reason = error instanceof GeoDbError ? error.reason : "error"
				if (reason === "unmapped_space") return {result: "unsupported", reason: "unmapped_space"}
				deps.log.warn(`${label} target read failed`, {entityId, spaces, reason, error: String(error)})
				return {result: "failed"}
			}
		}
	}
	// An every-Space read keys its Spaces by geo-db Space id, so the `spaceIds` PostGraphile named are mapped to those
	// before they are held to the Spaces geo-db holds statements in (a Space geo-db has not loaded cannot be one).
	let judged = light.card
	if (options.everySpace && light.card.spaceIds) {
		const mapped: string[] = []
		for (const space of light.card.spaceIds) {
			const geo = await deps.geoSpaceOf(space)
			if (geo) mapped.push(geo)
		}
		judged = {...light.card, spaceIds: mapped}
	}
	const derived = compareDerived(plan.card, judged, read.rows, read.scope)
	const wrong = Object.entries(derived.checks)
		.filter(([, result]) => result === "mismatch")
		.map(([field]) => field)

	// An `entities` filter's promises, on this entity's own cards.
	const unmet: FilterClaim[] = []
	// Read across every Space and found in none: nothing is known about the entity, so its claims are not judged.
	const unknown = options.everySpace === true && read.scope.populated.length === 0
	for (const claim of unknown ? [] : (options.claims ?? [])) {
		const wanted = claim.kind === "type" ? TYPES_PROPERTY : conformingId("entity", claim.typeId)
		const targets = new Set(
			claim.kind === "type"
				? claim.typeIds.map((id) => conformingId("entity", id))
				: [conformingId("entity", claim.toEntityId)],
		)
		if (!read.rows.some((row) => row.prop === wanted && targets.has(row.raw))) unmet.push(claim)
	}
	// An every-Space read keys its rows by geo-db Space id; the root's Spaces are PostGraphile ids. A root Space geo-db
	// has not loaded cannot be judged (the Space map answers null), so it is `unmapped_space`, not a mismatch.
	let heldIn = options.spaceIds ?? []
	if (options.everySpace && heldIn.length > 0) {
		const mapped: string[] = []
		for (const space of heldIn) {
			const geo = await deps.geoSpaceOf(space)
			if (!geo) return {result: "unsupported", reason: "unmapped_space"}
			mapped.push(geo)
		}
		heldIn = mapped
	}
	const notHeld = heldIn.length > 0 ? !heldIn.some((space) => (read.bySpace.get(space)?.length ?? 0) > 0) : false
	// PostGraphile counts an entity as in a Space when it has any value or outgoing relation there, derived ones
	// included; geo-db never holds those, so a Space the answer shows holding only derived rows (a vote Score in the
	// voter's Space) is not a membership geo-db can settle.
	const derivedOnly = notHeld && onlyDerivedIn(light, options.spaceIds ?? [])
	const outsideSpace = notHeld && !derivedOnly
	const bad = missing.length > 0 || wrong.length > 0 || unmet.length > 0 || outsideSpace
	// PostGraphile keeps an entity's row whatever its statements (a topic, or every statement since removed), so an
	// entity it has that geo-db holds nothing about in any loaded Space is not a difference geo-db can settle.
	const uncompared = [...derived.uncompared]
	if (derivedOnly) uncompared.push("derived_only")
	if (options.everySpace && read.scope.populated.length === 0) uncompared.push("no_statements")
	return {
		result: bad ? "mismatch" : "match",
		uncompared,
		detail: {
			entityId,
			spaces: read.scope.spaces.length,
			holders: options.everySpace ? read.scope.populated.length : undefined,
			missingInGeoDb: missing.length,
			sampleMissing: missing.slice(0, 5),
			skipped,
			derivedMismatches: wrong,
			unmetFilter: unmet.slice(0, 3),
			notInSpace: outsideSpace,
			targetsCompared,
			geoDbMs: read.ms,
		},
	}
}

/** Entities of an `entities` answer to compare: an even spread over the answer, at most the sample size. */
export function sampleNodes(nodes: unknown[], most: number = MAX_SAMPLED_ENTITIES): unknown[] {
	if (nodes.length <= most) return nodes
	if (most <= 1) return nodes.slice(0, most)
	const picks = new Set<number>()
	for (let i = 0; i < most; i++) picks.add(Math.floor((i * (nodes.length - 1)) / (most - 1)))
	return [...picks].map((at) => nodes[at])
}

/** What one light operation came to; the caller counts it under its own root field. */
export type LightOutcome =
	| {result: "skip"}
	| {result: "failed"}
	| {result: "unsupported"; reason: UnsupportedReason}
	| {result: "match" | "mismatch"; uncompared: UncomparedReason[]; detail: Record<string, unknown>}

/**
 * PostGraphile has no entity of this id: geo-db must hold no statement about it in any loaded Space. A Space geo-db
 * has not loaded cannot hold what PostGraphile lacks, so the loaded ones settle it.
 */
async function checkAbsent(deps: EntityShadowDeps, plan: LightEntityPlan, label: string): Promise<LightOutcome> {
	const read = await readEverySpace(deps, plan.entityId, label)
	if (!read.ok) return read.result === "failed" ? {result: "failed"} : {result: "unsupported", reason: read.reason}
	const holders = read.scope.populated
	return {
		result: holders.length > 0 ? "mismatch" : "match",
		uncompared: plan.uncompared,
		detail: {
			entityId: plan.entityId,
			absentInPostgres: true,
			spaces: read.scope.spaces.length,
			holders: holders.length,
			sampleHolders: holders.slice(0, 3),
			geoDbMs: read.ms,
		},
	}
}

/**
 * One `backlinks` / `backlinksList` field of an entity of the answer, held to geo-db's backlinks of that entity
 * (Space, relation type and referrer, as the selection names them). `depth` is the list's collection depth, which
 * sets PostGraphile's default page when the field has no `first`.
 */
async function checkBacklinkRows(
	deps: EntityShadowDeps,
	rows: BacklinkRowsPlan,
	node: unknown,
	entityId: string,
	depth: number,
	label: string,
): Promise<LightOutcome> {
	if (!deps.backlinks || !deps.geoDb.readBacklinks) return {result: "unsupported", reason: "selection"}
	const entity = asRecord(node)
	const postgres = entity ? extractBacklinkRows(entity, rows, depth) : null
	if (!postgres) return {result: "skip"}
	const {spaces, types, exact} = rows.filter
	return compareBacklinks(
		deps,
		{
			entityId,
			spaces,
			types,
			exactFilter: exact,
			bySpace: rows.space !== null || spaces?.length === 1,
			byType: rows.type !== null,
			postgres,
			uncompared: [],
		},
		label,
	)
}

/** Runs the comparison of a light plan over PostGraphile's parsed answer. Never throws. */
export async function runLight(
	deps: EntityShadowDeps,
	plan: LightEntityPlan | LightEntitiesPlan,
	body: unknown,
	label: string,
): Promise<LightOutcome> {
	const root = asRecord(body)
	const data = root && root.errors === undefined ? asRecord(root.data) : null
	if (!data || !(plan.key in data)) return {result: "skip"}
	const answer =
		plan.kind === "entities" && plan.nodesKey ? asRecord(data[plan.key])?.[plan.nodesKey] : data[plan.key]
	const entities: {id: string; node: unknown}[] = []
	let filterBreach: Record<string, unknown> | null = null
	/** PostGraphile's page in the order it came, for the ordered-page read. */
	const pageIds: string[] = []
	if (plan.kind === "entity") {
		if (answer === null) return plan.everySpace ? checkAbsent(deps, plan, label) : {result: "skip"}
		entities.push({id: plan.entityId, node: answer})
	} else {
		if (!Array.isArray(answer) || answer.length === 0) return {result: "skip"}
		const idKey = plan.card.idKey ?? ""
		const ids: string[] = []
		for (const node of answer) {
			const id = asRecord(node)?.[idKey]
			if (typeof id !== "string" || !isValidUuid(id)) return {result: "skip"}
			ids.push(normalizeUuid(id))
		}
		pageIds.push(...ids)
		// An `id: {in}` filter is exact: PostGraphile may return only the ids it was asked for.
		if (plan.ids) {
			const asked = new Set(plan.ids)
			const stray = ids.filter((id) => !asked.has(id))
			if (stray.length > 0) filterBreach = {strayIds: stray.slice(0, 3)}
		}
		for (const node of sampleNodes(answer, plan.everySpace ? MAX_EVERY_SPACE_SAMPLED : MAX_SAMPLED_ENTITIES)) {
			const id = asRecord(node)?.[idKey]
			entities.push({id: normalizeUuid(String(id)), node})
		}
	}
	const page =
		plan.kind === "entities" ? await checkOrderedPage(deps, plan, pageIds, label) : ({result: "skip"} as const)
	const checks: {id: string; check: LightCheck}[] = []
	const backlinkUncompared = new Set<UncomparedReason>()
	for (const {id, node} of entities) {
		const light = lightCardFromEntity(node, plan)
		if (!light) return {result: "skip"}
		const options =
			plan.kind === "entities"
				? {claims: plan.claims, spaceIds: plan.spaceIds, everySpace: plan.everySpace}
				: {everySpace: plan.everySpace}
		checks.push({id, check: await checkLight(deps, plan, light, id, label, options)})
		for (const rows of plan.card.backlinkRows ?? []) {
			const outcome = await checkBacklinkRows(deps, rows, node, id, plan.kind === "entity" ? 0 : 1, label)
			if (outcome.result === "match") for (const r of outcome.uncompared) backlinkUncompared.add(r)
			else if (outcome.result === "mismatch" || outcome.result === "failed") checks.push({id, check: outcome})
			// A question geo-db cannot be asked (flag off, unloaded Space, no budget) leaves the rows uncompared.
			else backlinkUncompared.add("backlink_rows")
		}
	}
	const mismatched = checks.filter((c) => c.check.result === "mismatch")
	if (mismatched.length > 0 || filterBreach || page.result === "mismatch") {
		return {
			result: "mismatch",
			uncompared: plan.uncompared,
			detail: {
				entities: checks.length,
				mismatches: mismatched.length,
				filterBreach,
				orderedPage: page.result === "mismatch" ? page.detail : undefined,
				first: mismatched[0]?.check.result === "mismatch" ? mismatched[0].check.detail : undefined,
			},
		}
	}
	const unsupported = checks.find((c) => c.check.result === "unsupported")?.check
	if (unsupported?.result === "unsupported") return {result: "unsupported", reason: unsupported.reason}
	if (checks.some((c) => c.check.result === "failed")) return {result: "failed"}
	// A page that could not be read leaves the order uncompared as it was: the cards stand on their own.
	const uncompared = new Set<UncomparedReason>(plan.uncompared)
	if (page.result === "match") {
		for (const r of page.uncompared) uncompared.add(r)
		// The page's members and order were compared, so which entities the filter and the order select is no longer open.
		if (page.pageCompared) {
			uncompared.delete("order_completeness")
			uncompared.delete("filter_completeness")
		}
	}
	for (const {check} of checks) if (check.result === "match") for (const r of check.uncompared) uncompared.add(r)
	for (const r of backlinkUncompared) uncompared.add(r)
	return {result: "match", uncompared: [...uncompared], detail: {entities: checks.length}}
}
