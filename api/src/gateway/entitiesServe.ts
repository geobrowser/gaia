import {Kind, type OperationDefinitionNode} from "graphql"
import {conformingId} from "../geodb/card"
import {MAX_LINE_SPACES, MAX_TYPE_SCAN, MAX_TYPE_SCAN_TYPES} from "../geodb/client"
import type {GaiaIdsOf} from "../geodb/idMap"
import type {CardDeps} from "../geodb/service"
import {argsOf, fieldsOf, isPlainField, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"
import {typeScanAsk, typeScanWindow} from "./typeScanShadow"

/**
 * The geo-db resolver for the `entities` root field (GRAPHQL_GATEWAY_FIELDS `entities=on`).
 *
 * It answers one shape, the one the type-scan shadow compares (typeScanShadow.ts):
 *
 *   entities(spaceId | spaceIds: {in}, typeId | typeIds: {in}, first: n, offset: m) { id __typename }
 *
 * with no `orderBy`, no `filter` and no other argument (aliases, fragments and variables allowed). PostGraphile
 * returns such a page in entity-id order; geo-db's type scan lists the distinct entities of the Types in the Spaces.
 * Everything else declines with the reason PostGraphile then answers it for (gaia_api_gateway_serve_fallback_total).
 *
 * Exactness rules:
 * - geo-db orders by the id it holds, which is the gaia id only for a conforming UUIDv4; a legacy id is a one-way
 *   hash that sorts elsewhere, and a legacy entity whose hash falls past the scan window is not seen at all. So only
 *   a page whose whole set was read (fewer entities than `offset + first`) is served; a cut page declines
 *   `truncated`. The whole set is mapped back to gaia ids through the reverse id map (geodb/idMap.ts), sorted by
 *   gaia id (PostGraphile's primary-key order) and then cut to the page. An id Postgres cannot place declines
 *   `legacy_id`, and a failed map read is counted `id_map`.
 * - The type scan is off by default (GEODB_TYPE_SCAN_SHADOW=on); without it, `not_enabled`.
 * - Selections other than `id` and `__typename` (names, values, relations) decline `selection`: a type scan carries
 *   no cards.
 */

export type EntitiesServeDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {
	/** The reverse id map (geo-db id -> gaia id). Absent: never serve. */
	gaiaIdsOf?: GaiaIdsOf
	/** The type scan read is switched on (GEODB_TYPE_SCAN_SHADOW=on). */
	typeScan: boolean
	/** A page may name several Spaces (GEODB_MULTI_SPACE_READS=on). */
	multiSpace: boolean
}

type Plan = {
	key: string
	fields: {key: string; name: "id" | "__typename"}[]
	spaceIds: string[]
	typeIds: string[]
	first: number
	offset: number
}

const decline = (reason: string): ResolveResult => ({ok: false, reason})

const ARGS = new Set(["spaceId", "spaceIds", "typeId", "typeIds", "first", "offset"])
const given = (value: unknown) => value !== undefined && value !== null

/** `{in: [<uuid> ..]}` and nothing else: the ids, distinct. */
function inList(value: unknown): string[] {
	const clause = value && typeof value === "object" ? (value as Record<string, unknown>) : null
	if (!clause || Object.keys(clause).length !== 1 || !Array.isArray(clause.in) || clause.in.length === 0)
		throw new Unsupported("args")
	return [...new Set(clause.in.map((id) => uuidArg(id)))]
}

/** A `first` or `offset`: a whole number, at least `min`. */
function whole(value: unknown, min: number): number {
	if (typeof value !== "number" || !Number.isInteger(value) || value < min) throw new Unsupported("args")
	return value
}

/** One of `single` or `list` (`{in: [..]}`) and not both; the ids it names, or null when neither is given. */
function oneOf(args: Record<string, unknown>, single: string, list: string): string[] | null {
	if (given(args[single]) && given(args[list])) throw new Unsupported("args")
	if (given(args[single])) return [uuidArg(args[single])]
	return given(args[list]) ? inList(args[list]) : null
}

/** Plans the one supported shape, or throws `Unsupported` with the reason. */
export function planEntitiesServe(
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
	if (parsed.roots.length !== 1 || !root || root.name.value !== "entities") throw new Unsupported("not_single_root")
	const args = argsOf(root, parsed.variables)
	if (given(args.filter)) throw new Unsupported("filter")
	for (const [name, value] of Object.entries(args)) if (given(value) && !ARGS.has(name)) throw new Unsupported("args")
	const spaceIds = oneOf(args, "spaceId", "spaceIds")
	if (!spaceIds) throw new Unsupported("no_space")
	const typeIds = oneOf(args, "typeId", "typeIds")
	// Without a Type the page is every entity of the Spaces, which a type scan does not list.
	if (!typeIds) throw new Unsupported("filter")
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
	return {key: responseKeyOf(root), fields, spaceIds, typeIds, first, offset}
}

export function createEntitiesResolver(deps: EntitiesServeDeps): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: Plan
		try {
			plan = planEntitiesServe(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const {gaiaIdsOf} = deps
		const readTypeScan = deps.geoDb.readTypeScan?.bind(deps.geoDb)
		if (!deps.typeScan || !readTypeScan) return decline("not_enabled")
		if (!gaiaIdsOf) return decline("legacy_id")
		const ask = typeScanAsk(plan)
		if (ask > MAX_TYPE_SCAN || plan.typeIds.length > MAX_TYPE_SCAN_TYPES) return decline("truncated")
		if (plan.spaceIds.length > MAX_LINE_SPACES) return decline("many_spaces")
		if (plan.spaceIds.length > 1 && !deps.multiSpace) return decline("no_space")
		const geoSpaceIds: string[] = []
		for (const space of plan.spaceIds) {
			const geo = await deps.geoSpaceOf(space)
			if (!geo) return decline("unmapped_space")
			geoSpaceIds.push(geo)
		}
		const scanned = await readTypeScan({
			geoSpaceIds: [...new Set(geoSpaceIds)],
			typeIds: [...new Set(plan.typeIds.map((id) => conformingId("entity", id)))],
			first: ask,
		})
		if (signal.aborted) return decline("timeout")
		// The scan was cut at its window: legacy entities past it are unseen, so the order is not PostGraphile's.
		if (scanned.length >= ask) return decline("truncated")
		const gaia = await gaiaIdsOf(scanned)
		if (signal.aborted) return decline("timeout")
		const ids: string[] = []
		for (const id of scanned) {
			const mapped = gaia.get(id)
			if (!mapped) return decline("legacy_id")
			ids.push(mapped)
		}
		// PostGraphile's order with no orderBy is the primary key, the id's bytes: undashed lowercase hex sorts the same.
		ids.sort()
		const page = typeScanWindow(ids, plan)
		return {
			ok: true,
			data: {
				[plan.key]: page.map((id) =>
					Object.fromEntries(plan.fields.map((f) => [f.key, f.name === "id" ? id : "Entity"])),
				),
			},
		}
	}
}
