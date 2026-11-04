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

import type {DocumentNode, FieldNode, SelectionSetNode} from "graphql"

import {
	conformingId,
	diffCards,
	type GeoCardRow,
	type RelationRow,
	statementsFromPostgres,
	type ValueRow,
} from "../geodb/card"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
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
	readName,
	readWide,
	responseKeyOf,
	TYPES_PROPERTY,
	Unsupported,
	uuidArg,
	VALUE_COLUMNS,
	VALUE_IGNORED,
} from "./entityShadow"
import type {UncomparedReason, UnsupportedReason} from "./metrics"

/** Most returned entities one `entities` operation compares; the rest are `sampled_rows`. */
export const MAX_SAMPLED_ENTITIES = 3

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
}

const noted = new Set<string>()
const MAX_NOTED = 200

/**
 * Logs, once per process for each (field, reason, operation name), the shape of an operation that could not be
 * shadowed: its root arguments and the top-level fields it selects. No values, so nothing private is logged. It
 * is how the real query mix behind `gaia_api_gateway_shadow_unsupported_total` is ranked. At most MAX_NOTED lines.
 */
export function noteUnsupported(
	deps: Pick<EntityShadowDeps, "log">,
	field: string,
	reason: string,
	document: DocumentNode,
	operationName: string | null,
): void {
	try {
		const key = `${field}\t${reason}\t${operationName ?? ""}`
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
							filterKeys: (selection.arguments ?? []).flatMap((a) =>
								a.name.value === "filter" && a.value.kind === "ObjectValue"
									? a.value.fields.map((f) => f.name.value)
									: [],
							),
							selects: (selection.selectionSet?.selections ?? []).flatMap((c) =>
								c.kind === "Field" ? [c.name.value] : [],
							),
						},
					]
				: [],
		)
		deps.log.info("gateway shadow unsupported", {field, reason, operationName, roots})
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

export type LightEntityPlan = LightPlan & {kind: "entity"; key: string; entityId: string}
export type LightEntitiesPlan = LightPlan & {
	kind: "entities"
	key: string
	claims: FilterClaim[]
	/** The ids the filter asked for (`id: {in}`), if that was the filter. */
	ids: string[] | null
	/** `entities(spaceId:)` or `entities(spaceIds: {in})`: the entity must hold a statement in one of these spaces. */
	spaceIds: string[] | null
}

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
function planLightList(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	resolved: Record<string, unknown>,
	uncompared: Set<UncomparedReason>,
): LightList {
	const name = field.name.value
	const kind = name === "values" || name === "valuesList" ? "values" : "relations"
	const connection = name === "values" || name === "relations"
	const args = argsOf(field, resolved)
	for (const arg of Object.keys(args)) if (!["first", "filter", "after", "offset"].includes(arg)) refuse("args")
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
		if (!has("propertyId")) refuse("selection")
		if (!VALUE_COLUMNS.every(has)) uncompared.add("partial_values")
	} else if (!has("typeId") || !has("toEntityId")) refuse("selection")
	return {
		kind,
		key: responseKeyOf(field),
		nodesKey,
		rows: {key: responseKeyOf(field), first, ...rows, nodesKey, hasNext: null},
		spaceKey: rows.columns.spaceId ?? null,
	}
}

function planLightEntity(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	resolved: Record<string, unknown>,
	uncompared: Set<UncomparedReason>,
): {card: EntityCardPlan; lists: LightList[]} {
	const lists: LightList[] = []
	const card = planEntitySelection(set, fragments, resolved, uncompared, (field) => {
		lists.push(planLightList(field, fragments, resolved, uncompared))
	})
	// No lists: the derived fields (name, description, types) of the entity's own spaces are all that is judged.
	if (lists.length === 0 && !(card.spaceIds && (card.name || card.description || card.types))) refuse("selection")
	// Rows with no space and no `spaceIds` leave nothing to say which cards to read.
	if (!card.spaceIds && !lists.some((list) => list.spaceKey)) refuse("no_space")
	uncompared.add("partial_values")
	return {card, lists}
}

/** `entity(id)` with the light selection. Same roots as the card shadow (`relations` is tolerated). */
export function analyzeLightEntity(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
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
		const {card, lists} = planLightEntity(root.selectionSet, fragments, resolved, uncompared)
		return {
			ok: true,
			plan: {kind: "entity", key: responseKeyOf(root), entityId, card, lists, uncompared: [...uncompared]},
		}
	} catch (error) {
		return {ok: false, reason: error instanceof Unsupported ? error.reason : "selection"}
	}
}

/** The soundness claims of an `entities` filter, and the ids it lists. Anything else is only uncompared. */
function readFilter(filter: unknown, uncompared: Set<UncomparedReason>): Pick<LightEntitiesPlan, "claims" | "ids"> {
	const claims: FilterClaim[] = []
	let ids: string[] | null = null
	const object = filter === undefined || filter === null ? {} : asRecord(filter)
	if (!object) return refuse("filter")
	uncompared.add("filter_completeness")
	for (const [key, clause] of Object.entries(object)) {
		const c = asRecord(clause)
		if (key === "typeIds" && c && typeof c.anyEqualTo === "string" && isValidUuid(c.anyEqualTo)) {
			claims.push({kind: "type", typeIds: [normalizeUuid(c.anyEqualTo)]})
		} else if (
			key === "id" &&
			c &&
			Array.isArray(c.in) &&
			c.in.every((id) => typeof id === "string" && isValidUuid(id))
		) {
			ids = (c.in as string[]).map((id) => normalizeUuid(id))
		} else if (key === "relations" && c) {
			const some = asRecord(c.some)
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
			else uncompared.add("filter_completeness")
		} else uncompared.add("filter_completeness")
	}
	return {claims, ids}
}

const ENTITIES_ARGS = [
	"filter",
	"first",
	"offset",
	"spaceId",
	"spaceIds",
	"typeId",
	"typeIds",
	"systemTypeId",
	"systemTypeIds",
]

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

/** `entities(filter, first, offset, spaceId, spaceIds, typeId, typeIds)` with the light selection, compared on a sample of what it returns. */
export function analyzeLightEntities(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
): LightAnalysis<LightEntitiesPlan> {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const root = roots[0]
		if (roots.length !== 1 || !root || root.name.value !== "entities") return {ok: false, reason: "not_single_root"}
		if (!root.selectionSet) return {ok: false, reason: "selection"}
		const args = argsOf(root, resolved)
		for (const arg of Object.keys(args)) if (!ENTITIES_ARGS.includes(arg)) refuse("args")
		for (const arg of ["first", "offset"])
			if (args[arg] !== undefined && args[arg] !== null && !Number.isInteger(args[arg])) refuse("args")
		const uncompared = new Set<UncomparedReason>()
		const {claims, ids} = readFilter(args.filter, uncompared)
		const spaceIds = readRootArgs(args, claims, uncompared)
		const {card, lists} = planLightEntity(root.selectionSet, fragments, resolved, uncompared)
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

/** Null when the object is not a plain answer for the plan. */
export function lightCardFromEntity(entity: unknown, plan: LightPlan): LightCard | null {
	const object = asRecord(entity)
	if (!object) return null
	const values: ValueRow[] = []
	const relations: RelationRow[] = []
	const valueSpaces: (string | null)[] = []
	const relationSpaces: (string | null)[] = []
	const nested: NonNullable<NonNullable<PostgresCard>["nested"]> = {}
	for (const list of plan.lists) {
		const holder = list.nodesKey ? asRecord(object[list.key]) : object
		const nodes = list.nodesKey ? holder?.[list.nodesKey] : object[list.key]
		if (!Array.isArray(nodes)) return null
		for (const raw of nodes) {
			const node = asRecord(raw)
			if (!node) return null
			const space = spaceOf(node, list.spaceKey)
			if (list.kind === "values") {
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
				const typeId = read(node, list.rows, "typeId")
				const toEntityId = read(node, list.rows, "toEntityId")
				if (typeof typeId !== "string" || typeof toEntityId !== "string") return null
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
			const id = plan.card.types.idKey ? asRecord(node)?.[plan.card.types.idKey] : undefined
			if (typeof id !== "string") return null
			ids.push(id)
			const name = plan.card.types.nameKey ? asRecord(node)?.[plan.card.types.nameKey] : undefined
			names.push(typeof name === "string" ? name : null)
		}
		card.types = ids
		if (plan.card.types.nameKey) card.typeNames = names
	}
	if (Object.keys(nested).length > 0) card.nested = nested
	return {card, valueSpaces, relationSpaces}
}

/** The spaces to read: the entity's own, else the spaces its rows name. */
function lightSpaces(light: LightCard, extra: string[] | null): string[] | null {
	const spaces = new Set<string>(light.card.spaceIds ?? [])
	if (!light.card.spaceIds) for (const s of [...light.valueSpaces, ...light.relationSpaces]) if (s) spaces.add(s)
	for (const s of extra ?? []) spaces.add(s)
	return spaces.size > 0 ? [...spaces] : null
}

export type LightCheck = CardCheck

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
	options: {claims?: FilterClaim[]; spaceIds?: string[] | null} = {},
): Promise<LightCheck> {
	const spaces = lightSpaces(light, options.spaceIds ?? null)
	if (!spaces) return {result: "unsupported", reason: "no_space"}
	const read = await readWide(deps, entityId, spaces, label)
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
		const diff = diffCards(statementsFromPostgres(rows.values, rows.relations), pool)
		skipped += diff.skipped
		for (const s of diff.onlyPostgres) missing.push({space, prop: s.prop, norm: s.norm})
	}
	const derived = compareDerived(plan.card, light.card, read.rows, read.scope)
	const wrong = Object.entries(derived.checks)
		.filter(([, result]) => result === "mismatch")
		.map(([field]) => field)

	// An `entities` filter's promises, on this entity's own cards.
	const unmet: FilterClaim[] = []
	for (const claim of options.claims ?? []) {
		const wanted = claim.kind === "type" ? TYPES_PROPERTY : conformingId("entity", claim.typeId)
		const targets = new Set(
			claim.kind === "type"
				? claim.typeIds.map((id) => conformingId("entity", id))
				: [conformingId("entity", claim.toEntityId)],
		)
		if (!read.rows.some((row) => row.prop === wanted && targets.has(row.raw))) unmet.push(claim)
	}
	const outsideSpace = options.spaceIds?.length
		? !options.spaceIds.some((space) => (read.bySpace.get(space)?.length ?? 0) > 0)
		: false
	const bad = missing.length > 0 || wrong.length > 0 || unmet.length > 0 || outsideSpace
	return {
		result: bad ? "mismatch" : "match",
		uncompared: derived.uncompared,
		detail: {
			entityId,
			spaces: spaces.length,
			missingInGeoDb: missing.length,
			sampleMissing: missing.slice(0, 5),
			skipped,
			derivedMismatches: wrong,
			unmetFilter: unmet.slice(0, 3),
			notInSpace: outsideSpace,
			geoDbMs: read.ms,
		},
	}
}

/** Entities of an `entities` answer to compare: an even spread over the answer, at most the sample size. */
export function sampleNodes(nodes: unknown[]): unknown[] {
	if (nodes.length <= MAX_SAMPLED_ENTITIES) return nodes
	const picks = new Set<number>()
	for (let i = 0; i < MAX_SAMPLED_ENTITIES; i++)
		picks.add(Math.floor((i * (nodes.length - 1)) / (MAX_SAMPLED_ENTITIES - 1)))
	return [...picks].map((at) => nodes[at])
}

/** What one light operation came to; the caller counts it under its own root field. */
export type LightOutcome =
	| {result: "skip"}
	| {result: "failed"}
	| {result: "unsupported"; reason: UnsupportedReason}
	| {result: "match" | "mismatch"; uncompared: UncomparedReason[]; detail: Record<string, unknown>}

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
	const answer = data[plan.key]
	const entities: {id: string; node: unknown}[] = []
	let filterBreach: Record<string, unknown> | null = null
	if (plan.kind === "entity") {
		if (answer === null) return {result: "skip"}
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
		// An `id: {in}` filter is exact: PostGraphile may return only the ids it was asked for.
		if (plan.ids) {
			const asked = new Set(plan.ids)
			const stray = ids.filter((id) => !asked.has(id))
			if (stray.length > 0) filterBreach = {strayIds: stray.slice(0, 3)}
		}
		for (const node of sampleNodes(answer)) {
			const id = asRecord(node)?.[idKey]
			entities.push({id: normalizeUuid(String(id)), node})
		}
	}
	const checks: {id: string; check: LightCheck}[] = []
	for (const {id, node} of entities) {
		const light = lightCardFromEntity(node, plan)
		if (!light) return {result: "skip"}
		const options = plan.kind === "entities" ? {claims: plan.claims, spaceIds: plan.spaceIds} : {}
		checks.push({id, check: await checkLight(deps, plan, light, id, label, options)})
	}
	const mismatched = checks.filter((c) => c.check.result === "mismatch")
	if (mismatched.length > 0 || filterBreach) {
		return {
			result: "mismatch",
			uncompared: plan.uncompared,
			detail: {
				entities: checks.length,
				mismatches: mismatched.length,
				filterBreach,
				first: mismatched[0]?.check.result === "mismatch" ? mismatched[0].check.detail : undefined,
			},
		}
	}
	const unsupported = checks.find((c) => c.check.result === "unsupported")?.check
	if (unsupported?.result === "unsupported") return {result: "unsupported", reason: unsupported.reason}
	if (checks.some((c) => c.check.result === "failed")) return {result: "failed"}
	const uncompared = new Set<UncomparedReason>(plan.uncompared)
	for (const {check} of checks) if (check.result === "match") for (const r of check.uncompared) uncompared.add(r)
	return {result: "match", uncompared: [...uncompared], detail: {entities: checks.length}}
}
