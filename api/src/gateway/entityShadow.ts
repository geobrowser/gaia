/**
 * Shadow of the root field `entity(id)`: PostGraphile answers, geo-db is asked the same question in the
 * background, and the two answers are compared.
 *
 * geo-db answers an entity's *card*: its values and its outgoing relations in one space (geodb/card.ts). The
 * shadow qualifies the selections a card can settle (at least in part) and counts every other selection as
 * `unsupported_selection` rather than guessing. The real frontend queries (`Entity`, `EntityPage`) look like:
 *
 *   entity(id: <uuid>) {
 *     id name description spaceIds updatedAt types { id name }
 *     allValuesList: valuesList(first: N) { spaceId propertyId }          // every space
 *     allRelationsList: relationsList(first: N) { spaceId }               // every space
 *     valuesList(first: N, filter: {spaceId: {is: S}}) { property { id name .. } text integer .. }
 *     relationsList(first: N, filter: {spaceId: {is: S}}) { id type { id name } toEntity { id name types .. } .. }
 *       | relations(first: N, after: null, filter: {spaceId: {is: S}}) { pageInfo { hasNextPage } nodes { .. } }
 *   }
 *   relations(filter: {entityId: {is: <id>}, spaceId: {is: S}}) { .. }    // EntityPage's second root field
 *
 * Statements (the two space-filtered lists) are mapped, normalised and diffed by the card shadow's own
 * functions. The derived fields are then checked against the same card rows in TypeScript:
 *
 *   - name, description: PostGraphile takes an arbitrary Name/Description value from *any* space (no ORDER BY,
 *     no space filter). The card's value in space S settles it exactly when S is the entity's only space, and
 *     is otherwise a membership check (a different value may come from another space: inconclusive).
 *   - types: the Types relation targets in the card must all be in PostGraphile's types; they must be equal
 *     when S is the only space.
 *   - spaceIds: S must be listed when the card has any statement; the other spaces are not checkable.
 *   - the unfiltered allValuesList propertyIds in S must be found among the card's rows.
 *
 * What a card cannot settle stays uncompared, by reason (metrics.ts): other spaces' rows, updatedAt, names of
 * nested entities and types, property metadata, nested entity data, the unfiltered allRelationsList, the
 * co-root `relations` field. A `match` covers the compared parts only.
 */

import {
	type DocumentNode,
	type FieldNode,
	Kind,
	type OperationDefinitionNode,
	type SelectionSetNode,
	valueFromASTUntyped,
} from "graphql"

import {
	conformingId,
	DERIVED_PROPERTIES,
	diffCards,
	isEmptyDiff,
	type RelationRow,
	statementsFromPostgres,
	type ValueRow,
} from "../geodb/card"
import {GeoDbError} from "../geodb/client"
import {askGeoDb, type CardDeps, type CardLogger} from "../geodb/service"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import type {FieldShadow} from "./gateway"
import {
	activateGatewayMetrics,
	recordGatewayShadow,
	recordGatewayUncompared,
	recordGatewayUnsupported,
	type UncomparedReason,
	type UnsupportedReason,
} from "./metrics"

export const ENTITY_FIELD = "entity"
/** Root fields tolerated beside `entity` (EntityPage asks for the relations that name the entity). */
export const ENTITY_TOLERATED_ROOTS = ["relations"] as const

/** PostGraphile's own page-size ceiling (paginationCapPlugin); a list this long may have been cut short. */
const PAGE_CAP = 1000
const SAMPLE = 5
const MAX_FRAGMENT_DEPTH = 8

const NAME_PROPERTY = conformingId("entity", "a126ca53-0c8e-48d5-b888-82c734c38935")
const DESCRIPTION_PROPERTY = conformingId("entity", "9b1f76ff-9711-404c-861e-59dc3fa7d037")
const TYPES_PROPERTY = conformingId("entity", "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")

const VALUE_COLUMNS = ["text", "integer", "float", "boolean", "datetime", "date", "decimal"] as const
/** Columns of a value row the card mapping does not compare; selecting them is allowed. */
const VALUE_IGNORED = ["spaceId", "language", "unit", "point", "time", "schedule"] as const
const RELATION_IGNORED = ["id", "spaceId", "entityId", "fromEntityId", "position", "verified", "toSpaceId"] as const

/** A nested object in a row: which compared column its `id` carries, and what its other fields are worth. */
type ObjectSpec = {idColumn: string | null; fields: Record<string, UncomparedReason | null>}

const PROPERTY_OBJECT: ObjectSpec = {
	idColumn: "propertyId",
	fields: {
		id: null,
		name: "property_info",
		dataTypeId: "property_info",
		dataTypeName: "property_info",
		renderableTypeId: "property_info",
		renderableTypeName: "property_info",
		format: "property_info",
		isType: "property_info",
		__typename: null,
	},
}
const RELATION_OBJECTS: Record<string, ObjectSpec> = {
	type: {idColumn: "typeId", fields: {id: null, name: "nested_names", __typename: null}},
	toEntity: {
		idColumn: "toEntityId",
		fields: {
			id: null,
			name: "nested_names",
			description: "nested_entity",
			types: "nested_entity",
			valuesList: "nested_entity",
			relationsList: "nested_entity",
			spaceIds: "nested_entity",
			__typename: null,
		},
	},
	fromEntity: {idColumn: null, fields: {id: null, name: "nested_names", __typename: null}},
	entity: {idColumn: null, fields: {id: null, name: "nested_names", __typename: null}},
}

type ListPlan = {
	/** Response key of the list (or of the connection) in the entity object. */
	key: string
	first: number
	/** Response key of each column the comparison reads. */
	columns: Record<string, string>
	/** Response key of `{id}` for a column selected as a nested object (`property { id }`), by column. */
	nested: Record<string, string>
	/** For a connection: the response key of its `nodes`, and of `pageInfo` and its `hasNextPage`. */
	nodesKey: string | null
	hasNext: {pageInfo: string; field: string} | null
}

export type EntityPlan = {
	/** Response key of the entity in `data`. */
	key: string
	entityId: string
	spaceId: string
	/** Response keys of the derived scalar fields, when selected. */
	name: string | null
	description: string | null
	spaceIds: string | null
	types: {key: string; idKey: string | null} | null
	/** The unfiltered lists (every space): their response keys, and of the columns read. */
	allValues: {key: string; first: number; spaceKey: string | null; propertyKey: string | null} | null
	allRelations: {key: string; first: number; spaceKey: string | null} | null
	values: ListPlan
	relations: ListPlan
	/** Parts of the selection no card can settle, known from the query alone. */
	uncompared: UncomparedReason[]
}

export type Analysis = {ok: true; plan: EntityPlan} | {ok: false; reason: UnsupportedReason}

class Unsupported extends Error {
	constructor(readonly reason: UnsupportedReason) {
		super(reason)
	}
}

const refuse = (reason: UnsupportedReason): never => {
	throw new Unsupported(reason)
}

/** The fields of a selection set with fragments inlined, in order. Directives anywhere are unsupported. */
function fieldsOf(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	depth = 0,
	out: FieldNode[] = [],
): FieldNode[] {
	if (depth > MAX_FRAGMENT_DEPTH) return refuse("selection")
	for (const selection of set.selections) {
		if (selection.directives?.length) refuse("directives")
		if (selection.kind === Kind.FIELD) out.push(selection)
		else if (selection.kind === Kind.INLINE_FRAGMENT) fieldsOf(selection.selectionSet, fragments, depth + 1, out)
		else {
			const fragment = fragments.get(selection.name.value)
			if (!fragment) refuse("selection")
			else fieldsOf(fragment, fragments, depth + 1, out)
		}
	}
	return out
}

const responseKeyOf = (field: FieldNode) => field.alias?.value ?? field.name.value

/** A field's arguments, resolved against the request's variables. */
function argsOf(field: FieldNode, variables: Record<string, unknown>): Record<string, unknown> {
	const args: Record<string, unknown> = {}
	for (const arg of field.arguments ?? []) args[arg.name.value] = valueFromASTUntyped(arg.value, variables)
	return args
}

function uuidArg(value: unknown): string {
	return typeof value === "string" && isValidUuid(value) ? normalizeUuid(value) : refuse("args")
}

/** `{spaceId: {is: <uuid>}}` and nothing else. */
function spaceFromFilter(filter: unknown): string {
	if (filter === undefined || filter === null) return refuse("no_space")
	if (typeof filter !== "object" || Array.isArray(filter)) return refuse("args")
	const keys = Object.keys(filter)
	if (keys.length !== 1 || keys[0] !== "spaceId") return refuse("args")
	const clause = (filter as {spaceId: unknown}).spaceId
	if (clause === null || typeof clause !== "object" || Array.isArray(clause)) return refuse("args")
	const clauseKeys = Object.keys(clause)
	if (clauseKeys.length !== 1 || clauseKeys[0] !== "is") return refuse("args")
	const is = (clause as {is: unknown}).is
	return is === undefined || is === null ? refuse("no_space") : uuidArg(is)
}

const isPlainField = (field: FieldNode) => !field.arguments?.length && !field.selectionSet

/** The columns (and nested `{id}` objects) of a list's rows, with fragments inlined. */
function planColumns(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	spec: {columns: readonly string[]; objects: Record<string, ObjectSpec>; ignored: readonly string[]},
	uncompared: Set<UncomparedReason>,
): {columns: Record<string, string>; nested: Record<string, string>} {
	const columns: Record<string, string> = {}
	const nested: Record<string, string> = {}
	const seen = new Set<string>()
	for (const child of fieldsOf(set, fragments)) {
		const name = child.name.value
		const key = responseKeyOf(child)
		if (seen.has(key)) refuse("selection")
		seen.add(key)
		if (name === "__typename") continue
		const object = spec.objects[name]
		if (object) {
			// A nested object may carry its id (compared, when it is a compared column) and fields the card
			// cannot settle (counted as uncompared). Its own arguments and sub-objects are not interpreted.
			if (!child.selectionSet || child.arguments?.length) throw new Unsupported("selection")
			for (const inner of fieldsOf(child.selectionSet, fragments)) {
				if (!(inner.name.value in object.fields)) refuse("selection")
				const reason = object.fields[inner.name.value]
				if (reason) uncompared.add(reason)
				else if (inner.name.value === "id" && object.idColumn) {
					if (!isPlainField(inner)) refuse("selection")
					nested[object.idColumn] ??= `${key}\t${responseKeyOf(inner)}`
				}
			}
			continue
		}
		if (!isPlainField(child)) refuse(child.selectionSet ? "selection" : "args")
		if (spec.columns.includes(name)) columns[name] ??= key
		else if (!spec.ignored.includes(name)) refuse("selection")
	}
	return {columns, nested}
}

type PlannedList = {list: ListPlan; spaceId: string}

/** A space-filtered `valuesList`/`relationsList`, or (`connection`) `relations(first, after, filter) { nodes }`. */
function planList(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	variables: Record<string, unknown>,
	spec: {columns: readonly string[]; objects: Record<string, ObjectSpec>; ignored: readonly string[]},
	uncompared: Set<UncomparedReason>,
	connection: boolean,
): PlannedList {
	const args = argsOf(field, variables)
	for (const name of Object.keys(args)) if (name !== "first" && name !== "filter" && name !== "after") refuse("args")
	// A later page of a connection cannot be compared with the card's first page.
	if (args.after !== undefined && args.after !== null && (!connection || args.after !== null)) refuse("args")
	const first = args.first
	if (typeof first !== "number" || !Number.isInteger(first) || first < 1) throw new Unsupported("args")
	const spaceId = spaceFromFilter(args.filter)
	if (!field.selectionSet) refuse("selection")
	const listKey = responseKeyOf(field)
	if (!connection) {
		const {columns, nested} = planColumns(field.selectionSet as SelectionSetNode, fragments, spec, uncompared)
		return {list: {key: listKey, first, columns, nested, nodesKey: null, hasNext: null}, spaceId}
	}
	let nodesKey: string | null = null
	let hasNext: ListPlan["hasNext"] = null
	let rows: {columns: Record<string, string>; nested: Record<string, string>} | null = null
	for (const child of fieldsOf(field.selectionSet as SelectionSetNode, fragments)) {
		const name = child.name.value
		if (name === "__typename") continue
		if (child.arguments?.length || !child.selectionSet) throw new Unsupported("selection")
		if (name === "nodes" && !nodesKey) {
			nodesKey = responseKeyOf(child)
			rows = planColumns(child.selectionSet, fragments, spec, uncompared)
		} else if (name === "pageInfo" && !hasNext) {
			for (const inner of fieldsOf(child.selectionSet, fragments)) {
				if (!["hasNextPage", "endCursor", "__typename"].includes(inner.name.value) || !isPlainField(inner))
					refuse("selection")
				if (inner.name.value === "hasNextPage")
					hasNext ??= {pageInfo: responseKeyOf(child), field: responseKeyOf(inner)}
			}
		} else refuse("selection")
	}
	if (!nodesKey || !rows) return refuse("selection")
	return {list: {key: listKey, first, columns: rows.columns, nested: rows.nested, nodesKey, hasNext}, spaceId}
}

/** The unfiltered `valuesList`/`relationsList`: only `spaceId` (and for values `propertyId`) are read. */
function planAllList(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	variables: Record<string, unknown>,
	allowed: readonly string[],
) {
	const args = argsOf(field, variables)
	for (const name of Object.keys(args)) if (name !== "first") refuse("args")
	const first = args.first
	if (typeof first !== "number" || !Number.isInteger(first) || first < 1) throw new Unsupported("args")
	if (!field.selectionSet) refuse("selection")
	const columns: Record<string, string> = {}
	for (const child of fieldsOf(field.selectionSet as SelectionSetNode, fragments)) {
		const name = child.name.value
		if (name === "__typename") continue
		if (!allowed.includes(name) || !isPlainField(child)) refuse("selection")
		columns[name] ??= responseKeyOf(child)
	}
	return {key: responseKeyOf(field), first, columns}
}

/** Decides whether an `entity(id)` operation is one a card can settle, and where its answers sit in the response. */
export function analyzeEntityOperation(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
): Analysis {
	try {
		const fragments = new Map<string, SelectionSetNode>()
		const operations: OperationDefinitionNode[] = []
		for (const definition of document.definitions) {
			if (definition.kind === Kind.FRAGMENT_DEFINITION)
				fragments.set(definition.name.value, definition.selectionSet)
			else if (definition.kind === Kind.OPERATION_DEFINITION) operations.push(definition)
		}
		const operation = operationName
			? operations.find((op) => op.name?.value === operationName)
			: operations.length === 1
				? operations[0]
				: undefined
		if (!operation || operation.operation !== "query") return {ok: false, reason: "not_single_root"}
		if (operation.directives?.length) return {ok: false, reason: "directives"}

		// A variable the request left out takes its declared default.
		const resolved = {...variables}
		for (const definition of operation.variableDefinitions ?? []) {
			const name = definition.variable.name.value
			if (resolved[name] === undefined && definition.defaultValue) {
				resolved[name] = valueFromASTUntyped(definition.defaultValue)
			}
		}

		const uncompared = new Set<UncomparedReason>()
		const roots = fieldsOf(operation.selectionSet, fragments).filter((f) => f.name.value !== "__typename")
		const entityRoots = roots.filter((f) => f.name.value === ENTITY_FIELD)
		const root = entityRoots[0]
		if (entityRoots.length !== 1 || !root) return {ok: false, reason: "not_single_root"}
		for (const other of roots) {
			if (other === root) continue
			if (!(ENTITY_TOLERATED_ROOTS as readonly string[]).includes(other.name.value))
				return {ok: false, reason: "not_single_root"}
			uncompared.add("root_relations")
		}
		const rootArgs = argsOf(root, resolved)
		if (Object.keys(rootArgs).length !== 1 || !("id" in rootArgs)) return {ok: false, reason: "args"}
		const entityId = uuidArg(rootArgs.id)
		if (!root.selectionSet) return {ok: false, reason: "selection"}

		let values: PlannedList | null = null
		let relations: PlannedList | null = null
		let name: string | null = null
		let description: string | null = null
		let spaceIds: string | null = null
		let types: EntityPlan["types"] = null
		let allValues: EntityPlan["allValues"] = null
		let allRelations: EntityPlan["allRelations"] = null
		const seen = new Set<string>()
		const valueSpec = {
			columns: ["propertyId", ...VALUE_COLUMNS],
			objects: {property: PROPERTY_OBJECT},
			ignored: VALUE_IGNORED,
		}
		const relationSpec = {
			columns: ["typeId", "toEntityId"],
			objects: RELATION_OBJECTS,
			ignored: RELATION_IGNORED,
		}
		for (const field of fieldsOf(root.selectionSet, fragments)) {
			const key = responseKeyOf(field)
			if (seen.has(key)) return {ok: false, reason: "selection"}
			seen.add(key)
			const fieldName = field.name.value
			const filtered = argsOf(field, resolved).filter !== undefined
			if (fieldName === "id" || fieldName === "__typename") {
				if (field.arguments?.length) return {ok: false, reason: "args"}
				if (field.selectionSet) return {ok: false, reason: "selection"}
			} else if (fieldName === "name" && !name && isPlainField(field)) {
				name = key
			} else if (fieldName === "description" && !description && isPlainField(field)) {
				description = key
			} else if (fieldName === "spaceIds" && !spaceIds && isPlainField(field)) {
				spaceIds = key
			} else if (fieldName === "updatedAt" && isPlainField(field)) {
				uncompared.add("updated_at")
			} else if (fieldName === "types" && !types && !field.arguments?.length && field.selectionSet) {
				let idKey: string | null = null
				for (const inner of fieldsOf(field.selectionSet, fragments)) {
					if (!isPlainField(inner)) return {ok: false, reason: "selection"}
					if (inner.name.value === "id") idKey ??= responseKeyOf(inner)
					else if (inner.name.value === "name") uncompared.add("nested_names")
					else if (inner.name.value !== "__typename") return {ok: false, reason: "selection"}
				}
				if (!idKey) return {ok: false, reason: "selection"}
				types = {key, idKey}
			} else if (fieldName === "valuesList" && filtered && !values) {
				values = planList(field, fragments, resolved, valueSpec, uncompared, false)
			} else if (fieldName === "valuesList" && !filtered && !allValues) {
				const all = planAllList(field, fragments, resolved, ["spaceId", "propertyId"])
				allValues = {
					key: all.key,
					first: all.first,
					spaceKey: all.columns.spaceId ?? null,
					propertyKey: all.columns.propertyId ?? null,
				}
			} else if (fieldName === "relationsList" && filtered && !relations) {
				relations = planList(field, fragments, resolved, relationSpec, uncompared, false)
			} else if (fieldName === "relations" && !relations) {
				relations = planList(field, fragments, resolved, relationSpec, uncompared, true)
			} else if (fieldName === "relationsList" && !filtered && !allRelations) {
				const all = planAllList(field, fragments, resolved, ["spaceId"])
				allRelations = {key: all.key, first: all.first, spaceKey: all.columns.spaceId ?? null}
				uncompared.add("membership_only")
			} else return {ok: false, reason: "selection"}
		}
		// A card does not say which rows are values and which relations, so it can settle both lists or neither.
		if (!values || !relations) return {ok: false, reason: values || relations ? "selection" : "no_space"}
		if (values.spaceId !== relations.spaceId) return {ok: false, reason: "args"}
		for (const column of ["propertyId", ...VALUE_COLUMNS]) {
			if (!values.list.columns[column] && !values.list.nested[column]) return {ok: false, reason: "selection"}
		}
		for (const column of ["typeId", "toEntityId"]) {
			if (!relations.list.columns[column] && !relations.list.nested[column])
				return {ok: false, reason: "selection"}
		}
		return {
			ok: true,
			plan: {
				key: responseKeyOf(root),
				entityId,
				spaceId: values.spaceId,
				name,
				description,
				spaceIds,
				types,
				allValues,
				allRelations,
				values: values.list,
				relations: relations.list,
				uncompared: [...uncompared],
			},
		}
	} catch (error) {
		if (error instanceof Unsupported) return {ok: false, reason: error.reason}
		return {ok: false, reason: "selection"}
	}
}

const asRecord = (v: unknown): Record<string, unknown> | null =>
	v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null

function read(node: Record<string, unknown>, list: ListPlan, column: string): unknown {
	const direct = list.columns[column]
	if (direct !== undefined) return node[direct]
	const [object, id] = (list.nested[column] ?? "").split("\t")
	return object && id ? asRecord(node[object])?.[id] : undefined
}

const orNull = <T>(v: unknown): T | null => (v === undefined || v === null ? null : (v as T))

export type PostgresCard = {
	values: ValueRow[]
	relations: RelationRow[]
	/** One of the compared lists may have been cut short (a full page, or `hasNextPage`). */
	truncated: boolean
	/** Derived fields, `undefined` when not selected. */
	name?: string | null
	description?: string | null
	spaceIds?: string[]
	types?: string[]
	/** The unfiltered lists' rows, `null` when the list is absent, `truncated` when it filled its page. */
	allValues?: {rows: {spaceId: string | null; propertyId: string | null}[]; truncated: boolean}
	allRelations?: {rows: {spaceId: string | null}[]; truncated: boolean}
} | null

const isFull = (count: number, first: number) => count >= Math.min(first, PAGE_CAP)

/**
 * Reads the answer out of PostGraphile's response. Null when it is not a plain successful answer for this
 * plan (errors, no data, an unexpected shape): that is not a verdict on geo-db, so nothing is counted.
 */
export function extractPostgresCard(body: unknown, plan: EntityPlan): PostgresCard {
	const root = asRecord(body)
	if (!root || root.errors !== undefined) return null
	const data = asRecord(root.data)
	if (!data || !(plan.key in data)) return null
	const entity = data[plan.key]
	// An entity PostGraphile does not have holds no statements and no derived data.
	if (entity === null) {
		return {
			values: [],
			relations: [],
			truncated: false,
			...(plan.name ? {name: null} : {}),
			...(plan.description ? {description: null} : {}),
			...(plan.spaceIds ? {spaceIds: []} : {}),
			...(plan.types ? {types: []} : {}),
			...(plan.allValues ? {allValues: {rows: [], truncated: false}} : {}),
			...(plan.allRelations ? {allRelations: {rows: [], truncated: false}} : {}),
		}
	}
	const entityObject = asRecord(entity)
	if (!entityObject) return null
	const listOf = (list: ListPlan): {nodes: unknown[]; hasNext: boolean} | null => {
		const holder = list.nodesKey ? asRecord(entityObject[list.key]) : entityObject
		const nodes = list.nodesKey ? holder?.[list.nodesKey] : entityObject[list.key]
		if (!Array.isArray(nodes)) return null
		const pageInfo = list.hasNext ? asRecord(holder?.[list.hasNext.pageInfo]) : null
		return {nodes, hasNext: list.hasNext ? pageInfo?.[list.hasNext.field] === true : false}
	}
	const valueList = listOf(plan.values)
	const relationList = listOf(plan.relations)
	if (!valueList || !relationList) return null
	const values: ValueRow[] = []
	for (const raw of valueList.nodes) {
		const node = asRecord(raw)
		if (!node) return null
		const propertyId = read(node, plan.values, "propertyId")
		if (typeof propertyId !== "string") return null
		values.push({
			property_id: propertyId,
			text: orNull(read(node, plan.values, "text")),
			integer: orNull(read(node, plan.values, "integer")),
			float: orNull(read(node, plan.values, "float")),
			decimal: orNull(read(node, plan.values, "decimal")),
			boolean: orNull(read(node, plan.values, "boolean")),
			datetime: orNull(read(node, plan.values, "datetime")),
			date: orNull(read(node, plan.values, "date")),
		})
	}
	const relations: RelationRow[] = []
	for (const raw of relationList.nodes) {
		const node = asRecord(raw)
		if (!node) return null
		const typeId = read(node, plan.relations, "typeId")
		const toEntityId = read(node, plan.relations, "toEntityId")
		if (typeof typeId !== "string" || typeof toEntityId !== "string") return null
		relations.push({type_id: typeId, to_entity_id: toEntityId, to_space_id: null})
	}
	const card: NonNullable<PostgresCard> = {
		values,
		relations,
		truncated:
			isFull(valueList.nodes.length, plan.values.first) ||
			(plan.relations.nodesKey ? relationList.hasNext : isFull(relationList.nodes.length, plan.relations.first)),
	}
	const stringOrNull = (key: string) => {
		const v = entityObject[key]
		return typeof v === "string" ? v : null
	}
	if (plan.name) card.name = stringOrNull(plan.name)
	if (plan.description) card.description = stringOrNull(plan.description)
	if (plan.spaceIds) {
		const ids = entityObject[plan.spaceIds]
		if (!Array.isArray(ids) || !ids.every((id) => typeof id === "string")) return null
		card.spaceIds = (ids as string[]).map((id) => normalizeUuid(id))
	}
	if (plan.types) {
		const nodes = entityObject[plan.types.key]
		if (!Array.isArray(nodes)) return null
		const ids: string[] = []
		for (const node of nodes) {
			const id = plan.types.idKey ? asRecord(node)?.[plan.types.idKey] : undefined
			if (typeof id !== "string") return null
			ids.push(id)
		}
		card.types = ids
	}
	if (plan.allValues) {
		const nodes = entityObject[plan.allValues.key]
		if (!Array.isArray(nodes)) return null
		const {spaceKey, propertyKey} = plan.allValues
		card.allValues = {
			rows: nodes.map((n) => ({
				spaceId: spaceKey ? orNull<string>(asRecord(n)?.[spaceKey]) : null,
				propertyId: propertyKey ? orNull<string>(asRecord(n)?.[propertyKey]) : null,
			})),
			truncated: isFull(nodes.length, plan.allValues.first),
		}
	}
	if (plan.allRelations) {
		const nodes = entityObject[plan.allRelations.key]
		if (!Array.isArray(nodes)) return null
		const {spaceKey} = plan.allRelations
		card.allRelations = {
			rows: nodes.map((n) => ({spaceId: spaceKey ? orNull<string>(asRecord(n)?.[spaceKey]) : null})),
			truncated: isFull(nodes.length, plan.allRelations.first),
		}
	}
	return card
}

export type CheckResult = "ok" | "mismatch" | "inconclusive"
export type DerivedChecks = {
	/** Per derived field: did geo-db's card agree? Only fields the query selected appear. */
	checks: Partial<Record<"name" | "description" | "types" | "spaceIds" | "allValuesList", CheckResult>>
	/** Reasons found while comparing (beyond the ones the query alone implies). */
	uncompared: UncomparedReason[]
}

type GeoRow = {prop: string; raw: string}

const sameSpace = (a: string | null | undefined, b: string) => a !== null && a !== undefined && normalizeUuid(a) === b

/**
 * Checks the derived fields against the card's rows (see the file comment for what each check means).
 * `card.truncated` must be false.
 */
export function compareDerived(plan: EntityPlan, card: NonNullable<PostgresCard>, geo: GeoRow[]): DerivedChecks {
	const checks: DerivedChecks["checks"] = {}
	const uncompared = new Set<UncomparedReason>()
	const space = plan.spaceId

	// The spaces PostGraphile attributes the entity to, when the answer says: `spaceIds`, else every row's space.
	let known: Set<string> | null = null
	if (card.spaceIds) known = new Set(card.spaceIds)
	else if (card.allValues && card.allRelations && !card.allValues.truncated && !card.allRelations.truncated) {
		const spaces = [...card.allValues.rows, ...card.allRelations.rows].map((r) => r.spaceId)
		if (spaces.every((s) => s !== null)) known = new Set((spaces as string[]).map((s) => normalizeUuid(s)))
	}
	const onlyThisSpace = known !== null && [...known].every((s) => s === space)

	const textField = (field: "name" | "description", property: string) => {
		const value = card[field]
		if (value === undefined) return
		const geoValues = geo.filter((row) => row.prop === property).map((row) => row.raw)
		if (value === null) checks[field] = geoValues.length === 0 ? "ok" : "mismatch"
		else if (geoValues.includes(value)) checks[field] = "ok"
		else if (onlyThisSpace) checks[field] = "mismatch"
		else {
			// Not this space's value, and other spaces could have supplied it: not decidable from the card.
			checks[field] = "inconclusive"
			uncompared.add("cross_space")
		}
	}
	textField("name", NAME_PROPERTY)
	textField("description", DESCRIPTION_PROPERTY)

	if (card.types !== undefined) {
		const postgres = new Set(card.types.map((id) => conformingId("entity", id)))
		const geoTypes = new Set(geo.filter((row) => row.prop === TYPES_PROPERTY).map((row) => row.raw))
		const missing = [...geoTypes].some((id) => !postgres.has(id))
		const extra = [...postgres].some((id) => !geoTypes.has(id))
		if (missing || (extra && onlyThisSpace)) checks.types = "mismatch"
		else if (extra) {
			checks.types = "inconclusive"
			uncompared.add("cross_space")
		} else checks.types = "ok"
	}

	if (card.spaceIds !== undefined) {
		// The space has statements in geo-db, so PostGraphile must list it. Other spaces are not checkable.
		checks.spaceIds = geo.length > 0 && !card.spaceIds.some((s) => s === space) ? "mismatch" : "ok"
		if (card.spaceIds.some((s) => s !== space)) uncompared.add("cross_space")
	}

	if (card.allValues) {
		const {rows, truncated} = card.allValues
		const scoped = plan.allValues?.spaceKey && plan.allValues.propertyKey
		if (truncated) uncompared.add("truncated")
		else if (!scoped) uncompared.add("membership_only")
		else {
			// This space's property ids must each be found among the card's rows (derived properties are not in geo-db).
			const pool = geo.map((row) => row.prop)
			let missing = false
			for (const row of rows) {
				if (!sameSpace(row.spaceId, space)) continue
				if (row.propertyId === null) continue
				const prop = conformingId("entity", row.propertyId)
				if (DERIVED_PROPERTIES.has(prop)) continue
				const at = pool.indexOf(prop)
				if (at < 0) missing = true
				else pool.splice(at, 1)
			}
			checks.allValuesList = missing ? "mismatch" : "ok"
			if (rows.some((row) => !sameSpace(row.spaceId, space))) uncompared.add("cross_space")
		}
	}
	if (card.allRelations?.truncated) uncompared.add("truncated")
	if (
		card.allRelations &&
		!card.allRelations.truncated &&
		card.allRelations.rows.some((r) => !sameSpace(r.spaceId, space))
	)
		uncompared.add("cross_space")
	return {checks, uncompared: [...uncompared]}
}

export type EntityShadowDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {log: CardLogger}

/** The `entity` shadow for the gateway. Registers its metric series; never throws. */
export function createEntityShadow(deps: EntityShadowDeps): FieldShadow {
	activateGatewayMetrics(ENTITY_FIELD)
	const shadow: FieldShadow = async ({params, document, classification, response}) => {
		try {
			const analysis = analyzeEntityOperation(document, classification.operationName, params.variables)
			if (!analysis.ok) {
				recordGatewayShadow(ENTITY_FIELD, "unsupported_selection")
				recordGatewayUnsupported(ENTITY_FIELD, analysis.reason)
				return
			}
			const {plan} = analysis
			let body: unknown
			try {
				body = await response.json()
			} catch {
				return
			}
			const postgres = extractPostgresCard(body, plan)
			if (!postgres) {
				deps.log.info("gateway entity shadow skipped: PostGraphile answer not comparable", {
					entityId: plan.entityId,
				})
				return
			}
			if (postgres.truncated) {
				recordGatewayShadow(ENTITY_FIELD, "unsupported_selection")
				recordGatewayUnsupported(ENTITY_FIELD, "truncated")
				return
			}
			const started = performance.now()
			let rows: Awaited<ReturnType<typeof askGeoDb>>
			try {
				rows = await askGeoDb(deps, conformingId("entity", plan.entityId), plan.spaceId)
			} catch (error) {
				recordGatewayShadow(ENTITY_FIELD, "failed")
				deps.log.warn("gateway entity shadow read failed", {
					entityId: plan.entityId,
					spaceId: plan.spaceId,
					reason: error instanceof GeoDbError ? error.reason : "error",
					message: String(error),
				})
				return
			}
			const diff = diffCards(statementsFromPostgres(postgres.values, postgres.relations), rows)
			const derived = compareDerived(plan, postgres, rows)
			for (const reason of new Set([...plan.uncompared, ...derived.uncompared]))
				recordGatewayUncompared(ENTITY_FIELD, reason)
			const wrong = Object.entries(derived.checks)
				.filter(([, result]) => result === "mismatch")
				.map(([field]) => field)
			if (isEmptyDiff(diff) && wrong.length === 0) {
				recordGatewayShadow(ENTITY_FIELD, "match")
				return
			}
			recordGatewayShadow(ENTITY_FIELD, "mismatch")
			deps.log.warn("gateway entity shadow mismatch", {
				entityId: plan.entityId,
				spaceId: plan.spaceId,
				operationName: classification.operationName,
				onlyPostgres: diff.onlyPostgres.length,
				onlyGeoDb: diff.onlyGeoDb.length,
				skipped: diff.skipped,
				derivedMismatches: wrong,
				samplePostgres: diff.onlyPostgres.slice(0, SAMPLE),
				sampleGeoDb: diff.onlyGeoDb.slice(0, SAMPLE),
				geoDbMs: Math.round(performance.now() - started),
			})
		} catch (error) {
			// Whatever went wrong, the user's response is already gone; count it and move on.
			try {
				recordGatewayShadow(ENTITY_FIELD, "failed")
				deps.log.warn("gateway entity shadow errored", {message: String(error)})
			} catch {}
		}
	}
	shadow.tolerates = ENTITY_TOLERATED_ROOTS
	return shadow
}
