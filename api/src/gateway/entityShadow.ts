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
 * The shadow asks geo-db for the card with `names` (geodb/card.ts), which adds the display names of each row's
 * property and of a reference value's target, resolved in the request's Space. They settle `property { name }`,
 * `type { name }`, `toEntity { name }`, `types { name }` and `fromEntity { name }` (the card entity's own name,
 * judged as `name` is). PostGraphile took each of those names from any space and the spaces of a nested entity
 * are not known, so a name PostGraphile lacks that geo-db has is a mismatch, and a different or missing geo-db
 * name is inconclusive (cross_space).
 *
 * What a card cannot settle stays uncompared, by reason (metrics.ts): other spaces' rows, updatedAt, the name
 * of a relation's own entity (`entity { name }`), property metadata beyond the name, nested entity data, the
 * unfiltered allRelationsList, the co-root `relations` field. A `match` covers the compared parts only.
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
	entityTimesOf,
	type GeoCardRow,
	isEmptyDiff,
	type RelationRow,
	statementsFromPostgres,
	type ValueRow,
} from "../geodb/card"
import {GeoDbError, isHoldersUnknownArgument, MAX_LINE_SPACES} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import {askGeoDb, askGeoDbSpaces, type CardDeps, type CardLogger} from "../geodb/service"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import {analyzeBacklinks, type BacklinkRowsPlan, readBacklinkFilter, runBacklinks} from "./backlinksShadow"
import type {FieldShadow} from "./gateway"
import {analyzeLightEntity, type LightOutcome, noteUnsupported, runLight} from "./lightShadow"
import {
	activateGatewayMetrics,
	recordGatewayShadow,
	recordGatewayUncompared,
	recordGatewayUnsupported,
	type UncomparedReason,
	type UnsupportedReason,
} from "./metrics"
import {
	checkNestedEntities,
	collectNestedEntities,
	type NestedCollector,
	type NestedEntities,
	type NestedEntityPlans,
	type NestedObjectName,
	planNestedEntity,
} from "./nestedEntity"
import {
	checkPropertyInfo,
	checkRelationsMembership,
	collectPropertyInfo,
	type PropertyInfoPlan,
	type PropertyInfos,
	planPropertyInfo,
} from "./propertyInfo"
import type {ReadBudget} from "./readBudget"

export const ENTITY_FIELD = "entity"
/** Root fields tolerated beside `entity` (EntityPage asks for the relations that name the entity). */
export const ENTITY_TOLERATED_ROOTS = ["relations"] as const

/** PostGraphile's own page-size ceiling (paginationCapPlugin); a list this long may have been cut short. */
const PAGE_CAP = 1000
const SAMPLE = 5
const MAX_FRAGMENT_DEPTH = 8

export const NAME_PROPERTY = conformingId("entity", "a126ca53-0c8e-48d5-b888-82c734c38935")
export const DESCRIPTION_PROPERTY = conformingId("entity", "9b1f76ff-9711-404c-861e-59dc3fa7d037")
export const TYPES_PROPERTY = conformingId("entity", "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")

/** Entity fields PostGraphile computes from Postgres votes and comments; geo-db holds none of them. */
export const DERIVED_FIELDS = [
	"upvotes",
	"downvotes",
	"agrees",
	"disagrees",
	"commenters",
	"commentScore",
	// Ranking columns the feeder derives into Postgres; geo-db holds none of them.
	"qualityScore",
	"rankingScore",
	"trendingScore",
	"participationScore",
	"intrinsicScore",
	"lastListedOn",
	"listedOnAgendaId",
	"trendingTopicId",
	"trendingSource",
	"matchedTopicIds",
]

export const VALUE_COLUMNS = ["text", "integer", "float", "boolean", "datetime", "date", "decimal"] as const
/** Columns of a value row the card mapping does not compare; selecting them is allowed. */
export const VALUE_IGNORED = ["id", "spaceId", "language", "unit", "point", "time", "schedule", "datetimeUtc"] as const
export const RELATION_IGNORED = [
	"id",
	"spaceId",
	"entityId",
	"fromEntityId",
	"position",
	"verified",
	"toSpaceId",
	"toVersionId",
	"fromSpaceId",
	"fromVersionId",
] as const

/** A nested object in a row: which compared column its `id` carries, and what its other fields are worth. */
export type ObjectSpec = {idColumn: string | null; fields: Record<string, UncomparedReason | "name" | null>}

/** The nested objects whose `name` the shadow compares (geo-db answers them as the card's names cells). */
const NAMED_OBJECTS = ["property", "type", "toEntity", "fromEntity"] as const
type NamedObject = (typeof NAMED_OBJECTS)[number]

export const PROPERTY_OBJECT: ObjectSpec = {
	idColumn: "propertyId",
	fields: {
		id: null,
		name: "name",
		dataTypeId: "property_info",
		dataTypeName: "property_info",
		renderableTypeId: "property_info",
		renderableTypeName: "property_info",
		format: "property_info",
		isType: "property_info",
		__typename: null,
	},
}
/** What a nested entity object (`toEntity`, `fromEntity`, `entity`) may select beyond its id and name: none of it is compared. */
export const NESTED_ENTITY_FIELDS: ObjectSpec["fields"] = {
	description: "nested_entity",
	types: "nested_entity",
	valuesList: "nested_entity",
	relationsList: "nested_entity",
	values: "nested_entity",
	relations: "nested_entity",
	backlinks: "nested_entity",
	backlinksList: "nested_entity",
	spaceIds: "nested_entity",
	typeIds: "nested_entity",
}

export const RELATION_OBJECTS: Record<string, ObjectSpec> = {
	type: {idColumn: "typeId", fields: {id: null, name: "name", __typename: null}},
	toEntity: {idColumn: "toEntityId", fields: {id: null, name: "name", ...NESTED_ENTITY_FIELDS, __typename: null}},
	fromEntity: {idColumn: null, fields: {id: null, name: "name", ...NESTED_ENTITY_FIELDS, __typename: null}},
	entity: {idColumn: null, fields: {id: null, name: "nested_names", ...NESTED_ENTITY_FIELDS, __typename: null}},
}

export type ListPlan = {
	/** Response key of the list (or of the connection) in the entity object. */
	key: string
	first: number
	/** Response key of each column the comparison reads. */
	columns: Record<string, string>
	/** Response key of `{id}` for a column selected as a nested object (`property { id }`), by column. */
	nested: Record<string, string>
	/** Response key path (`object\tname`) of each compared nested `name`, by object (property, type, toEntity, fromEntity). */
	names: Partial<Record<NamedObject, string>>
	/** For a connection: the response key of its `nodes`, and of `pageInfo` and its `hasNextPage`. */
	nodesKey: string | null
	hasNext: {pageInfo: string; field: string} | null
	/** The nested entity objects of the rows (`GEODB_NESTED_ENTITY_SHADOW`); absent when the rows select none. */
	nestedEntities?: NestedEntityPlans
	/** Where the property_info fields of the rows' `property` object sit (`GEODB_PROPERTY_INFO_SHADOW`); absent when none is selected. */
	propertyInfo?: PropertyInfoPlan
}

/** What a card comparison needs of an entity selection: where its answers sit in an entity object. */
export type EntityCardPlan = {
	/** Response key of `id`, when selected. */
	idKey: string | null
	/** The space the lists are filtered to, or null for every space (the entity's own spaces are read). */
	spaceId: string | null
	/** Response keys of the derived scalar fields, when selected. */
	name: string | null
	description: string | null
	spaceIds: string | null
	/** Response keys of `updatedAt` and `createdAt` (unix seconds as text), when selected plain. Their `*Block` twins stay uncompared. */
	updatedAt: string | null
	createdAt: string | null
	types: {key: string; idKey: string | null; nameKey: string | null; plain?: boolean} | null
	/** The unfiltered lists (every space): their response keys, and of the columns read. */
	allValues: {key: string; first: number; spaceKey: string | null; propertyKey: string | null} | null
	allRelations: {key: string; first: number; spaceKey: string | null} | null
	values: ListPlan
	relations: ListPlan
	/** Light selections only: the `backlinks` / `backlinksList` fields whose rows geo-db settles (`backlink_rows` otherwise). */
	backlinkRows?: BacklinkRowsPlan[]
}

export type EntityPlan = EntityCardPlan & {
	/** Response key of the entity in `data`. */
	key: string
	entityId: string
	/** Parts of the selection no card can settle, known from the query alone. */
	uncompared: UncomparedReason[]
}

export type Analysis = {ok: true; plan: EntityPlan} | {ok: false; reason: UnsupportedReason}

export class Unsupported extends Error {
	constructor(readonly reason: UnsupportedReason) {
		super(reason)
	}
}

const refuse = (reason: UnsupportedReason): never => {
	throw new Unsupported(reason)
}

/** The fields of a selection set with fragments inlined, in order. Directives anywhere are unsupported. */
export function fieldsOf(
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

export const responseKeyOf = (field: FieldNode) => field.alias?.value ?? field.name.value

/** A field's arguments, resolved against the request's variables. */
export function argsOf(field: FieldNode, variables: Record<string, unknown>): Record<string, unknown> {
	const args: Record<string, unknown> = {}
	for (const arg of field.arguments ?? []) args[arg.name.value] = valueFromASTUntyped(arg.value, variables)
	return args
}

export function uuidArg(value: unknown): string {
	return typeof value === "string" && isValidUuid(value) ? normalizeUuid(value) : refuse("args")
}

/**
 * True when a filter operand filters nothing: absent, null, or an object whose every field is such an operand.
 * PostGraphile drops null and empty-object filter input (`connectionFilterAllowNullInput`,
 * `connectionFilterAllowEmptyObjectInput`), so `{spaceIds: {anyEqualTo: null}}` is no filter at all.
 */
export function isEmptyFilter(value: unknown): boolean {
	if (value === undefined || value === null) return true
	if (typeof value !== "object" || Array.isArray(value)) return false
	return Object.values(value as Record<string, unknown>).every(isEmptyFilter)
}

/** A field with no arguments, or only a `filter` that filters nothing (`isEmptyFilter`). */
export function isUnfilteredField(field: FieldNode, resolved: Record<string, unknown>): boolean {
	const args = argsOf(field, resolved)
	return Object.entries(args).every(([name, value]) => name === "filter" && isEmptyFilter(value))
}

/**
 * `{spaceId: {is: <uuid>}}` and nothing else. A null `is` (or no filter) is every space: PostGraphile's
 * connection filter allows null input, so the list is unfiltered.
 */
export function spaceFromFilter(filter: unknown): string | null {
	if (filter === undefined || filter === null) return null
	if (typeof filter !== "object" || Array.isArray(filter)) return refuse("args")
	const keys = Object.keys(filter)
	if (keys.length !== 1 || keys[0] !== "spaceId") return refuse("args")
	const clause = (filter as {spaceId: unknown}).spaceId
	if (clause === null || typeof clause !== "object" || Array.isArray(clause)) return refuse("args")
	const clauseKeys = Object.keys(clause)
	if (clauseKeys.length !== 1 || clauseKeys[0] !== "is") return refuse("args")
	const is = (clause as {is: unknown}).is
	return is === undefined || is === null ? null : uuidArg(is)
}

export const isPlainField = (field: FieldNode) => !field.arguments?.length && !field.selectionSet

const LIST_FIELDS = ["values", "valuesList", "relations", "relationsList"]
const NO_LIST: ListPlan = {key: "", first: 0, columns: {}, nested: {}, names: {}, nodesKey: null, hasNext: null}

/** A connection field whose selection is `totalCount` alone (and `__typename`). */
export const isCountOnly = (field: FieldNode): boolean =>
	(field.name.value === "relations" || field.name.value === "values" || field.name.value === "backlinks") &&
	field.selectionSet !== undefined &&
	field.selectionSet.selections.length > 0 &&
	field.selectionSet.selections.every(
		(sel) =>
			sel.kind === Kind.FIELD && !sel.arguments?.length && ["totalCount", "__typename"].includes(sel.name.value),
	)

/** The columns (and nested `{id}` objects) of a list's rows, with fragments inlined. */
export type PlannedColumns = {
	columns: Record<string, string>
	nested: Record<string, string>
	names: ListPlan["names"]
	propertyInfo?: PropertyInfoPlan
}

const NESTED_OBJECT_NAMES = ["toEntity", "fromEntity", "entity"]

export function planColumns(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	spec: {columns: readonly string[]; objects: Record<string, ObjectSpec>; ignored: readonly string[]},
	uncompared: Set<UncomparedReason>,
	nestedOut?: NestedCollector,
): PlannedColumns {
	const columns: Record<string, string> = {}
	const nested: Record<string, string> = {}
	const names: PlannedColumns["names"] = {}
	let propertyInfo: PropertyInfoPlan | undefined
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
			if (object === PROPERTY_OBJECT) propertyInfo = planPropertyInfo(child, fragments, propertyInfo)
			if (nestedOut && NESTED_OBJECT_NAMES.includes(name)) {
				const planned = planNestedEntity(name as NestedObjectName, child, fragments, nestedOut.variables)
				nestedOut.plans.push(planned.plan)
				if (planned.residual) nestedOut.residual = true
			}
			for (const inner of fieldsOf(child.selectionSet, fragments)) {
				if (!(inner.name.value in object.fields)) refuse("selection")
				const reason = object.fields[inner.name.value]
				if (reason === "name") {
					// Compared where geo-db answers names (the card read asks for them); a plain field only.
					if (!isPlainField(inner)) refuse("selection")
					if ((NAMED_OBJECTS as readonly string[]).includes(name))
						names[name as NamedObject] ??= `${key}\t${responseKeyOf(inner)}`
					else uncompared.add("nested_names")
				} else if (reason) uncompared.add(reason)
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
	return {columns, nested, names, ...(propertyInfo ? {propertyInfo} : {})}
}

type PlannedList = {list: ListPlan; spaceId: string | null}

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
	const collector: NestedCollector = {plans: [], residual: false, variables}
	const withNested = (list: ListPlan): ListPlan =>
		collector.plans.length > 0
			? {...list, nestedEntities: {plans: collector.plans, residual: collector.residual}}
			: list
	if (!connection) {
		const {columns, nested, names, propertyInfo} = planColumns(
			field.selectionSet as SelectionSetNode,
			fragments,
			spec,
			uncompared,
			collector,
		)
		return {
			list: withNested({
				key: listKey,
				first,
				columns,
				nested,
				names,
				nodesKey: null,
				hasNext: null,
				...(propertyInfo ? {propertyInfo} : {}),
			}),
			spaceId,
		}
	}
	let nodesKey: string | null = null
	let hasNext: ListPlan["hasNext"] = null
	let rows: PlannedColumns | null = null
	for (const child of fieldsOf(field.selectionSet as SelectionSetNode, fragments)) {
		const name = child.name.value
		if (name === "__typename") continue
		if (child.arguments?.length || !child.selectionSet) throw new Unsupported("selection")
		if (name === "nodes" && !nodesKey) {
			nodesKey = responseKeyOf(child)
			rows = planColumns(child.selectionSet, fragments, spec, uncompared, collector)
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
	return {
		list: withNested({
			key: listKey,
			first,
			columns: rows.columns,
			nested: rows.nested,
			names: rows.names,
			nodesKey,
			hasNext,
			...(rows.propertyInfo ? {propertyInfo: rows.propertyInfo} : {}),
		}),
		spaceId,
	}
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

/** The operation's selection with fragments and variables resolved. Throws `Unsupported` for anything else. */
export type ParsedOperation = {
	fragments: Map<string, SelectionSetNode>
	/** The request's variables, plus the declared defaults of those it left out. */
	variables: Record<string, unknown>
	/** Root fields, `__typename` aside, with fragments inlined. */
	roots: FieldNode[]
}

export function parseOperation(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
): ParsedOperation {
	const fragments = new Map<string, SelectionSetNode>()
	const operations: OperationDefinitionNode[] = []
	for (const definition of document.definitions) {
		if (definition.kind === Kind.FRAGMENT_DEFINITION) fragments.set(definition.name.value, definition.selectionSet)
		else if (definition.kind === Kind.OPERATION_DEFINITION) operations.push(definition)
	}
	const operation = operationName
		? operations.find((op) => op.name?.value === operationName)
		: operations.length === 1
			? operations[0]
			: undefined
	if (!operation || operation.operation !== "query") return refuse("not_single_root")
	if (operation.directives?.length) return refuse("directives")

	// A variable the request left out takes its declared default.
	const resolved = {...variables}
	for (const definition of operation.variableDefinitions ?? []) {
		const name = definition.variable.name.value
		if (resolved[name] === undefined && definition.defaultValue) {
			resolved[name] = valueFromASTUntyped(definition.defaultValue)
		}
	}
	const roots = fieldsOf(operation.selectionSet, fragments).filter((f) => f.name.value !== "__typename")
	return {fragments, variables: resolved, roots}
}

/**
 * `backlinks(filter) { totalCount nodes { .. } }` or `backlinksList(first, filter) { .. }`: the relations that point at
 * the entity. The selection is read the way a relations list's is, so an unknown column or argument is still refused.
 * Rows that name their referrer (`fromEntityId` or `fromEntity { id }`) are planned for an exact comparison with
 * geo-db's backlinks (`BacklinkRowsPlan`: Space, relation type and referrer); any other is counted `backlink_rows`.
 */
function planBacklinkRows(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	resolved: Record<string, unknown>,
	uncompared: Set<UncomparedReason>,
): BacklinkRowsPlan | null {
	const args = argsOf(field, resolved)
	for (const arg of Object.keys(args)) if (!["first", "filter", "offset"].includes(arg)) refuse("args")
	if (args.filter !== undefined && args.filter !== null && !asRecord(args.filter)) refuse("filter")
	const spec = {
		columns: [] as string[],
		objects: RELATION_OBJECTS,
		ignored: [...RELATION_IGNORED, "typeId", "toEntityId"],
	}
	const rowsOf = (set: SelectionSetNode) => {
		planColumns(set, fragments, spec, uncompared)
		return set
	}
	let rowSet: SelectionSetNode
	let nodesKey: string | null = null
	if (field.name.value === "backlinksList") rowSet = rowsOf(field.selectionSet as SelectionSetNode)
	else {
		let nodes: SelectionSetNode | null = null
		for (const child of fieldsOf(field.selectionSet as SelectionSetNode, fragments)) {
			const name = child.name.value
			if (name === "__typename" || (name === "totalCount" && isPlainField(child))) continue
			if (name === "pageInfo" && child.selectionSet && !child.arguments?.length) continue
			if (name === "nodes" && child.selectionSet && !child.arguments?.length && !nodes) {
				nodes = rowsOf(child.selectionSet)
				nodesKey = responseKeyOf(child)
			} else refuse("selection")
		}
		// `backlinks { totalCount }` alone has no rows to compare (its count is the caller's).
		if (!nodes) {
			uncompared.add("backlink_rows")
			return null
		}
		rowSet = nodes
	}
	// Where the row's Space, relation type and referrer sit.
	let space: string[] | null = null
	let type: string[] | null = null
	let from: string[] | null = null
	for (const column of fieldsOf(rowSet, fragments)) {
		const name = column.name.value
		const key = responseKeyOf(column)
		if (name === "spaceId" && isPlainField(column)) space ??= [key]
		else if (name === "typeId" && isPlainField(column)) type ??= [key]
		else if (name === "fromEntityId" && isPlainField(column)) from ??= [key]
		else if ((name === "type" || name === "fromEntity") && column.selectionSet && !column.arguments?.length) {
			const id = fieldsOf(column.selectionSet, fragments).find((f) => f.name.value === "id" && isPlainField(f))
			if (!id) continue
			if (name === "type") type ??= [key, responseKeyOf(id)]
			else from ??= [key, responseKeyOf(id)]
		}
	}
	if (!from) {
		uncompared.add("backlink_rows")
		return null
	}
	const filter = readBacklinkFilter(args.filter)
	const first = typeof args.first === "number" && Number.isInteger(args.first) && args.first > 0 ? args.first : 0
	const offset = typeof args.offset === "number" && Number.isInteger(args.offset) && args.offset > 0 ? args.offset : 0
	return {key: responseKeyOf(field), nodesKey, space, type, from, first, offset, filter}
}

/**
 * Plans the selection of one entity: its derived fields and the two space-filtered lists a card can settle.
 * Throws `Unsupported` when the selection is anything else.
 */
export function planEntitySelection(
	selectionSet: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	resolved: Record<string, unknown>,
	uncompared: Set<UncomparedReason>,
	/** Light mode (lightShadow.ts): every list field goes to this callback instead of the card lists. */
	light?: (field: FieldNode) => void,
	options: {derivedFields?: boolean} = {},
): EntityCardPlan {
	let values: PlannedList | null = null
	let relations: PlannedList | null = null
	let idKey: string | null = null
	let name: string | null = null
	let description: string | null = null
	let spaceIds: string | null = null
	let updatedAt: string | null = null
	let createdAt: string | null = null
	let types: EntityPlan["types"] = null
	let allValues: EntityPlan["allValues"] = null
	let allRelations: EntityPlan["allRelations"] = null
	let backlinkRows = false
	const backlinkPlans: BacklinkRowsPlan[] = []
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
	for (const field of fieldsOf(selectionSet, fragments)) {
		const key = responseKeyOf(field)
		if (seen.has(key)) return refuse("selection")
		seen.add(key)
		const fieldName = field.name.value
		const filtered = argsOf(field, resolved).filter !== undefined
		if (light && LIST_FIELDS.includes(fieldName) && !isCountOnly(field)) {
			light(field)
		} else if (fieldName === "id" || fieldName === "__typename") {
			if (field.arguments?.length) return refuse("args")
			if (field.selectionSet) return refuse("selection")
			if (fieldName === "id") idKey ??= key
		} else if (fieldName === "name" && !name && isPlainField(field)) {
			name = key
		} else if (fieldName === "description" && !description && isPlainField(field)) {
			description = key
		} else if (fieldName === "spaceIds" && !spaceIds && isPlainField(field)) {
			spaceIds = key
		} else if (fieldName === "updatedAt" && !updatedAt && isPlainField(field)) {
			updatedAt = key
		} else if (fieldName === "createdAt" && !createdAt && isPlainField(field)) {
			createdAt = key
		} else if ((fieldName === "updatedAt" || fieldName === "updatedAtBlock") && isPlainField(field)) {
			// The block number is not in geo-db (the feeder writes block time only), nor is a field selected twice.
			uncompared.add("updated_at")
		} else if ((fieldName === "createdAt" || fieldName === "createdAtBlock") && isPlainField(field)) {
			uncompared.add("created_at")
		} else if (light && fieldName === "spacesByTopicIdConnection" && field.selectionSet) {
			// The Spaces that name this entity as their topic: a Space's page, members and settings are no part of an
			// entity card, so the field is counted uncompared and the rest of the entity is still judged.
			uncompared.add("topic_spaces")
		} else if (isCountOnly(field)) {
			// `relations { totalCount }`: a count of rows geo-db cannot be asked for without the list. Likewise
			// `backlinks { totalCount }` (the tiebreaker queries): the count is not compared, the rest of the card is.
			uncompared.add(fieldName === "backlinks" ? "backlink_count" : "relation_count")
		} else if (
			fieldName === "types" &&
			(!types || types.plain) &&
			isUnfilteredField(field, resolved) &&
			field.selectionSet
		) {
			// `typeIds` selected first: `types { id name }` is the same statements with names, so it takes over.
			if (types?.plain) uncompared.add("types_alias")
			let typeIdKey: string | null = null
			let nameKey: string | null = null
			for (const inner of fieldsOf(field.selectionSet, fragments)) {
				if (!isPlainField(inner)) return refuse("selection")
				if (inner.name.value === "id") typeIdKey ??= responseKeyOf(inner)
				else if (inner.name.value === "name") nameKey ??= responseKeyOf(inner)
				else if (inner.name.value !== "__typename") return refuse("selection")
			}
			// `types { name }` with no id (the light selection only): the names are judged against geo-db's Types rows' names.
			if (!typeIdKey && !(light && nameKey)) return refuse("selection")
			types = {key, idKey: typeIdKey, nameKey}
		} else if (light && options.derivedFields && DERIVED_FIELDS.includes(fieldName) && isPlainField(field)) {
			uncompared.add("derived_fields")
		} else if (light && fieldName === "typeIds" && isPlainField(field)) {
			// The light selection only: `typeIds` is the same Types statements as `types { id }`, as a list of ids.
			// Beside `types { id name }` (either order) it is the same answer twice: the `types` list is compared.
			if (types) uncompared.add("types_alias")
			else types = {key, idKey: null, nameKey: null, plain: true}
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
		} else if (light && (fieldName === "backlinks" || fieldName === "backlinksList") && field.selectionSet) {
			const planned = planBacklinkRows(field, fragments, resolved, uncompared)
			if (planned) backlinkPlans.push(planned)
			backlinkRows = true
		} else return refuse("selection")
	}
	if (light) {
		// `entity(id) { backlinksList { .. } }` and nothing of the entity itself is the backlinks shadow's (it compares the rows).
		if (backlinkRows && !idKey && !name && !description && !spaceIds && !types) return refuse("selection")
		return {
			idKey,
			spaceId: null,
			name,
			description,
			spaceIds,
			updatedAt,
			createdAt,
			types,
			allValues: null,
			allRelations: null,
			values: NO_LIST,
			relations: NO_LIST,
			backlinkRows: backlinkPlans,
		}
	}
	// A card does not say which rows are values and which relations, so it can settle both lists or neither.
	if (!values || !relations) return refuse(values || relations ? "selection" : "no_space")
	if (values.spaceId !== relations.spaceId) return refuse("args")
	// Every space: the spaces to read come from the entity's own `spaceIds` (or its unfiltered lists).
	if (values.spaceId === null && !spaceIds && !(allValues && allRelations)) return refuse("no_space")
	for (const column of ["propertyId", ...VALUE_COLUMNS]) {
		if (!values.list.columns[column] && !values.list.nested[column]) return refuse("selection")
	}
	for (const column of ["typeId", "toEntityId"]) {
		if (!relations.list.columns[column] && !relations.list.nested[column]) return refuse("selection")
	}
	return {
		idKey,
		spaceId: values.spaceId,
		name,
		description,
		spaceIds,
		updatedAt,
		createdAt,
		types,
		allValues,
		allRelations,
		values: values.list,
		relations: relations.list,
	}
}

/** Decides whether an `entity(id)` operation is one a card can settle, and where its answers sit in the response. */
export function analyzeEntityOperation(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
): Analysis {
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
		const rootArgs = argsOf(root, resolved)
		if (Object.keys(rootArgs).length !== 1 || !("id" in rootArgs)) return {ok: false, reason: "args"}
		const entityId = uuidArg(rootArgs.id)
		if (!root.selectionSet) return {ok: false, reason: "selection"}
		const card = planEntitySelection(root.selectionSet, fragments, resolved, uncompared)
		return {ok: true, plan: {...card, key: responseKeyOf(root), entityId, uncompared: [...uncompared]}}
	} catch (error) {
		if (error instanceof Unsupported) return {ok: false, reason: error.reason}
		return {ok: false, reason: "selection"}
	}
}

export const asRecord = (v: unknown): Record<string, unknown> | null =>
	v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null

export function read(node: Record<string, unknown>, list: ListPlan, column: string): unknown {
	const direct = list.columns[column]
	if (direct !== undefined) return node[direct]
	const [object, id] = (list.nested[column] ?? "").split("\t")
	return object && id ? asRecord(node[object])?.[id] : undefined
}

/** A nested `name` out of a row: its string, else null (an entity PostGraphile found no Name for). */
export function readName(node: Record<string, unknown>, path: string | undefined): string | null | undefined {
	if (path === undefined) return undefined
	const [object, name] = path.split("\t")
	const v = object && name ? asRecord(node[object])?.[name] : undefined
	return typeof v === "string" ? v : null
}

export const orNull = <T>(v: unknown): T | null => (v === undefined || v === null ? null : (v as T))

/** An entity a row names, as PostGraphile spelled its id, and the name PostGraphile gave it. */
export type NamedRef = {id: string; name: string | null}

export type PostgresCard = {
	values: ValueRow[]
	relations: RelationRow[]
	/** One of the compared lists may have been cut short (a full page, or `hasNextPage`). */
	truncated: boolean
	/** Derived fields, `undefined` when not selected. */
	name?: string | null
	description?: string | null
	spaceIds?: string[]
	/** unix seconds as PostGraphile spelled them: null for an entity it has none of, absent when not selected. */
	updatedAt?: number | null
	createdAt?: number | null
	types?: string[]
	/** The names of `types`, in order, when `types { name }` was selected. */
	typeNames?: (string | null)[]
	/** The nested names the rows carried, by object, for the objects whose `name` was selected. */
	nested?: {property?: NamedRef[]; type?: NamedRef[]; toEntity?: NamedRef[]; fromEntity?: (string | null)[]}
	/** The unfiltered lists' rows, `null` when the list is absent, `truncated` when it filled its page. */
	allValues?: {rows: {spaceId: string | null; propertyId: string | null}[]; truncated: boolean}
	allRelations?: {rows: {spaceId: string | null}[]; truncated: boolean}
	/** The nested entity objects of the relation rows, when the plan selected any (`GEODB_NESTED_ENTITY_SHADOW`). */
	nestedEntities?: NestedEntities
	/** The property_info objects of the values rows, when the plan selected any (`GEODB_PROPERTY_INFO_SHADOW`). */
	propertyInfo?: PropertyInfos
} | null

/** PostGraphile's `updatedAt` / `createdAt`: unix seconds as text (an ISO instant is read too), else null. */
/** How far geo-db's time may cross Postgres's on the wrong side before it is a disagreement (an hour). */
export const TIME_LAG_SECONDS = 3600

export function secondsOrNull(v: unknown): number | null {
	if (typeof v === "number") return Number.isFinite(v) ? Math.floor(v) : null
	if (typeof v !== "string") return null
	if (/^\d+$/.test(v)) return Number(v)
	const ms = Date.parse(v)
	return Number.isNaN(ms) ? null : Math.floor(ms / 1000)
}

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
	return cardFromEntity(data[plan.key], plan)
}

/**
 * The card out of one entity object of PostGraphile's answer (null: an entity PostGraphile does not have),
 * wherever it sits in the response. Null when the object is not a plain answer for this plan.
 */
export function cardFromEntity(entity: unknown, plan: EntityCardPlan): PostgresCard {
	// An entity PostGraphile does not have holds no statements and no derived data.
	if (entity === null) {
		return {
			values: [],
			relations: [],
			truncated: false,
			...(plan.name ? {name: null} : {}),
			...(plan.description ? {description: null} : {}),
			...(plan.spaceIds ? {spaceIds: []} : {}),
			...(plan.updatedAt ? {updatedAt: null} : {}),
			...(plan.createdAt ? {createdAt: null} : {}),
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
	const nested: NonNullable<NonNullable<PostgresCard>["nested"]> = {}
	if (plan.values.names.property) nested.property = []
	if (plan.relations.names.type) nested.type = []
	if (plan.relations.names.toEntity) nested.toEntity = []
	if (plan.relations.names.fromEntity) nested.fromEntity = []
	const values: ValueRow[] = []
	const propertyInfo: PropertyInfos = {items: [], incomplete: false}
	for (const raw of valueList.nodes) {
		const node = asRecord(raw)
		if (!node) return null
		const propertyId = read(node, plan.values, "propertyId")
		if (typeof propertyId !== "string") return null
		if (plan.values.propertyInfo) collectPropertyInfo(node, propertyId, plan.values.propertyInfo, propertyInfo)
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
		nested.property?.push({id: propertyId, name: readName(node, plan.values.names.property) ?? null})
	}
	const relations: RelationRow[] = []
	const nestedEntities: NestedEntities = {items: [], incomplete: false}
	for (const raw of relationList.nodes) {
		const node = asRecord(raw)
		if (!node) return null
		const typeId = read(node, plan.relations, "typeId")
		const toEntityId = read(node, plan.relations, "toEntityId")
		if (typeof typeId !== "string" || typeof toEntityId !== "string") return null
		if (plan.relations.nestedEntities)
			collectNestedEntities(node, toEntityId, plan.relations.nestedEntities, nestedEntities)
		relations.push({type_id: typeId, to_entity_id: toEntityId, to_space_id: null})
		nested.type?.push({id: typeId, name: readName(node, plan.relations.names.type) ?? null})
		nested.toEntity?.push({id: toEntityId, name: readName(node, plan.relations.names.toEntity) ?? null})
		nested.fromEntity?.push(readName(node, plan.relations.names.fromEntity) ?? null)
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
	if (plan.values.propertyInfo) card.propertyInfo = propertyInfo
	if (plan.name) card.name = stringOrNull(plan.name)
	if (plan.description) card.description = stringOrNull(plan.description)
	if (plan.updatedAt) card.updatedAt = secondsOrNull(entityObject[plan.updatedAt])
	if (plan.createdAt) card.createdAt = secondsOrNull(entityObject[plan.createdAt])
	if (plan.spaceIds) {
		const ids = entityObject[plan.spaceIds]
		if (!Array.isArray(ids) || !ids.every((id) => typeof id === "string")) return null
		card.spaceIds = (ids as string[]).map((id) => normalizeUuid(id))
	}
	if (plan.types) {
		const nodes = entityObject[plan.types.key]
		if (!Array.isArray(nodes)) return null
		const ids: string[] = []
		const typeNames: (string | null)[] = []
		for (const node of nodes) {
			const id = plan.types.idKey ? asRecord(node)?.[plan.types.idKey] : undefined
			if (typeof id !== "string") return null
			ids.push(id)
			const typeName = plan.types.nameKey ? asRecord(node)?.[plan.types.nameKey] : undefined
			typeNames.push(typeof typeName === "string" ? typeName : null)
		}
		card.types = ids
		if (plan.types.nameKey) card.typeNames = typeNames
	}
	if (Object.keys(nested).length > 0) card.nested = nested
	if (plan.relations.nestedEntities) card.nestedEntities = nestedEntities
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
	checks: Partial<
		Record<
			| "name"
			| "description"
			| "types"
			| "spaceIds"
			| "updatedAt"
			| "createdAt"
			| "allValuesList"
			| "propertyNames"
			| "typeNames"
			| "toEntityNames"
			| "typesNames"
			| "fromEntityNames",
			CheckResult
		>
	>
	/** Reasons found while comparing (beyond the ones the query alone implies). */
	uncompared: UncomparedReason[]
}

type GeoRow = GeoCardRow

/** The spaces a comparison read: what geo-db was asked for, and which of them answered with any row. */
export type ReadScope = {spaces: string[]; populated: string[]}

const inSpaces = (a: string | null | undefined, spaces: Set<string>) =>
	a !== null && a !== undefined && spaces.has(normalizeUuid(a))

/**
 * Checks the derived fields against the card's rows (see the file comment for what each check means).
 * `card.truncated` must be false.
 */
export function compareDerived(
	plan: Pick<EntityCardPlan, "spaceId" | "allValues">,
	card: NonNullable<PostgresCard>,
	geo: GeoRow[],
	scope?: ReadScope,
	names?: Map<string, string[]>,
): DerivedChecks {
	const checks: DerivedChecks["checks"] = {}
	const uncompared = new Set<UncomparedReason>()
	// One space (the plan's), or the several the entity lives in (every space): `geo` is the union of their cards.
	const spaces = new Set(scope?.spaces ?? (plan.spaceId ? [plan.spaceId] : []))
	const populated = scope?.populated ?? (geo.length > 0 ? [...spaces] : [])

	// The spaces PostGraphile attributes the entity to, when the answer says: `spaceIds`, else every row's space.
	let known: Set<string> | null = null
	if (card.spaceIds) known = new Set(card.spaceIds)
	else if (card.allValues && card.allRelations && !card.allValues.truncated && !card.allRelations.truncated) {
		const spaces = [...card.allValues.rows, ...card.allRelations.rows].map((r) => r.spaceId)
		if (spaces.every((s) => s !== null)) known = new Set((spaces as string[]).map((s) => normalizeUuid(s)))
	}
	const onlyThisSpace = known !== null && [...known].every((s) => spaces.has(s))

	/**
	 * One name or description PostGraphile gave, against the values geo-db holds for the same thing. PostGraphile
	 * may have taken it from any space, so a value geo-db lacks is a mismatch only where the thing lives in this
	 * space alone (`single`); a name PostGraphile has none for is a mismatch whenever geo-db holds one.
	 */
	const judge = (value: string | null, geoValues: string[], single: boolean): CheckResult => {
		if (value === null) return geoValues.length === 0 ? "ok" : "mismatch"
		if (geoValues.includes(value)) return "ok"
		if (single) return "mismatch"
		// Not this space's value, and other spaces could have supplied it: not decidable from the card.
		uncompared.add("cross_space")
		return "inconclusive"
	}
	const textField = (field: "name" | "description", property: string) => {
		const value = card[field]
		if (value === undefined) return
		checks[field] = judge(
			value,
			geo.filter((row) => row.prop === property).map((row) => row.raw),
			onlyThisSpace,
		)
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

	// Nested names. geo-db prints, with each row, the property's name and the target's name (null for `none`),
	// resolved in the request's Space; PostGraphile gave each entity a name from any space, so these are judged
	// like `name` above except that a nested entity's spaces are not known (`single` is false): a name
	// PostGraphile lacks that geo-db has is a mismatch, a different or missing one is inconclusive.
	const nestedAsked = card.nested !== undefined || card.typeNames !== undefined
	if (nestedAsked && geo.some((row) => row.propName === undefined)) {
		// The reader was not asked for names (or an older geo-db did not send them): nothing to compare.
		uncompared.add("nested_names")
	} else if (nestedAsked) {
		const propNames = new Map<string, string | null>()
		const targetNames = new Map<string, string | null>()
		const typeTargetNames = new Map<string, string | null>()
		for (const row of geo) {
			// Several spaces may each name the same thing: any name beats none.
			if (!propNames.get(row.prop)) propNames.set(row.prop, row.propName ?? null)
			if (!targetNames.get(row.raw)) targetNames.set(row.raw, row.targetName ?? null)
			if (row.prop === TYPES_PROPERTY && !typeTargetNames.get(row.raw))
				typeTargetNames.set(row.raw, row.targetName ?? null)
		}
		const against = (refs: NamedRef[], known: Map<string, string | null>): CheckResult[] =>
			refs.flatMap((ref) => {
				const id = conformingId("entity", ref.id)
				// An id geo-db has no row for is a statement difference, which the card diff already reports.
				if (!known.has(id)) return []
				// geo-db names a thing only from the Spaces read; `names` holds its Name in every Space (`readNames`).
				if (ref.name !== null && names?.get(id)?.includes(ref.name)) return ["ok"]
				const name = known.get(id) ?? null
				return [judge(ref.name, name === null ? [] : [name], false)]
			})
		const combine = (results: CheckResult[]): CheckResult | undefined =>
			results.length === 0
				? undefined
				: results.includes("mismatch")
					? "mismatch"
					: results.includes("inconclusive")
						? "inconclusive"
						: "ok"
		const set = (key: keyof DerivedChecks["checks"], result: CheckResult | undefined) => {
			if (result !== undefined) checks[key] = result
		}
		if (card.nested?.property) set("propertyNames", combine(against(card.nested.property, propNames)))
		if (card.nested?.type) set("typeNames", combine(against(card.nested.type, propNames)))
		if (card.nested?.toEntity) set("toEntityNames", combine(against(card.nested.toEntity, targetNames)))
		if (card.types === undefined && card.typeNames !== undefined) {
			// Names only: a name geo-db has for a Type is one PostGraphile names too, in any Space; geo-db holding a Type
			// where PostGraphile lists none is wrong.
			const geoNames = [...typeTargetNames.values()].filter((name): name is string => name !== null)
			const held = new Set(card.typeNames.filter((name): name is string => name !== null))
			if (typeTargetNames.size > 0 && card.typeNames.length === 0) checks.typesNames = "mismatch"
			else if (geoNames.every((name) => held.has(name))) checks.typesNames = "ok"
			else {
				checks.typesNames = "inconclusive"
				uncompared.add("cross_space")
			}
		}
		if (card.types && card.typeNames) {
			const refs = card.types.map((id, at) => ({id, name: card.typeNames?.[at] ?? null}))
			set("typesNames", combine(against(refs, typeTargetNames)))
		}
		if (card.nested?.fromEntity) {
			const own = geo.filter((row) => row.prop === NAME_PROPERTY).map((row) => row.raw)
			set("fromEntityNames", combine(card.nested.fromEntity.map((name) => judge(name, own, onlyThisSpace))))
		}
	}

	// createdAt and updatedAt. geo-db's times are a subset of Postgres's (section 5.6: the feeder stamps an entity
	// only where one of its own statements changed, Postgres also where an op merely names it), so geo-db's
	// updatedAt should be at or below Postgres's and its createdAt at or above. Equal is agreement. A time geo-db
	// does not hold, or one on the expected side, is not decidable. A bound crossed by more than TIME_LAG_SECONDS
	// is a disagreement; a smaller crossing is not, because the feeder can run ahead of the kg-indexer, the
	// indexer's own writes can land out of order, and Postgres stops advancing updated_at on a soft-deleted entity
	// (its upsert has WHERE deleted_at IS NULL) while geo-db goes on stamping it.
	// The bounds hold for a subset of the entity's Spaces too: a least over fewer Spaces is no smaller.
	const times = entityTimesOf(geo)
	if (card.updatedAt != null) {
		if (times.updatedAt === null || times.updatedAt < card.updatedAt) {
			checks.updatedAt = "inconclusive"
			uncompared.add("updated_at")
		} else if (times.updatedAt === card.updatedAt) checks.updatedAt = "ok"
		else if (times.updatedAt - card.updatedAt <= TIME_LAG_SECONDS) {
			checks.updatedAt = "inconclusive"
			uncompared.add("updated_at")
		} else checks.updatedAt = "mismatch"
	}
	if (card.createdAt != null) {
		if (times.createdAt === null || times.createdAt > card.createdAt) {
			checks.createdAt = "inconclusive"
			uncompared.add("created_at")
		} else if (times.createdAt === card.createdAt) checks.createdAt = "ok"
		else if (card.createdAt - times.createdAt <= TIME_LAG_SECONDS) {
			checks.createdAt = "inconclusive"
			uncompared.add("created_at")
		} else checks.createdAt = "mismatch"
	}

	if (card.spaceIds !== undefined) {
		// The space has statements in geo-db, so PostGraphile must list it. Other spaces are not checkable.
		const listed = card.spaceIds
		checks.spaceIds = populated.some((s) => !listed.includes(s)) ? "mismatch" : "ok"
		if (listed.some((s) => !spaces.has(s))) uncompared.add("cross_space")
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
				if (!inSpaces(row.spaceId, spaces)) continue
				if (row.propertyId === null) continue
				const prop = conformingId("entity", row.propertyId)
				if (DERIVED_PROPERTIES.has(prop)) continue
				const at = pool.indexOf(prop)
				if (at < 0) missing = true
				else pool.splice(at, 1)
			}
			checks.allValuesList = missing ? "mismatch" : "ok"
			if (rows.some((row) => !inSpaces(row.spaceId, spaces))) uncompared.add("cross_space")
		}
	}
	if (card.allRelations?.truncated) uncompared.add("truncated")
	if (
		card.allRelations &&
		!card.allRelations.truncated &&
		card.allRelations.rows.some((r) => !inSpaces(r.spaceId, spaces))
	)
		uncompared.add("cross_space")
	return {checks, uncompared: [...uncompared]}
}

export type EntityShadowDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {
	log: CardLogger
	/** Caps the geo-db reads of the wide (several-space) comparisons; absent means no cap. */
	budget?: ReadBudget
	/**
	 * Read the cards of a wide comparison in one pipe line across all its Spaces (one read, one budget unit)
	 * instead of one read a Space. Needs a geodb-client from geo-db 353ab26313 or later (`GEODB_MULTI_SPACE_READS`).
	 */
	multiSpace?: boolean
	/**
	 * Every geo-db Space id the space map holds. With it, an `entity` answer that does not say which Spaces the
	 * entity lives in (`EntityExists`, `EntityTypes`) is read across all of them in one pipe line
	 * (`readEverySpace`). Absent (GEODB_EVERY_SPACE_READS off) leaves those operations `no_space`.
	 */
	allGeoSpaces?: () => Promise<string[]>
	/** Caps the every-Space reads (one unit each); absent means no cap. */
	everySpaceBudget?: ReadBudget
	/**
	 * Compare `backlinksList` (`EntityBacklinksPage`) against geo-db's card backlinks (`GEODB_BACKLINK_READS`):
	 * a filtered list reads its Space under `budget`, an unfiltered one every Space under `everySpaceBudget`.
	 */
	backlinks?: boolean
	/** Accept the EntitiesBatch selection's membership-only `allRelationsList { spaceId }` in the light `entities` shadow (`GEODB_ENTITIES_BATCH_SHADOW`). */
	entitiesBatch?: boolean
	/** Accept the Postgres-computed vote scores (`upvotes` ..) in the light `entities` shadow, uncompared (`GEODB_DERIVED_FIELDS_SHADOW`). */
	derivedFields?: boolean
	/** Split an every-Space read over more than MAX_EVERY_SPACES loaded Spaces into several lines (`GEODB_EVERY_SPACE_CHUNKS`). */
	everySpaceChunks?: boolean
	/** Timeout in ms of an every-Space read, in place of the pool's (`GEODB_EVERY_SPACE_TIMEOUT_MS`). */
	everySpaceTimeoutMs?: number
	/**
	 * Settle an every-Space read by asking geo-db which Spaces hold the entity (`.args.holders`) and reading the card in
	 * those alone (`GEODB_HOLDERS_LOOKUP`). Absent: the card is read across every Space as before. A store that refuses
	 * the argument is read the old way until the gate's cooldown passes.
	 */
	holders?: HoldersGate
	/**
	 * Read a single-key `CREATED_AT`/`UPDATED_AT` `orderBy` page of the light `entities` shadow from geo-db with the order's
	 * `.pick` and `.ties` members and compare it (`GEODB_ORDER_PICK_SHADOW`). Needs a serve that reads those members, `multiSpace`
	 * and `allGeoSpaces`; off by default because an older serve refuses the document.
	 */
	orderPick?: boolean
	/**
	 * Read an unordered `entities(spaceId(s), typeId | typeIds, first, offset)` page of the light `entities` shadow from geo-db as a
	 * type scan (`.args.types`, `first = offset + first`) and compare the window's ids in primary-key order (`GEODB_TYPE_SCAN_SHADOW`).
	 * Needs a serve from geo-db c2f9d09cfa; an older one refuses the argument, counted `type_scan_refused`.
	 */
	typeScan?: boolean
	/** Lower a `totalCount`-only `entitiesConnection` to geo-db's entities count form and compare the counts (`GEODB_COUNT_ONLY_SHADOW`; countOnlyShadow.ts). */
	countOnly?: boolean
	/** Compare a relation target's own `valuesList` against geo-db's cards of the targets, read in one batch (`GEODB_NESTED_TARGET_VALUES_SHADOW`). */
	nestedTargetValues?: boolean
	/** Accept a `typeId`-filtered relations list that does not select the type in the light `entities` shadow (`GEODB_RELATIONS_TYPE_FILTER_SHADOW`). */
	relationsTypeFilter?: boolean
	/** Settle a nested name geo-db prints as none (named in a Space outside the read) from its Name in every Space (`GEODB_CROSS_SPACE_NAMES_SHADOW`). */
	crossSpaceNames?: boolean
	/** Compare the fields of a relation row's nested entity (`description`, `types`, `spaceIds`, lists) against one batched card read (`GEODB_NESTED_ENTITY_SHADOW`). */
	nestedEntity?: boolean
	/** Compare the `property { dataTypeId dataTypeName renderableTypeId renderableTypeName format isType }` metadata of a values row against one batched card read of the Properties (`GEODB_PROPERTY_INFO_SHADOW`). */
	propertyInfo?: boolean
	/** Compare an unfiltered `allRelationsList { spaceId }` against geo-db's reference statements of the entity (`.args.relations = true`; `GEODB_RELATIONS_MEMBERSHIP_SHADOW`). */
	relationsMembership?: boolean
}

export type CardCheck =
	| {result: "failed"}
	| {result: "unsupported"; reason: UnsupportedReason}
	| {
			result: "match" | "mismatch"
			uncompared: UncomparedReason[]
			detail: Record<string, unknown>
			/** Reasons in the plan's `uncompared` that this check judged after all (`property_info`, `membership_only`). */
			settles?: UncomparedReason[]
	  }

/** Most spaces one wide comparison reads one by one; an entity in more is not compared (`many_spaces`). A multi-Space line is capped at MAX_LINE_SPACES instead. */
export const MAX_READ_SPACES = 3

/**
 * The spaces a comparison must read: the plan's space, or (every space) the entity's own spaces as PostGraphile
 * lists them. Null when the answer does not say which spaces the entity lives in.
 */
export function spacesToRead(
	plan: Pick<EntityCardPlan, "spaceId">,
	postgres: Pick<NonNullable<PostgresCard>, "spaceIds" | "allValues" | "allRelations">,
): string[] | null {
	if (plan.spaceId) return [plan.spaceId]
	if (postgres.spaceIds) return [...new Set(postgres.spaceIds)]
	const {allValues, allRelations} = postgres
	if (allValues && allRelations && !allValues.truncated && !allRelations.truncated) {
		const spaces = [...allValues.rows, ...allRelations.rows].map((r) => r.spaceId)
		if (spaces.every((s) => s !== null)) return [...new Set((spaces as string[]).map((s) => normalizeUuid(s)))]
	}
	return null
}

/** What one wide read of an entity's cards found. */
export type WideRead =
	| {ok: true; rows: GeoRow[]; bySpace: Map<string, GeoRow[]>; scope: ReadScope; ms: number}
	| {ok: false; result: "failed"}
	| {ok: false; result: "unsupported"; reason: UnsupportedReason}

/**
 * Reads the card of `entityId` in each of `spaces` (concurrently, within the read budget) and concatenates the
 * rows. A space geo-db has not loaded, too many spaces, or an exhausted budget make the entity unsupported
 * rather than failed: geo-db was not asked a question it should have answered. Never throws.
 */
export async function readWide(
	deps: EntityShadowDeps,
	entityId: string,
	spaces: string[],
	label: string,
): Promise<WideRead> {
	if (spaces.length === 0) return {ok: false, result: "unsupported", reason: "no_space"}
	const oneLine = deps.multiSpace === true && spaces.length > 1 && deps.geoDb.readCardSpaces !== undefined
	// One multi-Space line reads them all at the cost of one read; only the per-Space fallback is capped at 3.
	if (spaces.length > (oneLine ? MAX_LINE_SPACES : MAX_READ_SPACES))
		return {ok: false, result: "unsupported", reason: "many_spaces"}
	if (deps.budget && !deps.budget.tryAcquire(oneLine ? 1 : spaces.length))
		return {ok: false, result: "unsupported", reason: "throttled"}
	const started = performance.now()
	if (oneLine) {
		try {
			const bySpace = await askGeoDbSpaces(deps, conformingId("entity", entityId), spaces, {names: true})
			const rows = spaces.flatMap((spaceId) => bySpace.get(spaceId) ?? [])
			const populated = spaces.filter((spaceId) => (bySpace.get(spaceId)?.length ?? 0) > 0)
			return {ok: true, rows, bySpace, scope: {spaces, populated}, ms: Math.round(performance.now() - started)}
		} catch (error) {
			const reason = error instanceof GeoDbError ? error.reason : "error"
			if (reason === "unmapped_space") return {ok: false, result: "unsupported", reason: "unmapped_space"}
			deps.log.warn(`${label} read failed`, {entityId, spaces, reason, error: String(error)})
			return {ok: false, result: "failed"}
		}
	}
	const answers = await Promise.all(
		spaces.map(async (spaceId) => {
			try {
				return {spaceId, rows: await askGeoDb(deps, conformingId("entity", entityId), spaceId, {names: true})}
			} catch (error) {
				return {spaceId, error}
			}
		}),
	)
	const rows: GeoRow[] = []
	const bySpace = new Map<string, GeoRow[]>()
	const populated: string[] = []
	for (const answer of answers) {
		if ("error" in answer) {
			const reason = answer.error instanceof GeoDbError ? answer.error.reason : "error"
			if (reason === "unmapped_space") return {ok: false, result: "unsupported", reason: "unmapped_space"}
			deps.log.warn(`${label} read failed`, {
				entityId,
				spaceId: answer.spaceId,
				reason,
				error: String(answer.error),
			})
			return {ok: false, result: "failed"}
		}
		if (answer.rows.length > 0) populated.push(answer.spaceId)
		bySpace.set(answer.spaceId, answer.rows)
		rows.push(...answer.rows)
	}
	return {ok: true, rows, bySpace, scope: {spaces, populated}, ms: Math.round(performance.now() - started)}
}

/** How long a refused holders read keeps the lookup off, so a store that predates the argument is not asked every read. */
const HOLDERS_COOLDOWN_MS = 5 * 60_000

/** Whether the holders lookup may be tried, and a note that the store refused it. */
export type HoldersGate = {allowed(): boolean; refused(): void}

export function createHoldersGate(options: {cooldownMs?: number; now?: () => number} = {}): HoldersGate {
	const cooldownMs = options.cooldownMs ?? HOLDERS_COOLDOWN_MS
	const now = options.now ?? Date.now
	let offUntil = 0
	return {
		allowed: () => now() >= offUntil,
		refused: () => {
			offUntil = now() + cooldownMs
		},
	}
}

/**
 * The Spaces of `lines` that hold `entityId`, from `.args.holders` (one read a line, all at once), ascending within a
 * line. Null when the lookup cannot answer and the card should be read across every Space as before: no gate or client
 * support, the gate cooling down after a refusal, or a serve that predates the argument refusing it (the exact
 * refusal `isHoldersUnknownArgument` names; it starts the cooldown, and a refusal ends the worker's session, so it
 * is not retried every read). Every other failure (a dead pipe, a timeout, an overloaded pool, a malformed row,
 * another refusal) throws, as the card read it replaces would, and the caller counts it as failed.
 */
async function readHolderSpaces(deps: EntityShadowDeps, entityId: string, lines: string[][], label: string) {
	const readHolders = deps.geoDb.readHolders?.bind(deps.geoDb)
	const gate = deps.holders
	if (!gate || !readHolders || !gate.allowed()) return null
	let answers: string[][]
	try {
		answers = await Promise.all(
			lines.map((line) =>
				readHolders({
					geoSpaceIds: line,
					entityId: conformingId("entity", entityId),
					...(deps.everySpaceTimeoutMs ? {timeoutMs: deps.everySpaceTimeoutMs} : {}),
				}),
			),
		)
	} catch (error) {
		// Only the old serve's refusal of the argument means "read without it". A dead pipe, a timeout, an overload,
		// a malformed row or any other refusal is a failed read, and the caller counts it so.
		if (!isHoldersUnknownArgument(error)) throw error
		gate.refused()
		deps.log.warn(`${label} serve does not know holders; reading every Space`, {entityId, error: String(error)})
		return null
	}
	const asked = new Set(lines.flat())
	const holders = answers.flat()
	if (holders.some((space) => !asked.has(space))) {
		gate.refused()
		deps.log.warn(`${label} holders lookup answered an unrequested space; reading every Space`, {entityId})
		return null
	}
	return holders
}

/** Most Spaces one every-Space read names: geo-db's `MAX_SOURCES`, past which it refuses the request. */
export const MAX_EVERY_SPACES = 1024

/**
 * Reads the card of `entityId` across every Space geo-db has loaded, in one pipe line, for an answer that does
 * not say where the entity lives. geo-db prunes the union by its `presence` index, so only the Spaces holding the
 * entity are read. The scope and `bySpace` are in geo-db Space ids; `populated` is the Spaces that hold it.
 * Needs `allGeoSpaces` and the multi-Space line; without them the read is `no_space`. Never throws.
 */
export async function readEverySpace(deps: EntityShadowDeps, entityId: string, label: string): Promise<WideRead> {
	const readCardSpaces = deps.geoDb.readCardSpaces?.bind(deps.geoDb)
	if (!deps.allGeoSpaces || deps.multiSpace !== true || !readCardSpaces)
		return {ok: false, result: "unsupported", reason: "no_space"}
	let spaces: string[]
	try {
		spaces = await deps.allGeoSpaces()
	} catch (error) {
		deps.log.warn(`${label} space map unreadable`, {entityId, error: String(error)})
		return {ok: false, result: "failed"}
	}
	if (spaces.length === 0) return {ok: false, result: "unsupported", reason: "no_space"}
	if (spaces.length > MAX_EVERY_SPACES && deps.everySpaceChunks !== true)
		return {ok: false, result: "unsupported", reason: "many_spaces"}
	const chunked = (all: string[]): string[][] => {
		const out: string[][] = []
		for (let at = 0; at < all.length; at += MAX_EVERY_SPACES) out.push(all.slice(at, at + MAX_EVERY_SPACES))
		return out
	}
	let lines = chunked(spaces)
	if (deps.everySpaceBudget && !deps.everySpaceBudget.tryAcquire(lines.length))
		return {ok: false, result: "unsupported", reason: "throttled"}
	const started = performance.now()
	const rows: GeoCardRow[] = []
	try {
		// With the holders lookup, the card is read only in the Spaces that hold the entity (at most as many lines as
		// the full read, so the budget taken above covers both reads).
		const holders = await readHolderSpaces(deps, entityId, lines, label)
		if (holders) {
			if (holders.length === 0) {
				observeGeoDbRead((performance.now() - started) / 1000, "ok")
				return {
					ok: true,
					rows: [],
					bySpace: new Map(),
					scope: {spaces, populated: []},
					ms: Math.round(performance.now() - started),
				}
			}
			lines = chunked(holders)
		}
		// The lines go out together: every one holds a token of `everySpaceBudget`, taken above, so the read's
		// width is the budget's, and the whole read waits for its slowest line (each line's own timeout bounds it).
		// `Promise.all` keeps the lines' order, so the rows concatenate as the serial walk's did.
		const answers = await Promise.all(
			lines.map((line) =>
				readCardSpaces({
					geoSpaceIds: line,
					entityId: conformingId("entity", entityId),
					names: true,
					...(deps.everySpaceTimeoutMs ? {timeoutMs: deps.everySpaceTimeoutMs} : {}),
				}),
			),
		)
		lines.forEach((line, i) => {
			// A one-Space line prints bare statement ids, so its rows name no Space.
			const only = line.length === 1 ? line[0] : undefined
			for (const row of answers[i] ?? []) rows.push(only && row.space === undefined ? {...row, space: only} : row)
		})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		const reason = error instanceof GeoDbError ? error.reason : "error"
		deps.log.warn(`${label} every-space read failed`, {
			entityId,
			spaces: spaces.length,
			reason,
			error: String(error),
		})
		return {ok: false, result: "failed"}
	}
	const asked = new Set(spaces)
	const bySpace = new Map<string, GeoRow[]>()
	const plain: GeoRow[] = []
	for (const row of rows) {
		const {space: printed, ...rest} = row
		// A one-Space line prints bare statement ids, so its rows name no Space (the line's own Space is set above).
		const space = printed ?? (spaces.length === 1 ? spaces[0] : undefined)
		if (!space || !asked.has(space)) {
			deps.log.warn(`${label} every-space read answered an unrequested space`, {entityId, space})
			return {ok: false, result: "failed"}
		}
		const list = bySpace.get(space) ?? []
		list.push(rest)
		bySpace.set(space, list)
		plain.push(rest)
	}
	return {
		ok: true,
		rows: plain,
		bySpace,
		scope: {spaces, populated: [...bySpace.keys()]},
		ms: Math.round(performance.now() - started),
	}
}

/** Most entities whose names one wide comparison reads across every Space. */
const MAX_NAME_IDS = 64

/** The ids of the nested properties, Types and targets (and listed Types) PostGraphile named, in geo-db's id form. */
export function nestedNameIds(card: Pick<NonNullable<PostgresCard>, "nested" | "types" | "typeNames">): string[] {
	const ids = new Set<string>()
	const add = (refs: NamedRef[] | undefined) => {
		for (const ref of refs ?? []) if (ref.name !== null) ids.add(conformingId("entity", ref.id))
	}
	add(card.nested?.property)
	add(card.nested?.type)
	add(card.nested?.toEntity)
	card.types?.forEach((id, at) => {
		if (card.typeNames?.[at] != null) ids.add(conformingId("entity", id))
	})
	return [...ids]
}

/**
 * The Name each of `ids` holds in every Space geo-db has loaded (one batch read over all of them), for the nested names
 * geo-db prints as none because the thing is named in a Space outside the comparison's scope. Null when it cannot be
 * read (no every-Space reads, too many Spaces or ids, an empty budget, a failed read): the names stay inconclusive.
 * Never throws.
 */
export async function readNames(
	deps: EntityShadowDeps,
	ids: string[],
	label: string,
): Promise<Map<string, string[]> | null> {
	const readCards = deps.geoDb.readCards?.bind(deps.geoDb)
	if (!deps.allGeoSpaces || !readCards || ids.length === 0 || ids.length > MAX_NAME_IDS) return null
	try {
		const spaces = await deps.allGeoSpaces()
		if (spaces.length === 0 || spaces.length > MAX_EVERY_SPACES) return null
		if (deps.everySpaceBudget && !deps.everySpaceBudget.tryAcquire(1)) return null
		const started = performance.now()
		let rows: GeoCardRow[]
		try {
			rows = await readCards({geoSpaceIds: spaces, entityIds: ids})
			observeGeoDbRead((performance.now() - started) / 1000, "ok")
		} catch (error) {
			observeGeoDbRead((performance.now() - started) / 1000, "error")
			throw error
		}
		const names = new Map<string, string[]>()
		for (const row of rows) {
			if (row.prop !== NAME_PROPERTY || !row.entity) continue
			const held = names.get(row.entity) ?? []
			held.push(row.raw)
			names.set(row.entity, held)
		}
		return names
	} catch (error) {
		deps.log.warn(`${label} name read failed`, {ids: ids.length, error: String(error)})
		return null
	}
}

/**
 * `compareDerived`, and when it leaves a name inconclusive for want of another Space (`cross_space`) and
 * `crossSpaceNames` is on, once more with the names geo-db holds in every Space.
 */
export async function compareDerivedWide(
	deps: EntityShadowDeps,
	plan: Pick<EntityCardPlan, "spaceId" | "allValues">,
	card: NonNullable<PostgresCard>,
	geo: GeoRow[],
	scope: ReadScope | undefined,
	label: string,
): Promise<DerivedChecks> {
	const first = compareDerived(plan, card, geo, scope)
	if (deps.crossSpaceNames !== true || !first.uncompared.includes("cross_space")) return first
	const ids = nestedNameIds(card)
	const names = await readNames(deps, ids, label)
	return names ? compareDerived(plan, card, geo, scope, names) : first
}

/**
 * Asks geo-db for one entity's card (with names) and judges PostGraphile's card against it: the statement diff
 * plus the derived checks. When the plan has no space the entity's own spaces are all read and their cards are
 * compared as one. Never throws; a failed read is logged and reported as `failed`. The caller counts.
 */
export async function checkCard(
	deps: EntityShadowDeps,
	plan: Pick<EntityCardPlan, "spaceId" | "allValues">,
	postgres: NonNullable<PostgresCard>,
	entityId: string,
	label: string,
): Promise<CardCheck> {
	const spaces = spacesToRead(plan, postgres)
	if (!spaces) return {result: "unsupported", reason: "no_space"}
	let read: WideRead
	if (plan.spaceId) {
		// One space, as before: not metered, and a space geo-db lacks is a failed read.
		const started = performance.now()
		try {
			const rows = await askGeoDb(deps, conformingId("entity", entityId), plan.spaceId, {names: true})
			read = {
				ok: true,
				rows,
				bySpace: new Map([[plan.spaceId, rows]]),
				scope: {spaces, populated: rows.length > 0 ? spaces : []},
				ms: Math.round(performance.now() - started),
			}
		} catch (error) {
			deps.log.warn(`${label} read failed`, {
				entityId,
				spaceId: plan.spaceId,
				reason: error instanceof GeoDbError ? error.reason : "error",
				error: String(error),
			})
			return {result: "failed"}
		}
	} else if (spaces.length === 0) {
		// The answer holds no row and no Space for the entity: nothing says where to look, so geo-db is asked across
		// every Space it has loaded (it prunes by presence) and must hold nothing either.
		read = await readEverySpace(deps, entityId, label)
	} else read = await readWide(deps, entityId, spaces, label)
	if (!read.ok) return read.result === "failed" ? {result: "failed"} : {result: "unsupported", reason: read.reason}
	const rows = read.rows
	const diff = diffCards(statementsFromPostgres(postgres.values, postgres.relations), rows, {skipEmptyText: true})
	const derived = await compareDerivedWide(deps, plan, postgres, rows, read.scope, label)
	const wrong = Object.entries(derived.checks)
		.filter(([, result]) => result === "mismatch")
		.map(([field]) => field)
	return {
		result: isEmptyDiff(diff) && wrong.length === 0 ? "match" : "mismatch",
		uncompared: derived.uncompared,
		detail: {
			entityId,
			spaceId: plan.spaceId,
			spaces: spaces.length,
			onlyPostgres: diff.onlyPostgres.length,
			onlyGeoDb: diff.onlyGeoDb.length,
			skipped: diff.skipped,
			derivedMismatches: wrong,
			samplePostgres: diff.onlyPostgres.slice(0, SAMPLE),
			sampleGeoDb: diff.onlyGeoDb.slice(0, SAMPLE),
			geoDbMs: read.ms,
		},
	}
}

/** Counts a light outcome under `field`, and logs a mismatch. */
export function countLight(
	deps: Pick<EntityShadowDeps, "log">,
	field: string,
	outcome: LightOutcome,
	context: Record<string, unknown>,
): void {
	if (outcome.result === "skip") return
	if (outcome.result === "failed") {
		recordGatewayShadow(field, "failed")
		return
	}
	if (outcome.result === "unsupported") {
		recordGatewayShadow(field, "unsupported_selection")
		recordGatewayUnsupported(field, outcome.reason)
		return
	}
	for (const reason of new Set(outcome.uncompared)) recordGatewayUncompared(field, reason)
	recordGatewayShadow(field, outcome.result)
	if (outcome.result === "mismatch")
		deps.log.warn(`gateway ${field} shadow mismatch (light)`, {...context, ...outcome.detail})
}

/** The `entity` shadow for the gateway. Registers its metric series; never throws. */
export function createEntityShadow(deps: EntityShadowDeps): FieldShadow {
	activateGatewayMetrics(ENTITY_FIELD)
	const shadow: FieldShadow = async ({params, document, classification, response}) => {
		try {
			const analysis = analyzeEntityOperation(document, classification.operationName, params.variables)
			if (!analysis.ok) {
				// The card shadow cannot settle it: try the light selection (every space, partial lists).
				const light = analyzeLightEntity(document, classification.operationName, params.variables, {
					relationsTypeFilter: deps.relationsTypeFilter === true,
				})
				if (light.ok) {
					let lightBody: unknown
					try {
						lightBody = await response.json()
					} catch {
						return
					}
					await countLight(
						deps,
						ENTITY_FIELD,
						await runLight(deps, light.plan, lightBody, "gateway entity shadow"),
						{
							operationName: classification.operationName,
						},
					)
					return
				}
				const backlinks = deps.backlinks
					? analyzeBacklinks(document, classification.operationName, params.variables)
					: null
				if (backlinks?.ok) {
					let backlinksBody: unknown
					try {
						backlinksBody = await response.json()
					} catch {
						return
					}
					countLight(
						deps,
						ENTITY_FIELD,
						await runBacklinks(deps, backlinks.plan, backlinksBody, "gateway entity shadow"),
						{operationName: classification.operationName},
					)
					return
				}
				recordGatewayShadow(ENTITY_FIELD, "unsupported_selection")
				recordGatewayUnsupported(ENTITY_FIELD, analysis.reason)
				noteUnsupported(
					deps,
					ENTITY_FIELD,
					analysis.reason,
					document,
					classification.operationName,
					params.variables,
				)
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
			const check = await checkCard(deps, plan, postgres, plan.entityId, "gateway entity shadow")
			if (check.result === "failed") {
				recordGatewayShadow(ENTITY_FIELD, "failed")
				return
			}
			if (check.result === "unsupported") {
				recordGatewayShadow(ENTITY_FIELD, "unsupported_selection")
				recordGatewayUnsupported(ENTITY_FIELD, check.reason)
				return
			}
			let result = check.result
			let detail = check.detail
			const reasons = new Set([...plan.uncompared, ...check.uncompared])
			if (deps.nestedEntity === true && postgres.nestedEntities && reasons.has("nested_entity")) {
				const nested = await checkNestedEntities(deps, postgres.nestedEntities, "gateway entity shadow")
				if (nested.read) {
					for (const reason of nested.uncompared) reasons.add(reason)
					// A selection left unplanned (backlinks, odd arguments) keeps it: only a fully judged one settles.
					if (nested.settled) reasons.delete("nested_entity")
					const wrong = Object.keys(nested.mismatches)
					if (wrong.length > 0) {
						result = "mismatch"
						detail = {...detail, nestedMismatches: nested.mismatches}
					}
				}
			}
			if (deps.propertyInfo === true && postgres.propertyInfo && reasons.has("property_info")) {
				const verdict = await checkPropertyInfo(deps, postgres.propertyInfo, "gateway entity shadow")
				if (verdict.read) {
					for (const reason of verdict.uncompared) reasons.add(reason)
					if (verdict.settled) reasons.delete("property_info")
					if (Object.keys(verdict.mismatches).length > 0) {
						result = "mismatch"
						detail = {...detail, propertyInfoMismatches: verdict.mismatches}
					}
				}
			}
			if (deps.relationsMembership === true && postgres.allRelations && reasons.has("membership_only")) {
				// The Spaces PostgreSQL says hold the entity (its own lists), in geo-db's ids; geo-db's reference statements there
				// may not outnumber the rows PostgreSQL lists for the Space.
				const source =
					postgres.spaceIds !== undefined ||
					(postgres.allValues !== undefined && !postgres.allValues.truncated)
				const named = new Set<string>()
				for (const id of postgres.spaceIds ?? []) named.add(normalizeUuid(id))
				for (const r of [...(postgres.allValues?.rows ?? []), ...postgres.allRelations.rows])
					if (r.spaceId !== null) named.add(normalizeUuid(r.spaceId))
				const populated = new Set<string>()
				for (const id of named) {
					const geo = await deps.geoSpaceOf(id)
					if (geo) populated.add(geo)
				}
				if (source && populated.size > 0 && populated.size <= MAX_EVERY_SPACES) {
					const verdict = await checkRelationsMembership(
						deps,
						plan.entityId,
						{
							spaces: postgres.allRelations.rows.map((r) => r.spaceId),
							cut: postgres.allRelations.truncated,
						},
						[...populated],
						true,
						async (space) => (await deps.geoSpaceOf(space)) ?? null,
						"gateway entity shadow",
					)
					if (verdict.read) {
						reasons.delete("membership_only")
						if (verdict.mismatch) {
							result = "mismatch"
							detail = {...detail, relationsMembership: verdict.detail}
						}
					}
				}
			}
			for (const reason of reasons) recordGatewayUncompared(ENTITY_FIELD, reason)
			recordGatewayShadow(ENTITY_FIELD, result)
			if (result === "mismatch")
				deps.log.warn("gateway entity shadow mismatch", {
					operationName: classification.operationName,
					...detail,
				})
		} catch (error) {
			// Whatever went wrong, the user's response is already gone; count it and move on.
			try {
				recordGatewayShadow(ENTITY_FIELD, "failed")
				deps.log.warn("gateway entity shadow errored", {error: String(error)})
			} catch {}
		}
	}
	shadow.tolerates = ENTITY_TOLERATED_ROOTS
	return shadow
}
