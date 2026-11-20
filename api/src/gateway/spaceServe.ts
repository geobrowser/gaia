import type {FieldNode} from "graphql"
import type {GeoCardRow} from "../geodb/card"
import {conformingId} from "../geodb/card"
import {MAX_LINE_SPACES, MAX_TYPE_SCAN} from "../geodb/client"
import type {GaiaIdsOf} from "../geodb/idMap"
import {askGeoDb, type CardDeps} from "../geodb/service"
import {entityRef, idPart, recordEntityId, textPart} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
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
 *   space(id: <uuid>) { id type address topicId __typename }
 *   spaces(filter: {id: {in: [<uuid> ..]}}) { id type address topicId __typename }
 *
 * (aliases, fragments and variables allowed, every field optional). `address` and `topicId` are the keyed records the
 * feeder writes into the `governance` system Space (spaceAddress, spaceTopic: one record per Space, record id derived
 * from the Space id): `address` is the record's text exactly as Postgres holds it (EIP-55 after a create, lowercase after
 * an update), `topicId` is its reference mapped back through the reverse id map. A missing record, a record without an
 * address, two values in one column or a topic the map does not know decline (`empty`, `legacy_id`); the governance Space
 * not being founded in this process declines `not_enabled`. No record for a topic Postgres holds as null is told from a
 * Space the feeder has not reached, so only a record that exists and holds no topic answers null. Everything else declines with the reason
 * PostGraphile then answers it for (gaia_api_gateway_serve_fallback_total):
 * - `spaces` with no argument (the every-Space list) is read as two type scans across every Space geo-db holds
 *   (`allGeoSpaces`), one for EoaSpace (`PERSONAL`) and one for DaoSpace (`DAO`), each mapped back through the reverse id
 *   map, and served in PostGraphile's primary-key order. It answers only when it is certain to be the whole list: neither
 *   scan filled its window (`truncated`), the Spaces fit one line (`many_spaces`, past `MAX_LINE_SPACES`), the reads are
 *   switched on (`not_enabled`, `no_space`), every typed entity maps back to a gaia id (`legacy_id`) that is a Space
 *   geo-db holds (`unmapped_space`), no Space is typed as both kinds, and at least one Space is found (`empty`: PostgreSQL
 *   may hold Spaces geo-db has not been fed). `address` and `topicId` need one record read each per Space, so they are served
 *   only for a page of `MAX_SPACES_PER_PAGE` or fewer (else `truncated`); `id` and `type` come from the scans alone.
 * - `page`, `topic`, members, voting settings and the rest decline `selection`: joins and keyed records not read here.
 *   A filter other than the id list or an address (`members some`) or any paging declines `filter` / `args`.
 * - `spaces(filter: {address: {equalTo | equalToInsensitive: "0x<40 hex>"}})` reads the feeder's reverse index (spaceByAddress,
 *   keyed by the lowercase address, GEODB_SPACE_BY_ADDRESS) for the one Space it names, then that Space's own address record,
 *   and answers only when the two agree (`spaceOfAddress`): the index can be stale after an address change. No index record,
 *   two Spaces, a Space the reverse map does not know or a disagreeing forward record decline `empty` / `legacy_id`.
 *   PostgreSQL may hold a second Space with the same address; the shadow, not this resolver, would show that.
 * - A Space geo-db has not founded declines `unmapped_space`, and a Space whose system entity is not loaded yet (or
 *   whose kind relation is absent) declines `empty`: PostgreSQL may hold either, so "no such Space" is never inferred.
 *   Because every asked Space must be held, a `spaces` page is the whole answer, in PostGraphile's primary-key order
 *   (undashed lowercase hex sorts like the id's bytes).
 */

const ADDRESS_RE = /^0x[0-9a-fA-F]{40}$/

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

/** The feeder's record pins (geodb-feeder/src/recordKinds.ts); the test holds them to that file. */
export const SPACE_ADDRESS_KIND = "44433f0b01fd46a18317285f2236fc93"
export const SPACE_ADDRESS_COLUMNS = {
	space: "9c7f59c747a544fd844cbffdeb5bbc48",
	address: "a584f4100b8d47428afd4242371c44c8",
} as const
export const SPACE_TOPIC_KIND = "c74881c2d1d24ae5ad1e34d5c1c0535b"
export const SPACE_TOPIC_COLUMNS = {
	space: "0249560173894904a354428215129068",
	topic: "02b0149f64744e4d9ba1574e36e066ad",
} as const

/** The reverse index (feeder spaceByAddress): keyed by the lowercase address, one column referencing the Space. */
export const SPACE_BY_ADDRESS_KIND = "87af51da7d7149f4abac0f58bcca4338"
export const SPACE_BY_ADDRESS_COLUMNS = {space: "510ac26058a54a828ac224f97fe25220"} as const

export type SpaceServeDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {
	/** The geo-db id of the feeder's `governance` system Space, where the address and topic records live. */
	systemSpaceOf?: AnySystemSpaceLookup
	/** geo-db id -> gaia id, for the topic a record references; without it `topicId` declines. */
	gaiaIdsOf?: GaiaIdsOf
	/** Unfiltered `spaces`: the type scan read is switched on (GEODB_TYPE_SCAN_SHADOW=on). */
	typeScan?: boolean
	/** Unfiltered `spaces`: a scan may name several Spaces (GEODB_MULTI_SPACE_READS=on). */
	multiSpace?: boolean
	/** Unfiltered `spaces`: every geo-db Space id the space map holds (the Spaces the scans read). */
	allGeoSpaces?: () => Promise<string[]>
}

type Plan = {
	root: typeof SPACE_ROOT | typeof SPACES_ROOT
	key: string
	fields: {key: string; name: "id" | "type" | "__typename" | "address" | "topicId"}[]
	/** The Spaces asked for, sorted; null for the unfiltered list and for an address lookup. */
	ids: string[] | null
	/** `spaces(filter: {address})`: the address asked for and whether the match ignores case. */
	address?: {value: string; insensitive: boolean}
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
	let ids: string[] | null
	let address: Plan["address"]
	if (root === SPACES_ROOT && Object.keys(args).length === 0) {
		ids = null
	} else if (root === SPACE_ROOT) {
		if (Object.keys(args).length !== 1 || !("id" in args)) throw new Unsupported("args")
		ids = [uuidArg(args.id)]
	} else {
		const keys = Object.keys(args)
		if (keys.length !== 1 || keys[0] !== "filter") throw new Unsupported("args")
		const filter = args.filter as {id?: {in?: unknown}} | null
		const byAddress =
			filter && typeof filter === "object" && Object.keys(filter).length === 1
				? (filter as {address?: unknown}).address
				: undefined
		if (byAddress !== undefined) {
			const a = byAddress as Record<string, unknown> | null
			const op = a && typeof a === "object" && Object.keys(a).length === 1 ? Object.keys(a)[0] : undefined
			const value = op ? a?.[op] : undefined
			if (
				(op !== "equalTo" && op !== "equalToInsensitive") ||
				typeof value !== "string" ||
				!ADDRESS_RE.test(value)
			)
				throw new Unsupported("filter")
			address = {value, insensitive: op === "equalToInsensitive"}
		}
		const clause = address
			? undefined
			: filter && typeof filter === "object" && Object.keys(filter).length === 1
				? filter.id
				: undefined
		if (
			!address &&
			(!clause || typeof clause !== "object" || Object.keys(clause).length !== 1 || !Array.isArray(clause.in))
		)
			throw new Unsupported("filter")
		if (address) ids = null
		else {
			const list = (clause as {in: unknown[]}).in
			if (list.length === 0) throw new Unsupported("filter")
			ids = [...new Set(list.map(uuidArg))].sort()
			if (ids.length > MAX_SPACES_PER_PAGE) throw new Unsupported("truncated")
		}
	}
	if (!node.selectionSet) throw new Unsupported("selection")
	const fields: Plan["fields"] = []
	for (const field of fieldsOf(node.selectionSet, parsed.fragments)) {
		const name = field.name.value
		if (field.directives?.length) throw new Unsupported("directives")
		if (
			(name === "id" || name === "type" || name === "__typename" || name === "address" || name === "topicId") &&
			!field.arguments?.length &&
			!field.selectionSet
		)
			fields.push({key: responseKeyOf(field), name})
		else throw new Unsupported("selection")
	}
	return {root, key: responseKeyOf(node), fields, ids, ...(address ? {address} : {})}
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

/**
 * One keyed record's current column from its card: undefined when the record is absent or is not this Space's (no key
 * column holding the Space's reference), null when the record holds no value in the column, else the value. Throws
 * `Ambiguous` when the column holds two different values.
 */
class Ambiguous extends Error {}
function recordColumn(
	rows: readonly GeoCardRow[],
	keyProp: string,
	space: string,
	prop: string,
): string | null | undefined {
	if (!rows.some((r) => r.prop === keyProp && r.raw === entityRef(space))) return undefined
	const held = [...new Set(rows.filter((r) => r.prop === prop).map((r) => r.raw))]
	if (held.length > 1) throw new Ambiguous()
	return held[0] ?? null
}

/**
 * Every Space and its kind, sorted by id, from one type scan a kind across the Spaces geo-db holds; or the decline.
 * The scan returns each Space's system entity (geo-db ids): it is named back through the reverse id map and must be a
 * Space geo-db holds, which also drops a stray entity typed with a Space kind in some other Space.
 */
async function listSpaces(
	deps: SpaceServeDeps,
	signal: AbortSignal,
): Promise<ResolveResult | [string, "PERSONAL" | "DAO"][]> {
	const readTypeScan = deps.geoDb.readTypeScan?.bind(deps.geoDb)
	if (!deps.typeScan || !readTypeScan) return decline("not_enabled")
	if (!deps.allGeoSpaces || !deps.multiSpace) return decline("no_space")
	if (!deps.gaiaIdsOf) return decline("legacy_id")
	const spaces = [...new Set(await deps.allGeoSpaces())]
	if (spaces.length === 0) return decline("empty")
	if (spaces.length > MAX_LINE_SPACES) return decline("many_spaces")
	const [personal, dao] = await Promise.all(
		[EOA, DAO].map((typeId) => readTypeScan({geoSpaceIds: spaces, typeIds: [typeId], first: MAX_TYPE_SCAN})),
	)
	if (signal.aborted) return decline("timeout")
	// A scan that filled its window may have more, and the page would no longer be the whole list.
	if (!personal || !dao || personal.length >= MAX_TYPE_SCAN || dao.length >= MAX_TYPE_SCAN)
		return decline("truncated")
	const named = await deps.gaiaIdsOf([...new Set([...personal, ...dao])])
	if (signal.aborted) return decline("timeout")
	const kinds = new Map<string, "PERSONAL" | "DAO">()
	for (const [list, kind] of [
		[personal, "PERSONAL"],
		[dao, "DAO"],
	] as const)
		for (const entity of list) {
			const id = named.get(entity)
			if (!id) return decline("legacy_id")
			if (!(await deps.geoSpaceOf(id))) return decline("unmapped_space")
			// One Space typed as both kinds is not an answer (spaceTypeOf declines the same card).
			if (kinds.has(id) && kinds.get(id) !== kind) return decline("empty")
			kinds.set(id, kind)
		}
	// No Space found at all means geo-db has not been fed them, not that PostgreSQL holds none.
	if (kinds.size === 0) return decline("empty")
	// PostGraphile's order with no orderBy is the primary key, the id's bytes: undashed lowercase hex sorts the same.
	return [...kinds].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
}

/**
 * The Space whose address is asked, from the reverse index and the Space's own address record, or the decline. The index
 * is never answered from alone: an address that changed leaves the old address's record pointing at the Space, and a
 * second Space taking an address replaces the first's entry, so the Space's own (forward) record must hold the address
 * asked for (case-insensitively, or exactly for `equalTo`). Anything else declines `empty`: PostgreSQL may hold it.
 */
async function spaceOfAddress(
	deps: SpaceServeDeps,
	governance: string,
	ask: {value: string; insensitive: boolean},
): Promise<ResolveResult | string> {
	const index = await deps.geoDb.readCard({
		geoSpaceId: governance,
		entityId: recordEntityId(SPACE_BY_ADDRESS_KIND, [textPart(ask.value.toLowerCase())]),
	})
	const refs = [...new Set(index.filter((r) => r.prop === SPACE_BY_ADDRESS_COLUMNS.space).map((r) => r.raw))]
	if (refs.length !== 1) return decline("empty")
	if (!deps.gaiaIdsOf) return decline("legacy_id")
	const space = (await deps.gaiaIdsOf(refs))?.get(refs[0] as string)
	if (!space) return decline("legacy_id")
	const forward = await deps.geoDb.readCard({
		geoSpaceId: governance,
		entityId: recordEntityId(SPACE_ADDRESS_KIND, [idPart(space)]),
	})
	let held: string | null | undefined
	try {
		held = recordColumn(forward, SPACE_ADDRESS_COLUMNS.space, space, SPACE_ADDRESS_COLUMNS.address)
	} catch (error) {
		if (error instanceof Ambiguous) return decline("empty")
		throw error
	}
	if (!held) return decline("empty")
	if (ask.insensitive ? held.toLowerCase() !== ask.value.toLowerCase() : held !== ask.value) return decline("empty")
	return space
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
		const wantAddress = plan.fields.some((f) => f.name === "address")
		const wantTopic = plan.fields.some((f) => f.name === "topicId")
		let governance: string | null = null
		if (wantAddress || wantTopic || plan.address) {
			governance = (await deps.systemSpaceOf?.("governance")) ?? null
			if (!governance) return decline("not_enabled")
		}
		let asked: string[] | null = plan.ids
		if (plan.address && governance) {
			const found = await spaceOfAddress(deps, governance, plan.address)
			if (typeof found !== "string") return found
			asked = [found]
		}
		const items: Record<string, unknown>[] = []
		let known: Map<string, "PERSONAL" | "DAO"> | null = null
		if (asked === null) {
			const listed = await listSpaces(deps, signal)
			if (!Array.isArray(listed)) return listed
			if ((wantAddress || wantTopic) && listed.length > MAX_SPACES_PER_PAGE) return decline("truncated")
			known = new Map(listed)
		}
		for (const id of asked ?? [...(known?.keys() ?? [])]) {
			let type: "PERSONAL" | "DAO" | null
			if (known) type = known.get(id) ?? null
			else {
				// A Space geo-db has not founded throws GeoDbError("unmapped_space"); the gateway names it as the reason.
				const rows = await askGeoDb(deps, conformingId("entity", id), id)
				if (signal.aborted) return decline("timeout")
				type = spaceTypeOf(rows)
			}
			if (type === null) return decline("empty")
			let address: string | null | undefined
			let topic: string | null | undefined
			try {
				if (wantAddress && governance) {
					const record = recordEntityId(SPACE_ADDRESS_KIND, [idPart(id)])
					const held = await deps.geoDb.readCard({geoSpaceId: governance, entityId: record})
					address = recordColumn(held, SPACE_ADDRESS_COLUMNS.space, id, SPACE_ADDRESS_COLUMNS.address)
					// spaces.address is never null in PostgreSQL: a record without it is a gap, not an answer.
					if (!address) return decline("empty")
				}
				if (wantTopic && governance) {
					const record = recordEntityId(SPACE_TOPIC_KIND, [idPart(id)])
					const held = await deps.geoDb.readCard({geoSpaceId: governance, entityId: record})
					topic = recordColumn(held, SPACE_TOPIC_COLUMNS.space, id, SPACE_TOPIC_COLUMNS.topic)
					if (topic === undefined) return decline("empty")
					if (topic !== null) {
						// The record references the topic by its geo-db id; a legacy id is held under a hash, so ask the map.
						if (!deps.gaiaIdsOf) return decline("legacy_id")
						const gaia = (await deps.gaiaIdsOf([topic])).get(topic)
						if (!gaia) return decline("legacy_id")
						topic = gaia
					}
				}
			} catch (error) {
				if (error instanceof Ambiguous) return decline("empty")
				throw error
			}
			if (signal.aborted) return decline("timeout")
			const item: Record<string, unknown> = {}
			for (const f of plan.fields)
				item[f.key] =
					f.name === "id"
						? id
						: f.name === "type"
							? type
							: f.name === "address"
								? address
								: f.name === "topicId"
									? topic
									: spaceTypename()
			items.push(item)
		}
		return {ok: true, data: {[plan.key]: plan.root === SPACE_ROOT ? items[0] : items}}
	}
}

const spaceTypename = () => "Space"
