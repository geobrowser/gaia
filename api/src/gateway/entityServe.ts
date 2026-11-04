import type {FieldNode} from "graphql"
import {conformingId, DERIVED_PROPERTIES, MEMBERSHIP_PROPERTIES} from "../geodb/card"
import type {GaiaIdsOf} from "../geodb/idMap"
import {askGeoDb, type CardDeps} from "../geodb/service"
import {argsOf, ENTITY_FIELD, fieldsOf, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"

/**
 * The geo-db resolver for the `entity` root field (GRAPHQL_GATEWAY_FIELDS `entity=on`).
 *
 * It answers one shape, and only exactly:
 *
 *   entity(id: <uuid>) { id __typename relationsList(filter: {spaceId: {is: <uuid>}}) { typeId toEntityId __typename } }
 *
 * (aliases and fragments allowed, every listed field optional, `relationsList` at most once). Everything else
 * declines with the reason PostGraphile then answers it for, so the fallback counter says which gap to close
 * next. Values are not served: a geo-db card does not say whether a value was text, a number or a boolean.
 *
 * Every id geo-db returns is in its own (conformed) id space, where a legacy gaia id is a one-way hash, so the
 * answer is served only through `gaiaIdsOf`, the reverse map (geodb/idMap.ts: Postgres, migration 0116). It
 * maps every id in the answer, the entity's own included, in one read after the card read; an id it cannot place
 * (Postgres does not hold it yet) declines with `legacy_id`, and a failed map read with `id_map`. Without the map
 * the resolver declines with `legacy_id` before asking geo-db anything. Known remaining gaps for an exact answer:
 * a 32-hex text value reads as a reference, the feeder collapses duplicate relations (same type and target), and
 * derived relations (rank position) live only in Postgres; see the PR and the postgres-dependency inventory.
 */

export type EntityServeDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {
	/** The reverse id map (geo-db id -> gaia id, compact); an id it leaves out is unknown. Absent: never serve. */
	gaiaIdsOf?: GaiaIdsOf
}

/** PostGraphile's page cap: a list longer than this is not what PostGraphile would return. */
const PAGE_CAP = 1000
const REFERENCE = /^[0-9a-f]{32}$/

type Plan = {
	entityKey: string
	entityId: string
	/** The entity's response keys in selection order; "relationsList" stands for the list. */
	entityFields: {key: string; name: "id" | "__typename" | "relationsList"}[]
	relations: {
		key: string
		spaceId: string
		fields: {key: string; name: "typeId" | "toEntityId" | "__typename"}[]
	} | null
}

const decline = (reason: string): ResolveResult => ({ok: false, reason})

function onlyFilterSpace(field: FieldNode, variables: Record<string, unknown>): string {
	const args = argsOf(field, variables)
	const keys = Object.keys(args)
	if (keys.length !== 1 || keys[0] !== "filter") throw new Unsupported(keys.length === 0 ? "no_space" : "args")
	const filter = args.filter as {spaceId?: {is?: unknown}} | null
	if (!filter || typeof filter !== "object" || Object.keys(filter).length !== 1) throw new Unsupported("filter")
	const clause = filter.spaceId
	if (!clause || typeof clause !== "object" || Object.keys(clause).length !== 1 || !("is" in clause))
		throw new Unsupported("filter")
	if (clause.is === null || clause.is === undefined) throw new Unsupported("no_space")
	return uuidArg(clause.is)
}

/** Plans the one supported shape, or throws `Unsupported` with the reason. */
export function planEntityServe(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): Plan {
	const {fragments, variables: vars, roots} = parseOperation(document, operationName, variables)
	const [root] = roots
	if (roots.length !== 1 || !root || root.name.value !== ENTITY_FIELD) throw new Unsupported("not_single_root")
	const args = argsOf(root, vars)
	if (Object.keys(args).length !== 1 || !("id" in args)) throw new Unsupported("args")
	const entityId = uuidArg(args.id)
	if (!root.selectionSet) throw new Unsupported("selection")
	const plan: Plan = {entityKey: responseKeyOf(root), entityId, entityFields: [], relations: null}
	for (const field of fieldsOf(root.selectionSet, fragments)) {
		const name = field.name.value
		if ((name === "id" || name === "__typename") && !field.arguments?.length && !field.selectionSet) {
			plan.entityFields.push({key: responseKeyOf(field), name})
		} else if (name === "relationsList" && field.selectionSet && plan.relations === null) {
			const spaceId = onlyFilterSpace(field, vars)
			const fields: NonNullable<Plan["relations"]>["fields"] = []
			for (const sub of fieldsOf(field.selectionSet, fragments)) {
				const subName = sub.name.value
				if (sub.arguments?.length || sub.selectionSet) throw new Unsupported("selection")
				if (subName !== "typeId" && subName !== "toEntityId" && subName !== "__typename")
					throw new Unsupported("selection")
				fields.push({key: responseKeyOf(sub), name: subName})
			}
			plan.relations = {key: responseKeyOf(field), spaceId, fields}
			plan.entityFields.push({key: plan.relations.key, name})
		} else {
			throw new Unsupported("selection")
		}
	}
	// Without a space, existence cannot be told from geo-db (no "which spaces hold this entity" read yet).
	if (!plan.relations) throw new Unsupported("no_space")
	return plan
}

export function createEntityResolver(deps: EntityServeDeps): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: Plan
		try {
			plan = planEntityServe(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const gaiaIdsOf = deps.gaiaIdsOf
		if (!gaiaIdsOf) return decline("legacy_id")
		const relations = plan.relations
		if (!relations) return decline("no_space")
		const geoEntity = conformingId("entity", plan.entityId)
		const rows = await askGeoDb(deps, geoEntity, relations.spaceId)
		if (signal.aborted) return decline("timeout")
		// An empty card cannot tell "no such entity" (PostGraphile: null) from "nothing in this space" (an empty list).
		if (rows.length === 0) return decline("empty")
		const references: {prop: string; raw: string}[] = []
		for (const row of rows) {
			if (MEMBERSHIP_PROPERTIES.has(row.prop)) continue
			if (DERIVED_PROPERTIES.has(row.prop)) return decline("selection")
			if (REFERENCE.test(row.raw)) references.push(row)
		}
		if (references.length > PAGE_CAP) return decline("truncated")
		const gaia = await gaiaIdsOf([geoEntity, ...references.flatMap((r) => [r.prop, r.raw])])
		if (signal.aborted) return decline("timeout")
		// The asked id must come back as itself: a v4 id that is some legacy entity's conformed id is not that entity.
		if (gaia.get(geoEntity) !== plan.entityId) return decline("legacy_id")
		const items: Record<string, unknown>[] = []
		for (const row of references) {
			const typeId = gaia.get(row.prop)
			const toEntityId = gaia.get(row.raw)
			if (!typeId || !toEntityId) return decline("legacy_id")
			const item: Record<string, unknown> = {}
			for (const f of relations.fields)
				item[f.key] = f.name === "__typename" ? "Relation" : f.name === "typeId" ? typeId : toEntityId
			items.push(item)
		}
		const entity: Record<string, unknown> = {}
		for (const f of plan.entityFields)
			entity[f.key] = f.name === "id" ? plan.entityId : f.name === "__typename" ? "Entity" : items
		return {ok: true, data: {[plan.entityKey]: entity}}
	}
}
