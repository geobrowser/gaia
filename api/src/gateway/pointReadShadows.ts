/**
 * Shadows of the point reads that follow `entity(id)`: `entities` by an id list, `relation(id)` and `relations`
 * by an id list. PostGraphile answers, geo-db is asked in the background, the answers are compared (see
 * entityShadow.ts for the card comparison these reuse).
 *
 * geo-db answers a card: one entity's values and outgoing relations in one space. It cannot look an entity or a
 * relation up without a space, so only selections a card can settle are compared; the rest count as
 * `unsupported_selection` with a reason (metrics.ts):
 *
 *   entities(filter: {id: {in: [<uuid> ..]}}, first: N) { id <entity card selection> }
 *     Each requested id is compared as `entity(id)` is: the selection must be the card selection (both
 *     space-filtered lists, `id` selected). An id PostGraphile does not return is an empty card. More than
 *     MAX_BATCH_IDS ids, a filter other than `id in`, or any other argument is unsupported.
 *
 *   relation(id: <uuid>) { id spaceId fromEntityId typeId toEntityId .. entity { <entity card selection> } }
 *     The relation's edge is checked against geo-db's card of its from-entity in the relation's space: a
 *     (type, target) statement must exist. That needs the space, from entity, type and target in the answer
 *     (a column or a nested `{id}`); without them the edge is uncompared (`relation_edge`). `entity { .. }` is
 *     compared as a card when it is the card selection, else only its names are selected and `nested_names`
 *     is uncompared. A selection with neither check is unsupported. The relation's own `type`, `toEntity` and
 *     `fromEntity` objects are read for their ids only; their other fields are uncompared.
 *
 *   relations(filter: {<id|entityId|fromEntityId|toEntityId|typeId|spaceId>: {is | equalTo | in}, ..},
 *             first, offset, orderBy) { same }
 *     A column the filter pins to one value need not be selected for the edge check. One from entity's relations
 *     of some types in one space (no offset, a page short of `first`) are a slice of its card and compared whole,
 *     both ways. Any other filter checks the answer's rows (an even spread of MAX_BATCH_IDS, `sampled_rows`)
 *     and leaves what Postgres left out uncompared (`filter_completeness`). An id list is read the same way.
 *
 * The entity page's `relations(filter: {entityId, spaceId})` co-root beside `entity` is the `entity` shadow's
 * (`root_relations`).
 *
 * One result is counted per operation: `mismatch` if any compared item differs, else `failed` if any read
 * failed, else `match`.
 */

import {type FieldNode, Kind, type SelectionSetNode, valueFromASTUntyped} from "graphql"

import {
	conformingId,
	DERIVED_PROPERTIES,
	diffCards,
	type GeoCardRow,
	isEmptyDiff,
	statementsFromPostgres,
} from "../geodb/card"
import {GeoDbError} from "../geodb/client"
import {askGeoDb} from "../geodb/service"
import {defaultLimitForDepth} from "../kg/paginationCapPlugin"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import {
	asRecord,
	type CardCheck,
	cardFromEntity,
	checkCard,
	countLight,
	type EntityCardPlan,
	type EntityShadowDeps,
	parseOperation,
	planEntitySelection,
	Unsupported,
} from "./entityShadow"
import type {FieldShadow} from "./gateway"
import {analyzeLightEntities, noteUnsupported, runLight} from "./lightShadow"
import {
	activateGatewayMetrics,
	recordGatewayShadow,
	recordGatewayUncompared,
	recordGatewayUnsupported,
	type UncomparedReason,
	type UnsupportedReason,
} from "./metrics"

export const ENTITIES_FIELD = "entities"
export const RELATION_FIELD = "relation"
export const RELATIONS_FIELD = "relations"
export const POINT_READ_FIELDS = [ENTITIES_FIELD, RELATION_FIELD, RELATIONS_FIELD] as const

/** Most ids (or relation rows) one operation may ask geo-db about; a larger batch is not shadowed. */
export const MAX_BATCH_IDS = 20

const refuse = (reason: UnsupportedReason): never => {
	throw new Unsupported(reason)
}

// ---------------------------------------------------------------------------------------------------------
// Selection analysis
// ---------------------------------------------------------------------------------------------------------

type Args = Record<string, unknown>
const isPlain = (f: FieldNode) => !f.arguments?.length && !f.selectionSet
const keyOf = (f: FieldNode) => f.alias?.value ?? f.name.value

function inlineFields(set: SelectionSetNode, fragments: Map<string, SelectionSetNode>, depth = 0): FieldNode[] {
	if (depth > 8) return refuse("selection")
	const out: FieldNode[] = []
	for (const selection of set.selections) {
		if (selection.directives?.length) refuse("directives")
		if (selection.kind === Kind.FIELD) out.push(selection)
		else if (selection.kind === Kind.INLINE_FRAGMENT)
			out.push(...inlineFields(selection.selectionSet, fragments, depth + 1))
		else {
			const fragment = fragments.get(selection.name.value)
			if (!fragment) refuse("selection")
			else out.push(...inlineFields(fragment, fragments, depth + 1))
		}
	}
	return out
}

/** A root field's arguments with variables resolved. */
function rootArgs(field: FieldNode, variables: Args): Args {
	const out: Args = {}
	for (const arg of field.arguments ?? []) out[arg.name.value] = valueFromASTUntyped(arg.value, variables)
	return out
}

/** `{id: {in: [<uuid> ..]}}` and nothing else; the ids, normalized and distinct. `limit`: most ids accepted. */
function idsFromFilter(filter: unknown, limit = MAX_BATCH_IDS): string[] {
	const object = asRecord(filter)
	if (!object || Object.keys(object).length !== 1 || !("id" in object)) return refuse("filter")
	const clause = asRecord(object.id)
	if (!clause || Object.keys(clause).length !== 1 || !Array.isArray(clause.in)) return refuse("filter")
	const ids = clause.in.map((id) => (typeof id === "string" && isValidUuid(id) ? normalizeUuid(id) : refuse("args")))
	const distinct = [...new Set(ids)]
	if (distinct.length === 0) return refuse("filter")
	if (distinct.length > limit) return refuse("batch_size")
	return distinct
}

/** `count` of `items`, evenly spread from first to last. */
export function spread<T>(items: T[], count: number): T[] {
	if (items.length <= count) return items
	const picks = new Set<number>()
	for (let i = 0; i < count; i++) picks.add(Math.floor((i * (items.length - 1)) / (count - 1)))
	return [...picks].map((at) => items[at] as T)
}

/** `first` alone beside `filter`; it must not cut the id list short. */
function checkListArgs(args: Args, ids: string[]): void {
	for (const name of Object.keys(args)) if (name !== "filter" && name !== "first") refuse("args")
	if (args.first !== undefined && args.first !== null) {
		if (typeof args.first !== "number" || !Number.isInteger(args.first) || args.first < ids.length) refuse("args")
	}
}

/** Where a relation's id column sits in a node: a plain column, or the `id` of a nested object. */
type IdPath = {object: string | null; key: string}

const EDGE_COLUMNS = ["spaceId", "fromEntityId", "typeId", "toEntityId"] as const
type EdgeColumn = (typeof EDGE_COLUMNS)[number]

/** A `relations` filter: the uuid columns it constrains, each to one value (`is`, `equalTo`) or a list (`in`). */
export type RelationFilter = Partial<Record<(typeof RELATION_COLUMNS)[number], string[]>>

/**
 * A `relations(filter: ..)` that is not an id list. Every answer row is checked as an edge (an even spread of
 * MAX_BATCH_IDS rows when there are more), and geo-db cannot say what Postgres left out (`filter_completeness`),
 * except when the filter is one from entity's relations of some types in one space: that is a slice of its card,
 * compared whole unless the page may have cut it short.
 */
export type RelationListPlan = {
	filter: RelationFilter
	/** Rows PostGraphile returns at most (`first`, else its default page). */
	limit: number
	/** One from entity, one space, the types: the answer is compared as that slice of the card. */
	slice: {fromEntityId: string; spaceId: string; typeIds: string[]} | null
}

export type RelationPlan = {
	/** Edge columns the filter pins to one value: a row need not select them. */
	fixed: Partial<Record<EdgeColumn, string>>
	idKey: string | null
	spaceId: string | null
	fromEntityId: IdPath | null
	typeId: IdPath | null
	toEntityId: IdPath | null
	entityIdKey: string | null
	/** The relation's own entity, when it is selected as a card. */
	entity: {key: string; card: EntityCardPlan} | null
	uncompared: UncomparedReason[]
}

const RELATION_COLUMNS = ["id", "spaceId", "entityId", "fromEntityId", "toEntityId", "typeId"] as const
const RELATION_IGNORED = ["toSpaceId", "position", "verified", "__typename"] as const
const RELATION_OBJECTS = {type: "typeId", toEntity: "toEntityId", fromEntity: "fromEntityId"} as const

function planRelation(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	asCard: (set: SelectionSetNode, uncompared: Set<UncomparedReason>) => EntityCardPlan,
): RelationPlan {
	const uncompared = new Set<UncomparedReason>()
	const columns: Partial<Record<(typeof RELATION_COLUMNS)[number], string>> = {}
	const paths: Partial<Record<"fromEntityId" | "typeId" | "toEntityId", IdPath>> = {}
	let entity: RelationPlan["entity"] = null
	const seen = new Set<string>()
	for (const field of inlineFields(set, fragments)) {
		const key = keyOf(field)
		if (seen.has(key)) return refuse("selection")
		seen.add(key)
		const name = field.name.value
		if ((RELATION_COLUMNS as readonly string[]).includes(name)) {
			if (!isPlain(field)) return refuse(field.selectionSet ? "selection" : "args")
			columns[name as (typeof RELATION_COLUMNS)[number]] ??= key
		} else if ((RELATION_IGNORED as readonly string[]).includes(name)) {
			if (!isPlain(field)) return refuse(field.selectionSet ? "selection" : "args")
		} else if (name in RELATION_OBJECTS) {
			if (!field.selectionSet || field.arguments?.length) return refuse("selection")
			const column = RELATION_OBJECTS[name as keyof typeof RELATION_OBJECTS]
			for (const inner of inlineFields(field.selectionSet, fragments)) {
				if (inner.name.value === "id" && isPlain(inner)) paths[column] ??= {object: key, key: keyOf(inner)}
				else if (inner.name.value === "name") uncompared.add("nested_names")
				else if (inner.name.value !== "__typename") uncompared.add("nested_entity")
			}
		} else if (name === "entity" && !entity) {
			if (!field.selectionSet || field.arguments?.length) return refuse("selection")
			try {
				entity = {key, card: asCard(field.selectionSet, uncompared)}
			} catch (error) {
				// Only its id and name: the name is not compared.
				if (!(error instanceof Unsupported)) throw error
				const inner = inlineFields(field.selectionSet, fragments)
				const namesOnly = inner.every((f) => ["id", "name", "__typename"].includes(f.name.value) && isPlain(f))
				if (!namesOnly) throw error
				if (inner.some((f) => f.name.value === "name")) uncompared.add("nested_names")
			}
		} else return refuse("selection")
	}
	const direct = (column: "fromEntityId" | "typeId" | "toEntityId"): IdPath | null => {
		const key = columns[column]
		return key ? {object: null, key} : (paths[column] ?? null)
	}
	return {
		fixed: {},
		idKey: columns.id ?? null,
		spaceId: columns.spaceId ?? null,
		fromEntityId: direct("fromEntityId"),
		typeId: direct("typeId"),
		toEntityId: direct("toEntityId"),
		entityIdKey: columns.entityId ?? null,
		entity,
		uncompared: [...uncompared],
	}
}

const edgeCheckable = (plan: RelationPlan) =>
	(plan.spaceId !== null || plan.fixed.spaceId !== undefined) &&
	(plan.fromEntityId !== null || plan.fixed.fromEntityId !== undefined) &&
	(plan.typeId !== null || plan.fixed.typeId !== undefined) &&
	(plan.toEntityId !== null || plan.fixed.toEntityId !== undefined)

const uuidOrRefuse = (v: unknown): string =>
	typeof v === "string" && isValidUuid(v) ? normalizeUuid(v) : refuse("filter")

/**
 * `{<column>: {is | equalTo: <uuid>} | {in: [<uuid> ..]}, ..}` over the relation's uuid columns. `{is: null}` is no
 * constraint (connection-filter ignores it). Anything else, or no constraint at all, is unsupported (`filter`).
 */
export function relationFilter(filter: unknown): RelationFilter {
	const object = asRecord(filter)
	if (!object) return refuse("filter")
	const out: RelationFilter = {}
	for (const [column, raw] of Object.entries(object)) {
		if (!(RELATION_COLUMNS as readonly string[]).includes(column)) return refuse("filter")
		const clause = asRecord(raw)
		if (!clause || Object.keys(clause).length !== 1) return refuse("filter")
		const [op, value] = Object.entries(clause)[0] as [string, unknown]
		let values: string[]
		if (op === "is" || op === "equalTo") {
			if (value === null) continue
			values = [uuidOrRefuse(value)]
		} else if (op === "in" && Array.isArray(value) && value.length > 0)
			values = [...new Set(value.map(uuidOrRefuse))]
		else return refuse("filter")
		out[column as (typeof RELATION_COLUMNS)[number]] = values
	}
	if (Object.keys(out).length === 0) return refuse("filter")
	return out
}

const LIST_ARGS = new Set(["filter", "first", "offset", "orderBy"])

/** `relations(filter, first, offset, orderBy)` with a filter other than an id list. */
function planRelationList(args: Args, filter: RelationFilter): RelationListPlan {
	for (const name of Object.keys(args)) if (!LIST_ARGS.has(name)) refuse("args")
	const count = (v: unknown) =>
		v === undefined || v === null
			? null
			: typeof v === "number" && Number.isInteger(v) && v >= 0
				? v
				: refuse("args")
	const first = count(args.first)
	const offset = count(args.offset) ?? 0
	const limit = first ?? defaultLimitForDepth(0)
	const one = (column: (typeof RELATION_COLUMNS)[number]) => {
		const values = filter[column]
		return values?.length === 1 ? (values[0] as string) : null
	}
	const fromEntityId = one("fromEntityId")
	const spaceId = one("spaceId")
	const typeIds = filter.typeId ?? []
	const slice =
		fromEntityId &&
		spaceId &&
		typeIds.length > 0 &&
		offset === 0 &&
		!filter.id &&
		!filter.entityId &&
		!filter.toEntityId
			? {fromEntityId, spaceId, typeIds}
			: null
	return {filter, limit, slice}
}

/** A relation selection must be checkable in at least one way, else it has nothing geo-db can settle. */
function finishRelationPlan(plan: RelationPlan): RelationPlan {
	if (!edgeCheckable(plan) && !plan.entity) return refuse("selection")
	if (!edgeCheckable(plan) && !plan.uncompared.includes("relation_edge"))
		return {...plan, uncompared: [...plan.uncompared, "relation_edge"]}
	return plan
}

export type PointPlan =
	| {field: typeof ENTITIES_FIELD; key: string; ids: string[]; card: EntityCardPlan; uncompared: UncomparedReason[]}
	| {field: typeof RELATION_FIELD; key: string; id: string; relation: RelationPlan}
	| {field: typeof RELATIONS_FIELD; key: string; ids: string[]; list: RelationListPlan | null; relation: RelationPlan}

export type PointAnalysis = {ok: true; plan: PointPlan} | {ok: false; reason: UnsupportedReason}

/** Decides whether an operation is a point read the shadow of `field` can settle. */
export function analyzePointRead(
	field: (typeof POINT_READ_FIELDS)[number],
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Args,
): PointAnalysis {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const root = roots[0]
		if (roots.length !== 1 || !root || root.name.value !== field) return {ok: false, reason: "not_single_root"}
		if (!root.selectionSet) return {ok: false, reason: "selection"}
		const args = rootArgs(root, resolved)
		const card = (set: SelectionSetNode, uncompared: Set<UncomparedReason>) =>
			planEntitySelection(set, fragments, resolved, uncompared)
		if (field === RELATION_FIELD) {
			if (Object.keys(args).length !== 1 || !("id" in args)) return {ok: false, reason: "args"}
			const id = typeof args.id === "string" && isValidUuid(args.id) ? normalizeUuid(args.id) : refuse("args")
			const relation = finishRelationPlan(planRelation(root.selectionSet, fragments, card))
			return {ok: true, plan: {field, key: keyOf(root), id, relation}}
		}
		if (field === RELATIONS_FIELD) {
			const filter = relationFilter(args.filter)
			const onlyIds = Object.keys(filter).length === 1 && filter.id !== undefined
			const list = onlyIds ? null : planRelationList(args, filter)
			if (!list) checkListArgs(args, filter.id ?? [])
			const planned = planRelation(root.selectionSet, fragments, card)
			const fixed: RelationPlan["fixed"] = {}
			for (const column of EDGE_COLUMNS) {
				const values = filter[column]
				if (values?.length === 1) fixed[column] = values[0]
			}
			const relation = finishRelationPlan({...planned, fixed})
			// The answer of a filtered list is checked row by row: geo-db cannot say what Postgres left out.
			if (list && !list.slice)
				relation.uncompared = [...new Set([...relation.uncompared, "filter_completeness" as const])]
			return {ok: true, plan: {field, key: keyOf(root), ids: filter.id ?? [], list, relation}}
		}
		// A longer `entities` id list is compared on an even spread of MAX_BATCH_IDS of its ids.
		const ids = idsFromFilter(args.filter, Number.POSITIVE_INFINITY)
		checkListArgs(args, ids)
		const uncompared = new Set<UncomparedReason>()
		const cardPlan = planEntitySelection(root.selectionSet, fragments, resolved, uncompared)
		// Each answer is matched to its requested id by the id the node carries.
		if (!cardPlan.idKey) return {ok: false, reason: "selection"}
		const compared = ids.length > MAX_BATCH_IDS ? spread(ids, MAX_BATCH_IDS) : ids
		if (compared !== ids) uncompared.add("sampled_rows")
		return {ok: true, plan: {field, key: keyOf(root), ids: compared, card: cardPlan, uncompared: [...uncompared]}}
	} catch (error) {
		if (error instanceof Unsupported) return {ok: false, reason: error.reason}
		return {ok: false, reason: "selection"}
	}
}

// ---------------------------------------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------------------------------------

type Item =
	| {kind: "card"; entityId: string; plan: EntityCardPlan; card: NonNullable<ReturnType<typeof cardFromEntity>>}
	| {kind: "edge"; fromEntityId: string; spaceId: string; typeId: string; toEntityId: string}
	/** Every relation of `typeIds` from one entity in one space, as Postgres answered them. */
	| {
			kind: "slice"
			fromEntityId: string
			spaceId: string
			typeIds: string[]
			edges: {typeId: string; toEntityId: string}[]
	  }

const pathValue = (node: Record<string, unknown>, path: IdPath | null): string | null => {
	if (!path) return null
	const v = path.object ? asRecord(node[path.object])?.[path.key] : node[path.key]
	return typeof v === "string" && isValidUuid(v) ? normalizeUuid(v) : null
}

/**
 * The comparisons one relation node asks for. Null when the node is not a plain answer for the plan (nothing is
 * counted then). `truncated` when a card list filled its page.
 */
function itemsOfRelation(raw: unknown, plan: RelationPlan): {items: Item[]; truncated: boolean} | null {
	const node = asRecord(raw)
	if (!node) return null
	const items: Item[] = []
	let truncated = false
	if (edgeCheckable(plan)) {
		// A selected column wins over the filter's value; they agree on any row the filter returned.
		const column = (c: EdgeColumn) =>
			pathValue(node, c === "spaceId" ? (plan.spaceId ? {object: null, key: plan.spaceId} : null) : plan[c]) ??
			plan.fixed[c] ??
			null
		const spaceId = column("spaceId")
		const fromEntityId = column("fromEntityId")
		const typeId = column("typeId")
		const toEntityId = column("toEntityId")
		if (!spaceId || !fromEntityId || !typeId || !toEntityId) return null
		items.push({kind: "edge", fromEntityId, spaceId, typeId, toEntityId})
	}
	if (plan.entity) {
		const entity = node[plan.entity.key]
		const card = cardFromEntity(entity === undefined ? null : entity, plan.entity.card)
		if (!card) return null
		const own = asRecord(entity)
		const idKey = plan.entity.card.idKey
		const fromEntityNode = idKey && own ? own[idKey] : null
		const fromColumn = plan.entityIdKey ? node[plan.entityIdKey] : null
		const candidate = typeof fromEntityNode === "string" ? fromEntityNode : fromColumn
		if (typeof candidate !== "string" || !isValidUuid(candidate)) return null
		if (card.truncated) truncated = true
		else items.push({kind: "card", entityId: normalizeUuid(candidate), plan: plan.entity.card, card})
	}
	return {items, truncated}
}

type Judged = {
	result: "match" | "mismatch" | "failed" | "unsupported"
	uncompared: UncomparedReason[]
	detail?: Record<string, unknown>
	/** Why geo-db was not asked, for `unsupported`. */
	reason?: UnsupportedReason
}

/** One operation's card reads, by entity and space: many rows of one answer share a from entity. */
type CardReads = Map<string, Promise<GeoCardRow[]>>

function readOnce(deps: EntityShadowDeps, reads: CardReads, entityId: string, spaceId: string) {
	const key = `${entityId}\t${spaceId}`
	let read = reads.get(key)
	if (!read) {
		read = askGeoDb(deps, conformingId("entity", entityId), spaceId)
		reads.set(key, read)
	}
	return read
}

async function judgeSlice(
	deps: EntityShadowDeps,
	item: Extract<Item, {kind: "slice"}>,
	reads: CardReads,
): Promise<Judged> {
	const types = new Set(item.typeIds.map((id) => conformingId("entity", id)))
	// A derived relation type (ranking position) is written straight to Postgres and never held by geo-db.
	if ([...types].every((type) => DERIVED_PROPERTIES.has(type)))
		return {result: "match", uncompared: ["relation_edge"]}
	const rows = await readOnce(deps, reads, item.fromEntityId, item.spaceId)
	const diff = diffCards(
		statementsFromPostgres(
			[],
			item.edges.map((e) => ({type_id: e.typeId, to_entity_id: e.toEntityId, to_space_id: null})),
		),
		rows.filter((row) => types.has(row.prop)),
	)
	if (isEmptyDiff(diff)) return {result: "match", uncompared: []}
	return {
		result: "mismatch",
		uncompared: [],
		detail: {
			slice: {fromEntityId: item.fromEntityId, spaceId: item.spaceId, typeIds: item.typeIds},
			postgresRows: item.edges.length,
			onlyPostgres: diff.onlyPostgres.slice(0, 5),
			onlyGeoDb: diff.onlyGeoDb.slice(0, 5),
		},
	}
}

async function judge(deps: EntityShadowDeps, item: Item, label: string, reads: CardReads = new Map()): Promise<Judged> {
	if (item.kind === "card") {
		const check: CardCheck = await checkCard(deps, item.plan, item.card, item.entityId, label)
		if (check.result === "failed") return {result: "failed", uncompared: []}
		if (check.result === "unsupported") return {result: "unsupported", uncompared: [], reason: check.reason}
		return {result: check.result, uncompared: check.uncompared, detail: check.detail}
	}
	if (item.kind === "slice") {
		try {
			return await judgeSlice(deps, item, reads)
		} catch (error) {
			deps.log.warn(`${label} read failed`, {
				entityId: item.fromEntityId,
				spaceId: item.spaceId,
				reason: error instanceof GeoDbError ? error.reason : "error",
				message: String(error),
			})
			return {result: "failed", uncompared: []}
		}
	}
	// A derived relation type (ranking position) is written straight to Postgres and never held by geo-db.
	if (DERIVED_PROPERTIES.has(conformingId("entity", item.typeId)))
		return {result: "match", uncompared: ["relation_edge"]}
	try {
		const rows = await readOnce(deps, reads, item.fromEntityId, item.spaceId)
		const type = conformingId("entity", item.typeId)
		const to = conformingId("entity", item.toEntityId)
		const found = rows.some((row) => row.prop === type && row.raw === to)
		return found
			? {result: "match", uncompared: []}
			: {
					result: "mismatch",
					uncompared: [],
					detail: {
						edge: {
							fromEntityId: item.fromEntityId,
							spaceId: item.spaceId,
							typeId: item.typeId,
							toEntityId: item.toEntityId,
						},
						geoDbRows: rows.length,
					},
				}
	} catch (error) {
		deps.log.warn(`${label} read failed`, {
			entityId: item.fromEntityId,
			spaceId: item.spaceId,
			reason: error instanceof GeoDbError ? error.reason : "error",
			message: String(error),
		})
		return {result: "failed", uncompared: []}
	}
}

/** The items to compare for an operation, from PostGraphile's body. Null when the body is not comparable. */
function collectItems(
	plan: PointPlan,
	body: unknown,
): {items: Item[]; truncated: boolean; uncompared?: UncomparedReason[]} | null {
	const root = asRecord(body)
	if (!root || root.errors !== undefined) return null
	const data = asRecord(root.data)
	if (!data || !(plan.key in data)) return null
	const answer = data[plan.key]
	if (plan.field === RELATION_FIELD) {
		// geo-db cannot look a relation up by id: a relation PostGraphile lacks has nothing to compare.
		return answer === null ? null : itemsOfRelation(answer, plan.relation)
	}
	if (!Array.isArray(answer)) return null
	if (plan.field === RELATIONS_FIELD) {
		const rows: Item[][] = []
		let truncated = false
		for (const node of answer) {
			const one = itemsOfRelation(node, plan.relation)
			if (!one) return null
			rows.push(one.items)
			truncated ||= one.truncated
		}
		const slice = plan.list?.slice
		// A full page may have been cut short: only what it holds is checked.
		if (slice && edgeCheckable(plan.relation) && answer.length < (plan.list?.limit ?? 0)) {
			const edges: {typeId: string; toEntityId: string}[] = []
			const cards: Item[] = []
			for (const item of rows.flat()) {
				if (item.kind === "edge") edges.push({typeId: item.typeId, toEntityId: item.toEntityId})
				else cards.push(item)
			}
			// Each row must have been read as an edge, else the slice is not what Postgres holds.
			if (edges.length !== answer.length) return null
			const sliceItem: Item = {kind: "slice", ...slice, edges}
			return {items: [sliceItem, ...spread(cards, MAX_BATCH_IDS)], truncated, uncompared: []}
		}
		const sampled = spread(rows, MAX_BATCH_IDS)
		const uncompared: UncomparedReason[] = []
		if (sampled.length < rows.length) uncompared.push("sampled_rows")
		if (slice) uncompared.push("filter_completeness")
		return {items: sampled.flat(), truncated, uncompared}
	}
	const idKey = plan.card.idKey ?? ""
	const byId = new Map<string, unknown>()
	for (const node of answer) {
		const id = asRecord(node)?.[idKey]
		if (typeof id !== "string" || !isValidUuid(id)) return null
		byId.set(normalizeUuid(id), node)
	}
	const items: Item[] = []
	let truncated = false
	for (const id of plan.ids) {
		const card = cardFromEntity(byId.get(id) ?? null, plan.card)
		if (!card) return null
		if (card.truncated) truncated = true
		else items.push({kind: "card", entityId: id, plan: plan.card, card})
	}
	return {items, truncated}
}

// ---------------------------------------------------------------------------------------------------------
// The shadows
// ---------------------------------------------------------------------------------------------------------

function createPointReadShadow(deps: EntityShadowDeps, field: (typeof POINT_READ_FIELDS)[number]): FieldShadow {
	activateGatewayMetrics(field)
	const label = `gateway ${field} shadow`
	return async ({params, document, classification, response}) => {
		try {
			const analysis = analyzePointRead(field, document, classification.operationName, params.variables)
			if (!analysis.ok) {
				// A card-shaped selection is not what was asked: `entities` also has a light form (lightShadow.ts).
				const light =
					field === ENTITIES_FIELD
						? analyzeLightEntities(document, classification.operationName, params.variables, {
								entitiesBatch: deps.entitiesBatch === true,
								relationsTypeFilter: deps.relationsTypeFilter === true,
							})
						: null
				if (light?.ok) {
					let lightBody: unknown
					try {
						lightBody = await response.json()
					} catch {
						return
					}
					countLight(deps, field, await runLight(deps, light.plan, lightBody, label), {
						operationName: classification.operationName,
					})
					return
				}
				// The card analysis' args/filter/batch_size only say "not an id list"; the light form's reason is the news.
				const generic = ["args", "filter", "batch_size"].includes(analysis.reason)
				const reason = generic && light && !light.ok ? light.reason : analysis.reason
				recordGatewayShadow(field, "unsupported_selection")
				recordGatewayUnsupported(field, reason)
				noteUnsupported(deps, field, reason, document, classification.operationName, params.variables)
				return
			}
			const {plan} = analysis
			let body: unknown
			try {
				body = await response.json()
			} catch {
				return
			}
			const collected = collectItems(plan, body)
			if (!collected) {
				deps.log.info(`${label} skipped: PostGraphile answer not comparable`, {})
				return
			}
			if (collected.truncated) {
				recordGatewayShadow(field, "unsupported_selection")
				recordGatewayUnsupported(field, "truncated")
				return
			}
			if (collected.items.length === 0) return
			const reads: CardReads = new Map()
			const judged = await Promise.all(collected.items.map((item) => judge(deps, item, label, reads)))
			const mismatched = judged.filter((j) => j.result === "mismatch")
			const planned =
				plan.field === RELATION_FIELD || plan.field === RELATIONS_FIELD
					? plan.relation.uncompared
					: plan.uncompared
			const unsupported = judged.find((j) => j.result === "unsupported")
			if (mismatched.length === 0 && unsupported?.reason) {
				// Part of the operation was never compared (the entity's spaces, the budget): not a match.
				recordGatewayShadow(field, "unsupported_selection")
				recordGatewayUnsupported(field, unsupported.reason)
				return
			}
			const result =
				mismatched.length > 0 ? "mismatch" : judged.some((j) => j.result === "failed") ? "failed" : "match"
			if (result !== "failed")
				for (const reason of new Set([
					...planned,
					...(collected.uncompared ?? []),
					...judged.flatMap((j) => j.uncompared),
				]))
					recordGatewayUncompared(field, reason)
			recordGatewayShadow(field, result)
			if (result === "mismatch")
				deps.log.warn(`${label} mismatch`, {
					operationName: classification.operationName,
					items: judged.length,
					mismatches: mismatched.length,
					first: mismatched[0]?.detail,
				})
		} catch (error) {
			// The response is already gone; count it and move on.
			try {
				recordGatewayShadow(field, "failed")
				deps.log.warn(`${label} errored`, {message: String(error)})
			} catch {}
		}
	}
}

/** The shadows for the given point-read root fields, by field. Registers only their metric series. */
export function createPointReadShadows(
	deps: EntityShadowDeps,
	fields: Iterable<(typeof POINT_READ_FIELDS)[number]> = POINT_READ_FIELDS,
): Map<string, FieldShadow> {
	return new Map([...fields].map((field) => [field, createPointReadShadow(deps, field)]))
}
