import {Kind, type OperationDefinitionNode} from "graphql"
import {conformingId} from "../geodb/card"
import {MAX_PAGE} from "../geodb/client"
import type {GaiaIdsOf} from "../geodb/idMap"
import type {CardDeps} from "../geodb/service"
import {argsOf, fieldsOf, isPlainField, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"
import {ORDERED_ROOT} from "./lightShadow"
import {compareKeys, keyOfRow} from "./propertyOrderShadow"

/**
 * The geo-db resolver for the `entitiesOrderedByProperty` root field (GRAPHQL_GATEWAY_FIELDS
 * `entitiesOrderedByProperty=on`).
 *
 * It answers one exact shape using geo-db's property-ordered page (`readEntitiesPage`):
 *
 *   entitiesOrderedByProperty(
 *     propertyId: <uuid>,
 *     spaceIds: [<uuid>],
 *     typeIds: [<uuid>],
 *     dataType: "integer" | "float" | "decimal" | "boolean" | "date" | "datetime",
 *     sortDirection?: ASC | DESC,
 *     includeWithoutValue?: false,
 *     first: n,
 *     offset?: m
 *   ) { id __typename }
 *
 * with no `filter`, no vote/id narrowing (`votedBy`, `votedByKinds`, `votedByTypes`, `entityIds`), and no other
 * argument (aliases, fragments and variables allowed). Everything else declines with a reason from the closed set in
 * `serveMetrics.ts` so PostGraphile answers it instead (`gaia_api_gateway_serve_fallback_total`).
 *
 * Exactness rules:
 * - Single Space and single Type: `entities_ordered_by_property` requires the TYPES relation in the same Space as the
 *   value (`r.space_id = v.space_id`), and `readEntitiesPage` takes one `typeId` and returns rows without a Space cell.
 *   A single Space satisfies the same-Space predicate automatically; several Spaces decline `many_spaces`.
 * - Exact key order only: `text` depends on Postgres's collation (which geo-db does not know), and omitted `dataType`
 *   looks up the property's type in Postgres, so both decline `args`.
 * - Whole set only: geo-db breaks ties by its own id (where legacy ids hash elsewhere), and a page cut at `ask = offset
 *   + first` distinct entities can hide tied legacy entities past the cut. So any read returning `>= ask` distinct
 *   entities (or `ask > MAX_PAGE`) declines `truncated`. The whole set's scored entities are mapped back to gaia ids
 *   through `gaiaIdsOf` (`legacy_id` on miss, `id_map` on read error), ordered by `(sort_value <dir>, gaia_id ASC)`
 *   matching `entities_ordered_by_property`, and sliced to `[offset, offset + first)`.
 * - `readEntitiesPage` is gated behind `orderPick` (`GEODB_ORDER_PICK_SHADOW=on`); without it, `not_enabled`.
 */

export type EntitiesOrderedByPropertyServeDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {
	/** The reverse id map (geo-db id -> gaia id). Absent: never serve. */
	gaiaIdsOf?: GaiaIdsOf
	/** The ordered page read is switched on (GEODB_ORDER_PICK_SHADOW=on). */
	orderPick: boolean
}

type Key = NonNullable<ReturnType<typeof keyOfRow>>

type Plan = {
	key: string
	fields: {key: string; name: "id" | "__typename"}[]
	propertyId: string
	spaceId: string
	typeId: string
	dataType: string
	direction: "asc" | "desc"
	first: number
	offset: number
}

const decline = (reason: string): ResolveResult => ({ok: false, reason})

const ARGS = new Set([
	"propertyId",
	"spaceIds",
	"typeIds",
	"dataType",
	"sortDirection",
	"includeWithoutValue",
	"first",
	"offset",
])
const FILTER_ARGS = new Set(["filter", "votedBy", "votedByKinds", "votedByTypes", "entityIds"])
const EXACT_DATA_TYPES = new Set(["integer", "float", "decimal", "boolean", "date", "datetime"])
const given = (value: unknown) => value !== undefined && value !== null

/** A plain list of UUIDs (or one UUID string via GraphQL list coercion), deduplicated. */
function uuidList(value: unknown): string[] {
	const listed = Array.isArray(value) ? value : typeof value === "string" ? [value] : null
	if (!listed) throw new Unsupported("args")
	return [...new Set(listed.map((id) => uuidArg(id)))]
}

/** A `first` or `offset`: a whole number, at least `min`. */
function whole(value: unknown, min: number): number {
	if (typeof value !== "number" || !Number.isInteger(value) || value < min) throw new Unsupported("args")
	return value
}

/** Plans the one supported shape, or throws `Unsupported` with the reason. */
export function planEntitiesOrderedByPropertyServe(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): Plan {
	const parsed = parseOperation(document, operationName, variables)
	const operation = document.definitions.find(
		(d): d is OperationDefinitionNode =>
			d.kind === Kind.OPERATION_DEFINITION && (operationName ? d.name?.value === operationName : true),
	)
	// A root `__typename` beside the field is dropped by parseOperation; it would be missing from the answer.
	if (!operation || operation.selectionSet.selections.length !== 1) throw new Unsupported("not_single_root")
	const [root] = parsed.roots
	if (parsed.roots.length !== 1 || !root || root.name.value !== ORDERED_ROOT) throw new Unsupported("not_single_root")
	const args = argsOf(root, parsed.variables)
	for (const [name, value] of Object.entries(args)) {
		if (!given(value)) continue
		if (FILTER_ARGS.has(name)) throw new Unsupported("filter")
		if (!ARGS.has(name)) throw new Unsupported("args")
	}
	if (!given(args.propertyId)) throw new Unsupported("args")
	const propertyId = uuidArg(args.propertyId)
	if (!given(args.spaceIds)) throw new Unsupported("no_space")
	const spaceIds = uuidList(args.spaceIds)
	if (spaceIds.length === 0) throw new Unsupported("no_space")
	if (spaceIds.length > 1) throw new Unsupported("many_spaces")
	if (!given(args.typeIds)) throw new Unsupported("filter")
	const typeIds = uuidList(args.typeIds)
	if (typeIds.length === 0) throw new Unsupported("filter")
	if (typeIds.length > 1) throw new Unsupported("args")
	if (typeof args.dataType !== "string") throw new Unsupported("args")
	const dataType = args.dataType.toLowerCase()
	if (!EXACT_DATA_TYPES.has(dataType)) throw new Unsupported("args")
	let direction: "asc" | "desc" = "asc"
	if (given(args.sortDirection)) {
		if (args.sortDirection !== "ASC" && args.sortDirection !== "DESC") throw new Unsupported("args")
		direction = args.sortDirection === "DESC" ? "desc" : "asc"
	}
	if (given(args.includeWithoutValue) && args.includeWithoutValue !== false) throw new Unsupported("args")
	const first = whole(args.first, 1)
	const offset = given(args.offset) ? whole(args.offset, 0) : 0
	if (!root.selectionSet) throw new Unsupported("selection")
	const fields: Plan["fields"] = []
	for (const field of fieldsOf(root.selectionSet, parsed.fragments)) {
		const name = field.name.value
		if ((name !== "id" && name !== "__typename") || !isPlainField(field)) throw new Unsupported("selection")
		fields.push({key: responseKeyOf(field), name})
	}
	if (fields.length === 0) throw new Unsupported("selection")
	return {
		key: responseKeyOf(root),
		fields,
		propertyId,
		spaceId: spaceIds[0] as string,
		typeId: typeIds[0] as string,
		dataType,
		direction,
		first,
		offset,
	}
}

export function createEntitiesOrderedByPropertyResolver(deps: EntitiesOrderedByPropertyServeDeps): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: Plan
		try {
			plan = planEntitiesOrderedByPropertyServe(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const {gaiaIdsOf} = deps
		const readEntitiesPage = deps.geoDb.readEntitiesPage?.bind(deps.geoDb)
		if (!deps.orderPick || !readEntitiesPage) return decline("not_enabled")
		if (!gaiaIdsOf) return decline("legacy_id")
		const ask = plan.offset + plan.first
		if (ask > MAX_PAGE) return decline("truncated")
		const geoSpaceId = await deps.geoSpaceOf(plan.spaceId)
		if (signal.aborted) return decline("timeout")
		if (!geoSpaceId) return decline("unmapped_space")
		const field = conformingId("entity", plan.propertyId)
		const pick = plan.direction === "asc" ? "least" : "greatest"
		const rows = await readEntitiesPage({
			geoSpaceIds: [geoSpaceId],
			typeId: conformingId("entity", plan.typeId),
			field,
			direction: plan.direction,
			pick,
			first: ask,
		})
		if (signal.aborted) return decline("timeout")
		const seen = new Set<string>()
		const bestKey = new Map<string, Key>()
		for (const row of rows) {
			seen.add(row.entity)
			if (row.prop !== field) continue
			if (row.raw.trim().length === 0) continue
			const key = keyOfRow(plan.dataType, row.raw)
			if (!key) return decline("args")
			const held = bestKey.get(row.entity)
			if (!held) {
				bestKey.set(row.entity, key)
			} else {
				const cmp = compareKeys(key, held)
				if (cmp === null) return decline("args")
				if (plan.direction === "asc" ? cmp < 0 : cmp > 0) bestKey.set(row.entity, key)
			}
		}
		// Cut at the page window: tied or legacy entities past the cut are unseen, so the order is not PostGraphile's.
		if (seen.size >= ask) return decline("truncated")
		if (bestKey.size === 0) return {ok: true, data: {[plan.key]: []}}
		const gaia = await gaiaIdsOf([...bestKey.keys()])
		if (signal.aborted) return decline("timeout")
		const scored: {gaiaId: string; key: Key}[] = []
		for (const [geoId, key] of bestKey) {
			const mapped = gaia.get(geoId)
			if (!mapped) return decline("legacy_id")
			scored.push({gaiaId: mapped, key})
		}
		// `entities_ordered_by_property`: ORDER BY sub.sort_value <dir>, e.id ASC.
		scored.sort((a, b) => {
			const cmp = compareKeys(a.key, b.key)
			if (cmp !== null && cmp !== 0) return plan.direction === "asc" ? cmp : -cmp
			return a.gaiaId < b.gaiaId ? -1 : a.gaiaId > b.gaiaId ? 1 : 0
		})
		const page = scored.slice(plan.offset, plan.offset + plan.first)
		return {
			ok: true,
			data: {
				[plan.key]: page.map(({gaiaId}) =>
					Object.fromEntries(plan.fields.map((f) => [f.key, f.name === "id" ? gaiaId : "Entity"])),
				),
			},
		}
	}
}
