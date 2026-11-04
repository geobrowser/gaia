/**
 * Shadow of the root field `entity(id)`: PostGraphile answers, geo-db is asked the same question in the
 * background, and the two statement sets are compared.
 *
 * geo-db answers an entity's *card*: its values and its outgoing relations in one space (geodb/card.ts). So the
 * shadow qualifies exactly the selections a card can settle, and counts every other selection as
 * `unsupported_selection` rather than guessing:
 *
 *   entity(id: <uuid>) {
 *     id
 *     valuesList(first: N, filter: {spaceId: {is: <uuid>}}) { propertyId|property{id}, text, integer, float,
 *                                                            boolean, datetime, date, decimal (+ ignored columns) }
 *     relationsList(first: N, filter: {spaceId: {is: <same uuid>}}) { typeId|type{id}, toEntityId|toEntity{id}
 *                                                                    (+ ignored columns) }
 *   }
 *
 * Both lists are required, because a card does not say which rows are values and which are relations. `id`
 * and `__typename` are allowed; any other entity field (name, description, types, spaceIds, updatedAt,
 * backlinks, relations connection, unfiltered lists) is unsupported. Statements are mapped, normalised and
 * diffed by the card shadow's own functions (geodb/card.ts), through its read (geodb/service.ts `askGeoDb`),
 * pool and space map.
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
import {activateGatewayMetrics, recordGatewayShadow, recordGatewayUnsupported, type UnsupportedReason} from "./metrics"

export const ENTITY_FIELD = "entity"

/** PostGraphile's own page-size ceiling (paginationCapPlugin); a list this long may have been cut short. */
const PAGE_CAP = 1000
const SAMPLE = 5
const MAX_FRAGMENT_DEPTH = 8

const VALUE_COLUMNS = ["text", "integer", "float", "boolean", "datetime", "date", "decimal"] as const
/** Columns of a value row the card mapping does not compare; selecting them is allowed. */
const VALUE_IGNORED = ["spaceId", "language", "unit", "point", "time", "schedule"] as const
const RELATION_IGNORED = ["id", "spaceId", "entityId", "fromEntityId", "position", "verified", "toSpaceId"] as const

type ListPlan = {
	/** Response key of the list in the entity object. */
	key: string
	first: number
	/** Response key of each column the comparison reads. */
	columns: Record<string, string>
	/** Response key of `{id}` for a column selected as a nested object (`property { id }`), by column. */
	nested: Record<string, string>
}

export type EntityPlan = {
	/** Response key of the entity in `data`. */
	key: string
	entityId: string
	spaceId: string
	values: ListPlan
	relations: ListPlan
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

function planList(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	variables: Record<string, unknown>,
	spec: {
		columns: readonly string[]
		/** column -> the nested object that may carry it as `{id}` */
		nestedIdOf: Record<string, string>
		ignored: readonly string[]
	},
): {list: ListPlan; spaceId: string} {
	const args = argsOf(field, variables)
	for (const name of Object.keys(args)) if (name !== "first" && name !== "filter") refuse("args")
	const first = args.first
	if (typeof first !== "number" || !Number.isInteger(first) || first < 1) throw new Unsupported("args")
	const spaceId = spaceFromFilter(args.filter)
	if (!field.selectionSet) refuse("selection")
	const columns: Record<string, string> = {}
	const nested: Record<string, string> = {}
	const seen = new Set<string>()
	for (const child of fieldsOf(field.selectionSet as SelectionSetNode, fragments)) {
		const name = child.name.value
		const key = responseKeyOf(child)
		if (seen.has(key)) refuse("selection")
		seen.add(key)
		if (child.arguments?.length) refuse("args")
		if (name === "__typename") continue
		const nestedColumn = Object.entries(spec.nestedIdOf).find(([, object]) => object === name)?.[0]
		if (nestedColumn) {
			// `property { id }`: the object is allowed only to carry its id.
			if (!child.selectionSet) refuse("selection")
			for (const inner of fieldsOf(child.selectionSet as SelectionSetNode, fragments)) {
				if (
					(inner.name.value !== "id" && inner.name.value !== "__typename") ||
					inner.arguments?.length ||
					inner.selectionSet
				) {
					refuse("selection")
				}
				if (inner.name.value === "id") nested[nestedColumn] ??= `${key}\t${responseKeyOf(inner)}`
			}
			continue
		}
		if (child.selectionSet) refuse("selection")
		if (spec.columns.includes(name)) columns[name] ??= key
		else if (!spec.ignored.includes(name)) refuse("selection")
	}
	return {list: {key: responseKeyOf(field), first, columns, nested}, spaceId}
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

		const roots = fieldsOf(operation.selectionSet, fragments).filter((f) => f.name.value !== "__typename")
		const root = roots[0]
		if (roots.length !== 1 || !root || root.name.value !== ENTITY_FIELD)
			return {ok: false, reason: "not_single_root"}
		const rootArgs = argsOf(root, resolved)
		if (Object.keys(rootArgs).length !== 1 || !("id" in rootArgs)) return {ok: false, reason: "args"}
		const entityId = uuidArg(rootArgs.id)
		if (!root.selectionSet) return {ok: false, reason: "selection"}

		let values: ReturnType<typeof planList> | null = null
		let relations: ReturnType<typeof planList> | null = null
		const seen = new Set<string>()
		for (const field of fieldsOf(root.selectionSet, fragments)) {
			const key = responseKeyOf(field)
			if (seen.has(key)) return {ok: false, reason: "selection"}
			seen.add(key)
			const name = field.name.value
			if (name === "id" || name === "__typename") {
				if (field.arguments?.length) return {ok: false, reason: "args"}
				if (field.selectionSet) return {ok: false, reason: "selection"}
			} else if (name === "valuesList" && !values) {
				values = planList(field, fragments, resolved, {
					columns: ["propertyId", ...VALUE_COLUMNS],
					nestedIdOf: {propertyId: "property"},
					ignored: VALUE_IGNORED,
				})
			} else if (name === "relationsList" && !relations) {
				relations = planList(field, fragments, resolved, {
					columns: ["typeId", "toEntityId"],
					nestedIdOf: {typeId: "type", toEntityId: "toEntity"},
					ignored: RELATION_IGNORED,
				})
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
				values: values.list,
				relations: relations.list,
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

export type PostgresCard = {values: ValueRow[]; relations: RelationRow[]; truncated: boolean} | null

/**
 * Reads the two lists out of PostGraphile's answer. Null when the answer is not a plain successful one for this
 * plan (errors, no data, an unexpected shape): that is not a verdict on geo-db, so nothing is counted.
 */
export function extractPostgresCard(body: unknown, plan: EntityPlan): PostgresCard {
	const root = asRecord(body)
	if (!root || root.errors !== undefined) return null
	const data = asRecord(root.data)
	if (!data || !(plan.key in data)) return null
	const entity = data[plan.key]
	// An entity PostGraphile does not have holds no statements.
	if (entity === null) return {values: [], relations: [], truncated: false}
	const entityObject = asRecord(entity)
	const valueNodes = entityObject?.[plan.values.key]
	const relationNodes = entityObject?.[plan.relations.key]
	if (!Array.isArray(valueNodes) || !Array.isArray(relationNodes)) return null
	const values: ValueRow[] = []
	for (const raw of valueNodes) {
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
	for (const raw of relationNodes) {
		const node = asRecord(raw)
		if (!node) return null
		const typeId = read(node, plan.relations, "typeId")
		const toEntityId = read(node, plan.relations, "toEntityId")
		if (typeof typeId !== "string" || typeof toEntityId !== "string") return null
		relations.push({type_id: typeId, to_entity_id: toEntityId, to_space_id: null})
	}
	const cut = (count: number, list: ListPlan) => count >= Math.min(list.first, PAGE_CAP)
	return {
		values,
		relations,
		truncated: cut(valueNodes.length, plan.values) || cut(relationNodes.length, plan.relations),
	}
}

export type EntityShadowDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {log: CardLogger}

/** The `entity` shadow for the gateway. Registers its metric series; never throws. */
export function createEntityShadow(deps: EntityShadowDeps): FieldShadow {
	activateGatewayMetrics(ENTITY_FIELD)
	return async ({params, document, classification, response}) => {
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
			if (isEmptyDiff(diff)) {
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
}
