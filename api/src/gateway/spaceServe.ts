import type {FieldNode} from "graphql"
import {conformingId} from "../geodb/card"
import {askGeoDb, type CardDeps} from "../geodb/service"
import {argsOf, fieldsOf, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"

/**
 * The geo-db resolver for the `space` and `spaces` root fields (GRAPHQL_GATEWAY_FIELDS `space=on`, `spaces=on`).
 *
 * geo-db holds no `spaces` table, but the feeder writes each registered Space's system entity into the Space it
 * describes (geodb-feeder/src/spaceEntities.ts, the rows kg-indexer's `map_space_registered` writes). Its card holds the
 * Space's kind as a relation: Types -> EoaSpace (`spaces.type = Personal`) or DaoSpace (`DAO`). So the Space's
 * existence and `type` are answerable from one card read of entity `<space id>` in Space `<space id>`; nothing in the
 * store or the wire changes. Two shapes, and only these:
 *
 *   space(id: <uuid>) { id type __typename }
 *   spaces(filter: {id: {in: [<uuid> ..]}}) { id type __typename }
 *
 * (aliases, fragments and variables allowed, every field optional). Everything else declines with the reason
 * PostGraphile then answers it for (gaia_api_gateway_serve_fallback_total):
 * - `address`, `topicId`, `page`, `topic`, members and the rest decline `selection`: the address is lowercase on the
 *   system entity but checksummed in `spaces` after a create, and the others are keyed records or joins a card read
 *   does not carry. A filter other than the id list (address, `members some`) or any paging declines `filter` / `args`:
 *   geo-db has no Space index to enumerate or reverse-look-up.
 * - A Space geo-db has not founded declines `unmapped_space`, and a Space whose system entity is not loaded yet (or
 *   whose kind relation is absent) declines `empty`: PostgreSQL may hold either, so "no such Space" is never inferred.
 *   Because every asked Space must be held, a `spaces` page is the whole answer, in PostGraphile's primary-key order
 *   (undashed lowercase hex sorts like the id's bytes).
 */

export const SPACE_ROOT = "space"
export const SPACES_ROOT = "spaces"

/** Most Spaces one `spaces` page reads: each is one card read. */
export const MAX_SPACES_PER_PAGE = 50

const SYSTEM_TYPES_RELATION_TYPE_ID = "88b3d6ad-288c-529c-a212-0e1c24819185"
const EOA_SPACE_TYPE_ID = "09d28123-178f-5828-b85a-81979389c746"
const DAO_SPACE_TYPE_ID = "afd76215-db11-5cba-81b9-e7f77f865805"
const KIND_RELATION = conformingId("entity", SYSTEM_TYPES_RELATION_TYPE_ID)
const EOA = conformingId("entity", EOA_SPACE_TYPE_ID)
const DAO = conformingId("entity", DAO_SPACE_TYPE_ID)

export type SpaceServeDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf">

type Plan = {
	root: typeof SPACE_ROOT | typeof SPACES_ROOT
	key: string
	fields: {key: string; name: "id" | "type" | "__typename"}[]
	ids: string[]
}

const decline = (reason: string): ResolveResult => ({ok: false, reason})

/** Plans one of the two supported shapes, or throws `Unsupported` with the reason. */
export function planSpaceServe(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
	root: typeof SPACE_ROOT | typeof SPACES_ROOT,
): Plan {
	const parsed = parseOperation(document, operationName, variables)
	const [node] = parsed.roots as FieldNode[]
	if (parsed.roots.length !== 1 || !node || node.name.value !== root) throw new Unsupported("not_single_root")
	if (node.directives?.length) throw new Unsupported("directives")
	const args = argsOf(node, parsed.variables)
	let ids: string[]
	if (root === SPACE_ROOT) {
		if (Object.keys(args).length !== 1 || !("id" in args)) throw new Unsupported("args")
		ids = [uuidArg(args.id)]
	} else {
		const keys = Object.keys(args)
		if (keys.length !== 1 || keys[0] !== "filter") throw new Unsupported(keys.length === 0 ? "filter" : "args")
		const filter = args.filter as {id?: {in?: unknown}} | null
		const clause = filter && typeof filter === "object" && Object.keys(filter).length === 1 ? filter.id : undefined
		if (!clause || typeof clause !== "object" || Object.keys(clause).length !== 1 || !Array.isArray(clause.in))
			throw new Unsupported("filter")
		if (clause.in.length === 0) throw new Unsupported("filter")
		ids = [...new Set(clause.in.map(uuidArg))].sort()
		if (ids.length > MAX_SPACES_PER_PAGE) throw new Unsupported("truncated")
	}
	if (!node.selectionSet) throw new Unsupported("selection")
	const fields: Plan["fields"] = []
	for (const field of fieldsOf(node.selectionSet, parsed.fragments)) {
		const name = field.name.value
		if (field.directives?.length) throw new Unsupported("directives")
		if (
			(name === "id" || name === "type" || name === "__typename") &&
			!field.arguments?.length &&
			!field.selectionSet
		)
			fields.push({key: responseKeyOf(field), name})
		else throw new Unsupported("selection")
	}
	return {root, key: responseKeyOf(node), fields, ids}
}

/** `Personal` / `DAO` from the card of a Space's system entity, or null when the kind relation is absent or ambiguous. */
export function spaceTypeOf(rows: readonly {prop: string; raw: string}[]): "PERSONAL" | "DAO" | null {
	let personal = false
	let dao = false
	for (const row of rows) {
		if (row.prop !== KIND_RELATION) continue
		if (row.raw === EOA) personal = true
		else if (row.raw === DAO) dao = true
	}
	return personal === dao ? null : personal ? "PERSONAL" : "DAO"
}

export function createSpaceResolver(deps: SpaceServeDeps, root: typeof SPACE_ROOT | typeof SPACES_ROOT): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: Plan
		try {
			plan = planSpaceServe(document, params.operationName, params.variables ?? {}, root)
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const items: Record<string, unknown>[] = []
		for (const id of plan.ids) {
			// A Space geo-db has not founded throws GeoDbError("unmapped_space"); the gateway names it as the reason.
			const rows = await askGeoDb(deps, conformingId("entity", id), id)
			if (signal.aborted) return decline("timeout")
			const type = spaceTypeOf(rows)
			if (type === null) return decline("empty")
			const item: Record<string, unknown> = {}
			for (const f of plan.fields) item[f.key] = f.name === "id" ? id : f.name === "type" ? type : spaceTypename()
			items.push(item)
		}
		return {ok: true, data: {[plan.key]: plan.root === SPACE_ROOT ? items[0] : items}}
	}
}

const spaceTypename = () => "Space"
