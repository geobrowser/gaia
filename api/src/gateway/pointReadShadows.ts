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
 *   relations(filter: {id: {in: [..]}}, first: N) { same }
 *     The relation's edge is checked against geo-db's card of its from-entity in the relation's space: a
 *     (type, target) statement must exist. That needs the space, from entity, type and target in the answer
 *     (a column or a nested `{id}`); without them the edge is uncompared (`relation_edge`). `entity { .. }` is
 *     compared as a card when it is the card selection, else only its names are selected and `nested_names`
 *     is uncompared. A selection with neither check is unsupported. The relation's own `type`, `toEntity` and
 *     `fromEntity` objects are read for their ids only; their other fields are uncompared.
 *
 * `relations(filter: {entityId, spaceId})` (the entity page's co-root) is unsupported here (`filter`): the
 * `entity` shadow already tolerates it as `root_relations`.
 *
 * One result is counted per operation: `mismatch` if any compared item differs, else `failed` if any read
 * failed, else `match`.
 */

import {type FieldNode, Kind, type SelectionSetNode, valueFromASTUntyped} from "graphql"

import {conformingId, DERIVED_PROPERTIES} from "../geodb/card"
import {GeoDbError} from "../geodb/client"
import {askGeoDb} from "../geodb/service"
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
import {analyzeLightEntities, runLight} from "./lightShadow"
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

/** `{id: {in: [<uuid> ..]}}` and nothing else; the ids, normalized and distinct. */
function idsFromFilter(filter: unknown): string[] {
	const object = asRecord(filter)
	if (!object || Object.keys(object).length !== 1 || !("id" in object)) return refuse("filter")
	const clause = asRecord(object.id)
	if (!clause || Object.keys(clause).length !== 1 || !Array.isArray(clause.in)) return refuse("filter")
	const ids = clause.in.map((id) => (typeof id === "string" && isValidUuid(id) ? normalizeUuid(id) : refuse("args")))
	const distinct = [...new Set(ids)]
	if (distinct.length === 0) return refuse("filter")
	if (distinct.length > MAX_BATCH_IDS) return refuse("batch_size")
	return distinct
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

export type RelationPlan = {
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
	plan.spaceId !== null && plan.fromEntityId !== null && plan.typeId !== null && plan.toEntityId !== null

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
	| {field: typeof RELATIONS_FIELD; key: string; ids: string[]; relation: RelationPlan}

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
		const ids = idsFromFilter(args.filter)
		checkListArgs(args, ids)
		if (field === RELATIONS_FIELD) {
			const relation = finishRelationPlan(planRelation(root.selectionSet, fragments, card))
			return {ok: true, plan: {field, key: keyOf(root), ids, relation}}
		}
		const uncompared = new Set<UncomparedReason>()
		const cardPlan = planEntitySelection(root.selectionSet, fragments, resolved, uncompared)
		// Each answer is matched to its requested id by the id the node carries.
		if (!cardPlan.idKey) return {ok: false, reason: "selection"}
		return {ok: true, plan: {field, key: keyOf(root), ids, card: cardPlan, uncompared: [...uncompared]}}
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
		const spaceId = typeof node[plan.spaceId ?? ""] === "string" ? String(node[plan.spaceId ?? ""]) : null
		const fromEntityId = pathValue(node, plan.fromEntityId)
		const typeId = pathValue(node, plan.typeId)
		const toEntityId = pathValue(node, plan.toEntityId)
		if (!spaceId || !isValidUuid(spaceId) || !fromEntityId || !typeId || !toEntityId) return null
		items.push({kind: "edge", fromEntityId, spaceId: normalizeUuid(spaceId), typeId, toEntityId})
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

async function judge(deps: EntityShadowDeps, item: Item, label: string): Promise<Judged> {
	if (item.kind === "card") {
		const check: CardCheck = await checkCard(deps, item.plan, item.card, item.entityId, label)
		if (check.result === "failed") return {result: "failed", uncompared: []}
		if (check.result === "unsupported") return {result: "unsupported", uncompared: [], reason: check.reason}
		return {result: check.result, uncompared: check.uncompared, detail: check.detail}
	}
	// A derived relation type (ranking position) is written straight to Postgres and never held by geo-db.
	if (DERIVED_PROPERTIES.has(conformingId("entity", item.typeId)))
		return {result: "match", uncompared: ["relation_edge"]}
	try {
		const rows = await askGeoDb(deps, conformingId("entity", item.fromEntityId), item.spaceId)
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
function collectItems(plan: PointPlan, body: unknown): {items: Item[]; truncated: boolean} | null {
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
		if (answer.length > MAX_BATCH_IDS) return null
		const all: Item[] = []
		let truncated = false
		for (const node of answer) {
			const one = itemsOfRelation(node, plan.relation)
			if (!one) return null
			all.push(...one.items)
			truncated ||= one.truncated
		}
		return {items: all, truncated}
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
						? analyzeLightEntities(document, classification.operationName, params.variables)
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
				recordGatewayShadow(field, "unsupported_selection")
				recordGatewayUnsupported(field, analysis.reason)
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
			const judged = await Promise.all(collected.items.map((item) => judge(deps, item, label)))
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
				for (const reason of new Set([...planned, ...judged.flatMap((j) => j.uncompared)]))
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
