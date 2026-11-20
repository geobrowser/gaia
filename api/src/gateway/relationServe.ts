import {Kind, type OperationDefinitionNode} from "graphql"
import {conformingId} from "../geodb/card"
import type {GeoDbCardClient} from "../geodb/client"
import type {GaiaIdsOf} from "../geodb/idMap"
import {argsOf, fieldsOf, isPlainField, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"
import {RELATION_FIELD} from "./pointReadShadows"

/**
 * The geo-db resolver for the `relation(id)` root field (GRAPHQL_GATEWAY_FIELDS `relation=on`).
 *
 * `readRelationEntity` (`.field = "entity", .args = .{ .statement = "<id>" }`) looks up a relation by id
 * across every Space and returns its `fromEntityId` (or an empty page when closed / absent). It does not
 * return `spaceId`, `typeId`, `toEntityId`, `entityId`, or relation facets (`toSpaceId`, `verified`,
 * `position`), so this resolver answers only the exact shape that `readRelationEntity` plus the reverse
 * id map can settle:
 *
 *   relation(id: <uuid>) { id fromEntityId __typename }
 *
 * (any non-empty subset, with aliases, fragments and variables). Everything else declines with a closed
 * reason from `serveMetrics.ts` so PostGraphile answers it:
 * - any other column or nested object (`spaceId`, `typeId`, `toEntityId`, `entityId`, `entity`, `fromEntity`, ...): `selection`
 * - missing/invalid `id` or any other argument: `args`
 * - `readRelationEntity` unavailable on the client: `not_enabled`
 * - empty page (absent, closed, or Postgres-only derived relation): `empty`
 * - `fromEntityId` missing from the reverse id map: `legacy_id`
 * - aborted signal: `timeout`
 */

export type RelationServeDeps = {
	geoDb: Pick<GeoDbCardClient, "readRelationEntity">
	/** The reverse id map (geo-db id -> gaia id). Absent: never serve. */
	gaiaIdsOf?: GaiaIdsOf
}

const COLUMNS = ["id", "fromEntityId"] as const

type Plan = {
	key: string
	fields: {key: string; name: (typeof COLUMNS)[number] | "__typename"}[]
	id: string
}

const decline = (reason: string): ResolveResult => ({ok: false, reason})

/** Plans the one supported `relation(id)` shape, or throws `Unsupported` with the reason. */
export function planRelationServe(
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
	if (parsed.roots.length !== 1 || !root || root.name.value !== RELATION_FIELD)
		throw new Unsupported("not_single_root")
	const args = argsOf(root, parsed.variables)
	for (const [name, value] of Object.entries(args)) {
		if (value !== undefined && value !== null && name !== "id") throw new Unsupported("args")
	}
	if (args.id === undefined || args.id === null) throw new Unsupported("args")
	const id = uuidArg(args.id)
	if (!root.selectionSet) throw new Unsupported("selection")
	const fields: Plan["fields"] = []
	for (const field of fieldsOf(root.selectionSet, parsed.fragments)) {
		const name = field.name.value
		if (!isPlainField(field) || !(name === "__typename" || (COLUMNS as readonly string[]).includes(name))) {
			throw new Unsupported("selection")
		}
		fields.push({key: responseKeyOf(field), name: name as Plan["fields"][number]["name"]})
	}
	if (fields.length === 0) throw new Unsupported("selection")
	return {key: responseKeyOf(root), fields, id}
}

export function createRelationResolver(deps: RelationServeDeps): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: Plan
		try {
			plan = planRelationServe(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const readRelationEntity = deps.geoDb.readRelationEntity?.bind(deps.geoDb)
		if (!readRelationEntity) return decline("not_enabled")
		const {gaiaIdsOf} = deps
		if (!gaiaIdsOf) return decline("legacy_id")

		const rows = await readRelationEntity({relationId: conformingId("entity", plan.id)})
		if (signal.aborted) return decline("timeout")
		if (rows.length === 0) return decline("empty")

		const entities = [...new Set(rows.map((row) => row.entity).filter((id): id is string => Boolean(id)))]
		if (entities.length === 0) return decline("empty")
		if (entities.length > 1) return decline("error")
		const geoFrom = entities[0] as string

		const gaia = await gaiaIdsOf([geoFrom])
		if (signal.aborted) return decline("timeout")
		const fromEntityId = gaia.get(geoFrom)
		if (!fromEntityId) return decline("legacy_id")

		const values = {id: plan.id, fromEntityId}
		const result = Object.fromEntries(
			plan.fields.map((f) => [f.key, f.name === "__typename" ? "Relation" : values[f.name]]),
		)
		return {ok: true, data: {[plan.key]: result}}
	}
}
