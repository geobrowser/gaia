import {Kind, type OperationDefinitionNode} from "graphql"
import type {GaiaIdsOf} from "../geodb/idMap"
import {
	type GeoDbSearchClient,
	type GeoDbSearchMode,
	lexicalBusyRefusal,
	lexicalDefiniteRefusal,
	MAX_EVERY_SPACE_SEARCH,
	MAX_LISTED_SPACES,
} from "../geodb/search"
import type {CardDeps} from "../geodb/service"
import {argsOf, fieldsOf, isPlainField, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"
import {queryTokens, SEARCH_FIELD, type SearchField, withoutExcludedTypes} from "./searchShadow"

/**
 * The geo-db resolver for the `search` and `searchConnection` root fields (GRAPHQL_GATEWAY_FIELDS `search=on`,
 * `searchConnection=on`, and GEODB_SEARCH_SERVE=on; both opt-ins are off by default).
 *
 * It answers the shape the search shadow compares (searchShadow.ts):
 *
 *   search(query: "..", spaceId: <uuid>, first: n) { id __typename }
 *   searchConnection(query: "..", spaceId: <uuid>, first: n) { nodes { id __typename } }
 *
 * Ranking: geo-db's bm25 lexical search over the named Space, or over every Space in the space map when none is
 * named (one ranking, docs/api/09 §1). When bm25 leaves fewer than `first` rows (after Postgres's type exclusion),
 * a typeahead search on the query's FINAL token is appended below the bm25 rows, each entity once, bm25 rows first
 * (ISS-565-842 point 5, option b): bm25 matches whole terms, so a query typed up to its last, partial word finds
 * nothing there, and the typeahead leg is what a user in the middle of typing needs.
 *
 * Everything else declines with a named reason (gaia_api_gateway_serve_fallback_total) and PostGraphile answers:
 *   - not_single_root, directives: the field is not alone, or carries a directive.
 *   - args: anything beside query, spaceId and first (offset, cursors, similarityThreshold); no `first`, or one past
 *     100 (what one geo-db search returns); a query the lexical search cannot take (under two characters, over 250,
 *     starting with `-`); a spaceId that is not a UUID. `filter` declines `filter`.
 *   - selection: anything beside `id` and `__typename` (a card needs a read this resolver does not make), or on a
 *     connection anything beside `nodes`.
 *   - not_enabled: GEODB_SEARCH_SERVE is off, or the card read that applies the type exclusion is missing.
 *   - no_space, many_spaces, unmapped_space: the space map names no Space, more than geo-db searches in one request,
 *     or not the named one.
 *   - empty: geo-db found nothing; PostgreSQL's similarity search may still.
 *   - legacy_id: a hit the reverse id map (geodb/idMap.ts) cannot place; without the map nothing is served.
 *   - definite_refusal: geo-db refused the query text by design (`DefiniteRefusal`, e.g. folds under 2 code points).
 *   - refused_busy: geo-db shed the read because its every-Space typeahead share of the worker pool was taken (load, not a fault).
 *   - timeout, overloaded, client_failed, bad_request: geo-db failing, passed through for the gateway to count.
 *
 * Postgres's rules the answer does not reproduce (OpenSearch candidate ids, Claim and block exclusions beyond the
 * type exclusion) are part of what the shadow measures; this resolver is for the flag to be flipped only after the
 * judged verdicts (gaia_api_gateway_shadow_search_judged_total) say so.
 */

export type SearchServeDeps = Pick<CardDeps, "geoSpaceOf"> & {
	geoSpaceOf: CardDeps["geoSpaceOf"] & {all(): Promise<string[]>}
	search: GeoDbSearchClient
	/** Applies the type exclusion of Postgres's search. Absent: never serve. */
	readCards?: Parameters<typeof withoutExcludedTypes>[0]
	/** The reverse id map (geo-db id -> gaia id). Absent: never serve. */
	gaiaIdsOf?: GaiaIdsOf
	/** GEODB_SEARCH_SERVE=on, or a dynamic check (e.g. runtime readSourceSwitch mode). */
	enabled: boolean | (() => boolean)
}

type SearchPlan = {
	field: SearchField
	key: string
	/** Response key of `nodes` on a connection. */
	nodesKey: string | null
	fields: {key: string; name: "id" | "__typename"}[]
	query: string
	/** gaia space id, normalized; null when the search names none and ranks every Space. */
	spaceId: string | null
	first: number
}

const MAX_FIRST = 100
const MIN_QUERY_LENGTH = 2
const MAX_QUERY_LENGTH = 250
const MIN_ASK = 30
const BM25: GeoDbSearchMode = "bm25"
const given = (value: unknown) => value !== undefined && value !== null
const decline = (reason: string): ResolveResult => ({ok: false, reason})

function leafFields(set: Parameters<typeof fieldsOf>[0], fragments: Parameters<typeof fieldsOf>[1]) {
	const out: SearchPlan["fields"] = []
	for (const field of fieldsOf(set, fragments)) {
		const name = field.name.value
		if ((name !== "id" && name !== "__typename") || !isPlainField(field)) throw new Unsupported("selection")
		out.push({key: responseKeyOf(field), name})
	}
	if (out.length === 0) throw new Unsupported("selection")
	return out
}

/** Plans the supported shape, or throws `Unsupported` with the reason. */
export function planSearchServe(
	field: SearchField,
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): SearchPlan {
	const parsed = parseOperation(document, operationName, variables)
	const operation = document.definitions.find(
		(d): d is OperationDefinitionNode =>
			d.kind === Kind.OPERATION_DEFINITION && (operationName ? d.name?.value === operationName : true),
	)
	// A root `__typename` beside the field is dropped by parseOperation; it would be missing from the answer.
	if (!operation || operation.selectionSet.selections.length !== 1) throw new Unsupported("not_single_root")
	const [root] = parsed.roots
	if (parsed.roots.length !== 1 || !root || root.name.value !== field) throw new Unsupported("not_single_root")
	const args = argsOf(root, parsed.variables)
	if (given(args.filter)) throw new Unsupported("filter")
	for (const [name, value] of Object.entries(args))
		if (given(value) && !["query", "spaceId", "first"].includes(name)) throw new Unsupported("args")
	const first = args.first
	if (typeof first !== "number" || !Number.isInteger(first) || first < 1 || first > MAX_FIRST)
		throw new Unsupported("args")
	const query = typeof args.query === "string" ? args.query.trim() : ""
	if (query.length < MIN_QUERY_LENGTH || query.length > MAX_QUERY_LENGTH || query.startsWith("-"))
		throw new Unsupported("args")
	const spaceId = given(args.spaceId) ? uuidArg(args.spaceId) : null
	if (!root.selectionSet) throw new Unsupported("selection")
	let fields: SearchPlan["fields"]
	let nodesKey: string | null = null
	if (field === SEARCH_FIELD) fields = leafFields(root.selectionSet, parsed.fragments)
	else {
		const children = fieldsOf(root.selectionSet, parsed.fragments)
		const [nodes] = children
		if (
			children.length !== 1 ||
			!nodes ||
			nodes.name.value !== "nodes" ||
			nodes.arguments?.length ||
			!nodes.selectionSet
		)
			throw new Unsupported("selection")
		nodesKey = responseKeyOf(nodes)
		fields = leafFields(nodes.selectionSet, parsed.fragments)
	}
	return {field, key: responseKeyOf(root), nodesKey, fields, query, spaceId, first}
}

/** The query's last word, when it can be searched alone. */
const finalToken = (text: string): string | null => {
	const words = text.split(/[\s,;|]+/u).filter((w) => w.length > 0)
	const last = words[words.length - 1]
	return last !== undefined && queryTokens(last).length === 1 ? last : null
}

export function createSearchResolver(field: SearchField, deps: SearchServeDeps): FieldResolver {
	return async ({params, document, signal}) => {
		let plan: SearchPlan
		try {
			plan = planSearchServe(field, document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const {readCards, gaiaIdsOf} = deps
		const enabled = typeof deps.enabled === "function" ? deps.enabled() : deps.enabled
		if (!enabled || !readCards) return decline("not_enabled")
		if (!gaiaIdsOf) return decline("legacy_id")
		let geoSpaceIds: string[]
		if (plan.spaceId === null) {
			geoSpaceIds = await deps.geoSpaceOf.all()
			if (geoSpaceIds.length === 0) return decline("no_space")
			if (geoSpaceIds.length > MAX_EVERY_SPACE_SEARCH) return decline("many_spaces")
		} else {
			const geo = await deps.geoSpaceOf(plan.spaceId.replaceAll("-", ""))
			if (!geo) return decline("unmapped_space")
			geoSpaceIds = [geo]
		}
		const everySpace = plan.spaceId === null && geoSpaceIds.length > MAX_LISTED_SPACES
		const ask = Math.min(MAX_FIRST, Math.max(MIN_ASK, plan.first * 3))
		const searchArgs = {geoSpaceIds, limit: ask, ...(everySpace ? {everySpace: true} : {})}
		const runLexical = async (text: string, mode: GeoDbSearchMode): Promise<string[] | ResolveResult> => {
			try {
				return await deps.search.searchLexical({...searchArgs, text, mode})
			} catch (error) {
				if (lexicalDefiniteRefusal(error)) return decline("definite_refusal")
				if (lexicalBusyRefusal(error)) return decline("refused_busy")
				throw error
			}
		}
		// Enough over-fetch that dropping the excluded types still leaves a page.
		const bm25 = await runLexical(plan.query, BM25)
		if (!Array.isArray(bm25)) return bm25
		if (signal.aborted) return decline("timeout")
		const hits = await withoutExcludedTypes(readCards, geoSpaceIds, bm25, plan.first)
		const token = finalToken(plan.query)
		if (hits.length < plan.first && token !== null) {
			const typeahead = await runLexical(token, "typeahead")
			if (!Array.isArray(typeahead)) return typeahead
			if (signal.aborted) return decline("timeout")
			const seen = new Set(bm25)
			const fresh = typeahead.filter((id) => !seen.has(id))
			const kept = await withoutExcludedTypes(readCards, geoSpaceIds, fresh, plan.first)
			hits.push(...kept.slice(0, plan.first - hits.length))
		}
		if (hits.length === 0) return decline("empty")
		const gaia = await gaiaIdsOf(hits)
		if (signal.aborted) return decline("timeout")
		const ids: string[] = []
		for (const id of hits) {
			const mapped = gaia.get(id)
			if (!mapped) return decline("legacy_id")
			ids.push(mapped)
		}
		const rows = ids.map((id) =>
			Object.fromEntries(plan.fields.map((f) => [f.key, f.name === "id" ? id : "Entity"])),
		)
		return {ok: true, data: {[plan.key]: plan.nodesKey === null ? rows : {[plan.nodesKey]: rows}}}
	}
}
