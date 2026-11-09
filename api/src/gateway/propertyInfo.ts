/**
 * Two parts of an entity answer the shadow used to count and skip:
 *
 * `property_info` (`GEODB_PROPERTY_INFO_SHADOW`): the metadata a values row's `property { .. }` object carries beyond its
 * id and name (`dataTypeId`, `dataTypeName`, `renderableTypeId`, `renderableTypeName`, `format`, `isType`). PostgreSQL
 * builds it from the Property entity's own statements, in any Space (`build_property_info`, migration 0040): the target
 * of its `DATA_TYPE` and `RENDERABLE_TYPE` relations and that target's Name, and its `format` and `is_type` values. geo-db
 * holds those same statements, so the Property entity's card across every Space (`.args.ids`, with names) settles it. No
 * schema root is read: the `properties` root lists declarations, which the deployed stores answer empty.
 *
 *   - an id, `format` or `isType` PostgreSQL gave must be a statement geo-db holds; one PostgreSQL gave as null must be
 *     one geo-db holds no statement for.
 *   - a name PostgreSQL gave must be the name geo-db gives some target of that relation; null, none.
 *
 * `membership_only` (`GEODB_RELATIONS_MEMBERSHIP_SHADOW`): an unfiltered `allRelationsList { spaceId }`. geo-db's reference
 * statements of the entity (`.args.relations = true`, geo-db ISS-590-263) in the Spaces that hold it must each have a row
 * in PostgreSQL's list, Space by Space. PostgreSQL also lists the relations it derives, which geo-db never holds, so more
 * rows on its side are no difference. A list that came back at `first` rows may be cut, and is not judged.
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
import {GeoDbError, MAX_CARD_IDS} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import {normalizeUuid} from "../utils/uuid"
import {asRecord, type EntityShadowDeps, fieldsOf, isPlainField, MAX_EVERY_SPACES, responseKeyOf} from "./entityShadow"

/** PostgreSQL's property_info columns, by the PostGraphile field that carries each. */
export const PROPERTY_INFO_FIELDS = [
	"dataTypeId",
	"dataTypeName",
	"renderableTypeId",
	"renderableTypeName",
	"format",
	"isType",
] as const
export type PropertyInfoField = (typeof PROPERTY_INFO_FIELDS)[number]

const DATA_TYPE_RELATION = "6d29d578-49bb-4959-baf7-2cc696b1671a"
const RENDERABLE_TYPE_RELATION = "2316bbe1-c76f-4635-83f2-3e03b4f1fe46"
const FORMAT_PROPERTY = "396f8c72-dfd0-4b57-91ea-09c1b9321b2f"
const IS_TYPE_PROPERTY = "d2c1a101-14e3-464a-8272-f4e75b0f1407"

/** Most distinct Properties one comparison reads: two batch reads of geo-db's bound. */
export const MAX_PROPERTY_INFO_IDS = 2 * MAX_CARD_IDS

/** Where the property_info fields of a values row's `property` object sit. */
export type PropertyInfoPlan = {
	/** Response key of the `property` object in the row. */
	key: string
	/** Response key of each compared field inside it. */
	fields: Partial<Record<PropertyInfoField, string>>
	/** A selection was left unplanned (an argument, a second `property` object): the reason stays. */
	residual: boolean
}

/** Plans the `property` object of a values row. Never throws. */
export function planPropertyInfo(
	field: FieldNode,
	fragments: Map<string, SelectionSetNode>,
	into: PropertyInfoPlan | undefined,
): PropertyInfoPlan | undefined {
	const key = responseKeyOf(field)
	if (into && into.key !== key) return {...into, residual: true}
	const plan: PropertyInfoPlan = into ?? {key, fields: {}, residual: false}
	try {
		for (const inner of field.selectionSet ? fieldsOf(field.selectionSet, fragments) : []) {
			const name = inner.name.value as PropertyInfoField
			if (!(PROPERTY_INFO_FIELDS as readonly string[]).includes(name)) continue
			if (!isPlainField(inner) || plan.fields[name] !== undefined) plan.residual = true
			else plan.fields[name] = responseKeyOf(inner)
		}
	} catch {
		plan.residual = true
	}
	return Object.keys(plan.fields).length > 0 || plan.residual ? plan : into
}

/** One property_info as PostGraphile answered it. */
export type PropertyInfoPg = {id: string} & Partial<Record<PropertyInfoField, string | boolean | null>>

/** The property_info objects of a card's values rows; `incomplete` when some part is not compared (so `property_info` stays). */
export type PropertyInfos = {items: PropertyInfoPg[]; incomplete: boolean}

/** Reads the property_info of one values row (`node`) into `into`. */
export function collectPropertyInfo(
	node: Record<string, unknown>,
	propertyId: string,
	plan: PropertyInfoPlan,
	into: PropertyInfos,
): void {
	if (plan.residual) into.incomplete = true
	const object = asRecord(node[plan.key])
	if (!object) {
		// A row whose Property PostGraphile could not resolve has no metadata to judge.
		if (node[plan.key] !== null) into.incomplete = true
		return
	}
	const item: PropertyInfoPg = {id: propertyId}
	for (const field of PROPERTY_INFO_FIELDS) {
		const key = plan.fields[field]
		if (key === undefined) continue
		const v = object[key]
		const want = field === "isType" ? "boolean" : "string"
		if (v === null || v === undefined) item[field] = null
		else if (typeof v === want) item[field] = v as string | boolean
		else {
			into.incomplete = true
			return
		}
	}
	into.items.push(item)
}

export type PropertyInfoVerdict =
	| {read: false}
	| {
			read: true
			/** Per field that disagreed, the number of Properties. */
			mismatches: Record<string, number>
			/** Reasons the read leaves open (`no_statements`, `cross_space`). */
			uncompared: ("no_statements" | "cross_space")[]
			/** Every field was judged: `property_info` is no longer uncompared. */
			settled: boolean
			properties: number
	  }

/**
 * Judges the Properties' metadata against one batched card read of them across every Space. `{read: false}` when it
 * cannot be read (too many Properties, no every-Space reads, no batch client, an empty budget, a failed read): nothing
 * is counted and `property_info` stays. Never throws.
 */
export async function checkPropertyInfo(
	deps: EntityShadowDeps,
	info: PropertyInfos,
	label: string,
): Promise<PropertyInfoVerdict> {
	const readCards = deps.geoDb.readCards?.bind(deps.geoDb)
	if (!deps.allGeoSpaces || !readCards) return {read: false}
	const byProperty = new Map<string, PropertyInfoPg[]>()
	for (const item of info.items) {
		const id = conformingId("entity", item.id)
		byProperty.set(id, [...(byProperty.get(id) ?? []), item])
	}
	if (byProperty.size === 0)
		return {read: true, mismatches: {}, uncompared: [], settled: !info.incomplete, properties: 0}
	if (byProperty.size > MAX_PROPERTY_INFO_IDS) return {read: false}
	const ids = [...byProperty.keys()]
	try {
		const spaces = await deps.allGeoSpaces()
		if (spaces.length === 0 || spaces.length > MAX_EVERY_SPACES) return {read: false}
		const reads = Math.ceil(ids.length / MAX_CARD_IDS)
		if (deps.everySpaceBudget && !deps.everySpaceBudget.tryAcquire(reads)) return {read: false}
		const started = performance.now()
		let rows: GeoCardRow[]
		try {
			rows = await readCards({geoSpaceIds: spaces, entityIds: ids, names: true})
			observeGeoDbRead((performance.now() - started) / 1000, "ok")
		} catch (error) {
			observeGeoDbRead((performance.now() - started) / 1000, "error")
			throw error
		}
		const held = new Map<string, GeoCardRow[]>(ids.map((id) => [id, []]))
		for (const row of rows) held.get(row.entity ?? "")?.push(row)
		return judgePropertyInfo(byProperty, held, !info.incomplete)
	} catch (error) {
		deps.log.warn(`${label} property info read failed`, {
			properties: ids.length,
			reason: error instanceof GeoDbError ? error.reason : "error",
			error: String(error),
		})
		return {read: false}
	}
}

/** The comparison itself, over the cards geo-db answered. Exported for tests. */
export function judgePropertyInfo(
	byProperty: Map<string, PropertyInfoPg[]>,
	held: Map<string, GeoCardRow[]>,
	complete: boolean,
): PropertyInfoVerdict {
	const mismatches: Record<string, number> = {}
	const uncompared = new Set<"no_statements" | "cross_space">()
	const flag = (field: string) => {
		mismatches[field] = (mismatches[field] ?? 0) + 1
	}
	const dataType = conformingId("entity", DATA_TYPE_RELATION)
	const renderable = conformingId("entity", RENDERABLE_TYPE_RELATION)
	const format = conformingId("entity", FORMAT_PROPERTY)
	const isType = conformingId("entity", IS_TYPE_PROPERTY)
	for (const [id, items] of byProperty) {
		const geo = held.get(id) ?? []
		if (geo.length === 0) {
			// geo-db holds nothing of this Property in any loaded Space: a gap the feeder owns, not a verdict on its fields.
			uncompared.add("no_statements")
			continue
		}
		for (const item of items) {
			const relation = (field: "dataType" | "renderableType", relationType: string) => {
				const rows = geo.filter((row) => row.prop === relationType)
				const wantId = item[`${field}Id`]
				if (wantId !== undefined) {
					if (
						wantId === null
							? rows.length > 0
							: !rows.some((row) => row.raw === conformingId("entity", String(wantId)))
					)
						flag(`${field}Id`)
				}
				const wantName = item[`${field}Name`]
				if (wantName !== undefined) {
					const names = rows.map((row) => row.targetName).filter((n): n is string => typeof n === "string")
					if (wantName === null ? names.length > 0 : names.length > 0 && !names.includes(String(wantName)))
						flag(`${field}Name`)
					// A target geo-db prints no name for (named outside the read) leaves the name unjudged.
					else if (wantName !== null && names.length === 0) uncompared.add("cross_space")
				}
			}
			relation("dataType", dataType)
			relation("renderableType", renderable)
			const value = (field: "format" | "isType", property: string) => {
				const want = item[field]
				if (want === undefined) return
				const rows = geo.filter((row) => row.prop === property && row.raw !== "")
				if (want === null) {
					if (rows.length > 0) flag(field)
					return
				}
				const row: ValueRow = {
					property_id: property,
					text: field === "format" ? String(want) : null,
					integer: null,
					float: null,
					decimal: null,
					boolean: field === "isType" ? Boolean(want) : null,
					datetime: null,
					date: null,
				}
				const relations: RelationRow[] = []
				const diff = diffCards(statementsFromPostgres([row], relations), rows, {skipEmptyText: true})
				if (diff.onlyPostgres.length > 0) flag(field)
			}
			value("format", format)
			value("isType", isType)
		}
	}
	return {read: true, mismatches, uncompared: [...uncompared], settled: complete, properties: byProperty.size}
}

/** What an unfiltered `allRelationsList { spaceId }` of one entity came to. */
export type RelationsMembership = {
	/** PostGraphile's row Spaces, as it spelled them (null: a row that did not say). */
	spaces: (string | null)[]
	/** The list came back at `first` rows, so it may have been cut. */
	cut: boolean
}

export type MembershipVerdict =
	| {read: false}
	| {read: true; mismatch: boolean; detail: {geoRelations: number; spaces: number; excessSpaces: string[]}}

/**
 * Holds geo-db's reference statements of the entity, in the Spaces that hold it (`populated`, geo-db Space ids), to
 * PostgreSQL's row Spaces: no Space may hold more of them than PostgreSQL lists. `toGeoSpace` maps a PostgreSQL Space id
 * to geo-db's (the identity on a read of named Spaces). `{read: false}` when it cannot be judged. Never throws.
 */
export async function checkRelationsMembership(
	deps: EntityShadowDeps,
	entityId: string,
	membership: RelationsMembership,
	populated: string[],
	every: boolean,
	toGeoSpace: (space: string) => Promise<string | null>,
	label: string,
): Promise<MembershipVerdict> {
	if (membership.cut || membership.spaces.some((s) => s === null)) return {read: false}
	const readCardSpaces = deps.geoDb.readCardSpaces?.bind(deps.geoDb)
	if (!readCardSpaces) return {read: false}
	// geo-db holds no statement of the entity anywhere it was read: it holds no relation, and PostgreSQL's rows are all its own.
	if (populated.length === 0)
		return {read: true, mismatch: false, detail: {geoRelations: 0, spaces: 0, excessSpaces: []}}
	if (populated.length > 1024) return {read: false}
	const budget = every ? deps.everySpaceBudget : deps.budget
	if (budget && !budget.tryAcquire(1)) return {read: false}
	const started = performance.now()
	let rows: GeoCardRow[]
	try {
		rows = await readCardSpaces({
			geoSpaceIds: populated,
			entityId: conformingId("entity", entityId),
			relations: true,
			...(every && deps.everySpaceTimeoutMs ? {timeoutMs: deps.everySpaceTimeoutMs} : {}),
		})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		deps.log.warn(`${label} relations read failed`, {
			entityId,
			reason: error instanceof GeoDbError ? error.reason : "error",
			error: String(error),
		})
		return {read: false}
	}
	const listed = new Map<string, number>()
	for (const space of membership.spaces) {
		const geo = await toGeoSpace(normalizeUuid(space as string))
		// A Space geo-db has not loaded holds no row of geo-db's to be missing.
		if (geo) listed.set(geo, (listed.get(geo) ?? 0) + 1)
	}
	const held = new Map<string, number>()
	for (const row of rows) {
		const space = row.space ?? (populated.length === 1 ? populated[0] : undefined)
		if (space === undefined) return {read: false}
		held.set(space, (held.get(space) ?? 0) + 1)
	}
	const excessSpaces = [...held].filter(([space, count]) => count > (listed.get(space) ?? 0)).map(([space]) => space)
	return {
		read: true,
		mismatch: excessSpaces.length > 0,
		detail: {geoRelations: rows.length, spaces: held.size, excessSpaces: excessSpaces.slice(0, 3)},
	}
}
