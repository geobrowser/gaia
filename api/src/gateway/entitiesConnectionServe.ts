import {Kind, type OperationDefinitionNode} from "graphql"
import {conformingId} from "../geodb/card"
import {MAX_LINE_SPACES, MAX_TYPE_SCAN, MAX_TYPE_SCAN_TYPES} from "../geodb/client"
import {ENTITIES_CONNECTION_FIELD} from "./entitiesConnectionShadow"
import type {EntitiesServeDeps} from "./entitiesServe"
import {argsOf, fieldsOf, isPlainField, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"
import {typeScanAsk, typeScanWindow} from "./typeScanShadow"

/**
 * The geo-db resolver for the `entitiesConnection` root field (GRAPHQL_GATEWAY_FIELDS `entitiesConnection=on`).
 *
 * Answers the same type-scan page shape as `entitiesServe.ts`, wrapped in a connection's `nodes` list:
 *
 *   entitiesConnection(spaceId | spaceIds: {in}, typeId | typeIds: {in}, first: n, offset: m) {
 *     nodes { id __typename }
 *     __typename
 *   }
 *
 * `totalCount`, `pageInfo`, `edges`, and any other connection or node field decline `selection` until a
 * count read and cursor design exist. Same exactness rules as `entitiesServe.ts`: whole set only
 * (`scanned.length >= ask` declines `truncated`), mapped back to gaia ids through `gaiaIdsOf` (`legacy_id`
 * / `id_map`), sorted by gaia id (PostGraphile's primary-key order), and sliced to the requested window.
 */

type Plan = {
	key: string
	/** Response keys on the EntitiesConnection wrapper: `"nodes"` stands for the node list. */
	connectionFields: {key: string; name: "nodes" | "__typename"}[]
	nodeFields: {key: string; name: "id" | "__typename"}[]
	spaceIds: string[]
	typeIds: string[]
	first: number
	offset: number
}

const decline = (reason: string): ResolveResult => ({ok: false, reason})

const ARGS = new Set(["spaceId", "spaceIds", "typeId", "typeIds", "first", "offset"])
const given = (value: unknown) => value !== undefined && value !== null

function inList(value: unknown): string[] {
	const clause = value && typeof value === "object" ? (value as Record<string, unknown>) : null
	if (!clause || Object.keys(clause).length !== 1 || !Array.isArray(clause.in) || clause.in.length === 0)
		throw new Unsupported("args")
	return [...new Set(clause.in.map((id) => uuidArg(id)))]
}

function whole(value: unknown, min: number): number {
	if (typeof value !== "number" || !Number.isInteger(value) || value < min) throw new Unsupported("args")
	return value
}

function oneOf(args: Record<string, unknown>, single: string, list: string): string[] | null {
	if (given(args[single]) && given(args[list])) throw new Unsupported("args")
	if (given(args[single])) return [uuidArg(args[single])]
	return given(args[list]) ? inList(args[list]) : null
}

/** Plans the one supported connection shape, or throws `Unsupported` with the reason. */
export function planEntitiesConnectionServe(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): Plan {
	const parsed = parseOperation(document, operationName, variables)
	const operation = document.definitions.find(
		(d): d is OperationDefinitionNode =>
			d.kind === Kind.OPERATION_DEFINITION && (operationName ? d.name?.value === operationName : true),
	)
	if (!operation || operation.selectionSet.selections.length !== 1) throw new Unsupported("not_single_root")
	const [root] = parsed.roots
	if (parsed.roots.length !== 1 || !root || root.name.value !== ENTITIES_CONNECTION_FIELD)
		throw new Unsupported("not_single_root")
	const args = argsOf(root, parsed.variables)
	if (given(args.filter)) throw new Unsupported("filter")
	for (const [name, value] of Object.entries(args)) if (given(value) && !ARGS.has(name)) throw new Unsupported("args")
	const spaceIds = oneOf(args, "spaceId", "spaceIds")
	if (!spaceIds) throw new Unsupported("no_space")
	const typeIds = oneOf(args, "typeId", "typeIds")
	if (!typeIds) throw new Unsupported("filter")
	const first = whole(args.first, 1)
	const offset = given(args.offset) ? whole(args.offset, 0) : 0
	if (!root.selectionSet) throw new Unsupported("selection")

	const connectionFields: Plan["connectionFields"] = []
	let nodeFields: Plan["nodeFields"] | null = null
	for (const field of fieldsOf(root.selectionSet, parsed.fragments)) {
		const name = field.name.value
		if (name === "__typename") {
			if (!isPlainField(field)) throw new Unsupported("selection")
			connectionFields.push({key: responseKeyOf(field), name})
			continue
		}
		if (name !== "nodes" || field.arguments?.length || field.directives?.length || !field.selectionSet) {
			throw new Unsupported("selection")
		}
		if (nodeFields !== null) throw new Unsupported("selection")
		const selected: Plan["nodeFields"] = []
		for (const leaf of fieldsOf(field.selectionSet, parsed.fragments)) {
			const leafName = leaf.name.value
			if ((leafName !== "id" && leafName !== "__typename") || !isPlainField(leaf))
				throw new Unsupported("selection")
			selected.push({key: responseKeyOf(leaf), name: leafName})
		}
		if (selected.length === 0) throw new Unsupported("selection")
		nodeFields = selected
		connectionFields.push({key: responseKeyOf(field), name: "nodes"})
	}
	if (!nodeFields) throw new Unsupported("selection")
	return {key: responseKeyOf(root), connectionFields, nodeFields, spaceIds, typeIds, first, offset}
}

export function createEntitiesConnectionResolver(deps: EntitiesServeDeps): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: Plan
		try {
			plan = planEntitiesConnectionServe(document, params.operationName, params.variables ?? {})
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
		if (scanned.length >= ask) return decline("truncated")
		const gaia = await gaiaIdsOf(scanned)
		if (signal.aborted) return decline("timeout")
		const ids: string[] = []
		for (const id of scanned) {
			const mapped = gaia.get(id)
			if (!mapped) return decline("legacy_id")
			ids.push(mapped)
		}
		ids.sort()
		const page = typeScanWindow(ids, plan)
		const nodes = page.map((id) =>
			Object.fromEntries(plan.nodeFields.map((f) => [f.key, f.name === "id" ? id : "Entity"])),
		)
		const connection = Object.fromEntries(
			plan.connectionFields.map((f) => [f.key, f.name === "nodes" ? nodes : "EntitiesConnection"]),
		)
		return {ok: true, data: {[plan.key]: connection}}
	}
}
