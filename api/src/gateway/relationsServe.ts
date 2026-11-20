import {conformingId, DERIVED_PROPERTIES, ENTITY_TIME_PROPERTIES, MEMBERSHIP_PROPERTIES} from "../geodb/card"
import type {GaiaIdsOf} from "../geodb/idMap"
import {askGeoDb, type CardDeps} from "../geodb/service"
import {argsOf, fieldsOf, isPlainField, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"

/**
 * The geo-db resolver for the `relations` root field (GRAPHQL_GATEWAY_FIELDS `relations=on`).
 *
 * It answers one shape, the slice of a card the relations shadow compares whole (pointReadShadows.ts):
 *
 *   relations(filter: {fromEntityId: {is}, spaceId: {is}, typeId: {is | in}}, first: n)
 *     { typeId toEntityId fromEntityId spaceId __typename }
 *
 * (`equalTo` for `is`, aliases, fragments and variables allowed; every selected column optional). The rows are the
 * from entity's relations of those Types in that Space, read from its geo-db card. Everything else declines with the
 * reason PostGraphile then answers it for (gaia_api_gateway_serve_fallback_total):
 * - a column geo-db's card does not carry (`id`, `entityId`, `position`, `toSpaceId`, `verified`, nested objects):
 *   `selection`; any other filter column or shape of filter, or no Type: `filter`; no single Space: `no_space`;
 *   `offset`, `orderBy`, a missing or non-positive `first`, any other argument: `args`.
 * - a derived, membership or time Type, whose rows live in Postgres only: `selection`.
 * - an unknown entity or an empty card: `empty` (it cannot tell no such entity from nothing in the Space).
 * - a page that may have been cut short (as many rows as `first`): `truncated`.
 * - an id Postgres cannot place (reverse id map, geodb/idMap.ts): `legacy_id`; a failed map read: `id_map`.
 *
 * Known gaps, shared with the `entity` resolver (entityServe.ts): rows come in card order, not the relation id's
 * order PostGraphile uses; a 32-hex text value reads as a reference; the feeder collapses duplicate relations (same
 * type and target).
 */

export type RelationsServeDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {
	/** The reverse id map (geo-db id -> gaia id). Absent: never serve. */
	gaiaIdsOf?: GaiaIdsOf
}

const REFERENCE = /^[0-9a-f]{32}$/
const ARGS = new Set(["filter", "first"])
const COLUMNS = ["typeId", "toEntityId", "fromEntityId", "spaceId"] as const

type Plan = {
	key: string
	fields: {key: string; name: (typeof COLUMNS)[number] | "__typename"}[]
	fromEntityId: string
	spaceId: string
	typeIds: string[]
	first: number
}

const decline = (reason: string): ResolveResult => ({ok: false, reason})

/** The one value of `{is: v}` or `{equalTo: v}`, or null when the clause is anything else. */
function single(clause: unknown): string | null {
	if (!clause || typeof clause !== "object") return null
	const entries = Object.entries(clause)
	const [only] = entries
	if (entries.length !== 1 || !only || (only[0] !== "is" && only[0] !== "equalTo") || only[1] == null) return null
	return uuidArg(only[1])
}

/** Plans the one supported shape, or throws `Unsupported` with the reason. */
export function planRelationsServe(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): Plan {
	const parsed = parseOperation(document, operationName, variables)
	const [root] = parsed.roots
	if (parsed.roots.length !== 1 || !root || root.name.value !== "relations") throw new Unsupported("not_single_root")
	const args = argsOf(root, parsed.variables)
	for (const [name, value] of Object.entries(args))
		if (value != null && !ARGS.has(name)) throw new Unsupported("args")
	const filter = args.filter
	if (!filter || typeof filter !== "object" || Array.isArray(filter)) throw new Unsupported("filter")
	const clauses = filter as Record<string, unknown>
	for (const column of Object.keys(clauses))
		if (column !== "fromEntityId" && column !== "spaceId" && column !== "typeId") throw new Unsupported("filter")
	const spaceId = single(clauses.spaceId)
	if (!spaceId) throw new Unsupported("no_space")
	const fromEntityId = single(clauses.fromEntityId)
	if (!fromEntityId) throw new Unsupported("filter")
	let typeIds: string[]
	const typeClause = clauses.typeId
	const one = single(typeClause)
	if (one) typeIds = [one]
	else {
		const list = (typeClause as {in?: unknown} | null | undefined)?.in
		if (
			!typeClause ||
			typeof typeClause !== "object" ||
			Object.keys(typeClause).length !== 1 ||
			!Array.isArray(list) ||
			list.length === 0
		)
			throw new Unsupported("filter")
		typeIds = [...new Set(list.map((id) => uuidArg(id)))]
	}
	const first = args.first
	if (typeof first !== "number" || !Number.isInteger(first) || first < 1) throw new Unsupported("args")
	if (!root.selectionSet) throw new Unsupported("selection")
	const fields: Plan["fields"] = []
	for (const field of fieldsOf(root.selectionSet, parsed.fragments)) {
		const name = field.name.value
		if (!isPlainField(field) || !(name === "__typename" || (COLUMNS as readonly string[]).includes(name)))
			throw new Unsupported("selection")
		fields.push({key: responseKeyOf(field), name: name as Plan["fields"][number]["name"]})
	}
	if (fields.length === 0) throw new Unsupported("selection")
	return {key: responseKeyOf(root), fields, fromEntityId, spaceId, typeIds, first}
}

export function createRelationsResolver(deps: RelationsServeDeps): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: Plan
		try {
			plan = planRelationsServe(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const {gaiaIdsOf} = deps
		if (!gaiaIdsOf) return decline("legacy_id")
		const types = new Set(plan.typeIds.map((id) => conformingId("entity", id)))
		// Rows of these Types are written straight to Postgres (or are not edges), so a card cannot list them.
		for (const type of types)
			if (DERIVED_PROPERTIES.has(type) || MEMBERSHIP_PROPERTIES.has(type) || ENTITY_TIME_PROPERTIES.has(type))
				return decline("selection")
		const geoFrom = conformingId("entity", plan.fromEntityId)
		const rows = await askGeoDb(deps, geoFrom, plan.spaceId)
		if (signal.aborted) return decline("timeout")
		if (rows.length === 0) return decline("empty")
		const edges = rows.filter((row) => types.has(row.prop) && REFERENCE.test(row.raw))
		// A page as long as `first` may have been cut short; only a page short of it is the whole slice.
		if (edges.length >= plan.first) return decline("truncated")
		const gaia = await gaiaIdsOf([geoFrom, ...edges.flatMap((r) => [r.prop, r.raw])])
		if (signal.aborted) return decline("timeout")
		// The asked id must come back as itself: a v4 id that is some legacy entity's conformed id is not that entity.
		if (gaia.get(geoFrom) !== plan.fromEntityId) return decline("legacy_id")
		const items: Record<string, unknown>[] = []
		for (const row of edges) {
			const typeId = gaia.get(row.prop)
			const toEntityId = gaia.get(row.raw)
			if (!typeId || !toEntityId) return decline("legacy_id")
			const values = {typeId, toEntityId, fromEntityId: plan.fromEntityId, spaceId: plan.spaceId}
			items.push(
				Object.fromEntries(
					plan.fields.map((f) => [f.key, f.name === "__typename" ? "Relation" : values[f.name]]),
				),
			)
		}
		return {ok: true, data: {[plan.key]: items}}
	}
}
