/**
 * Shadows of the root fields `search` and `searchConnection`: PostGraphile answers, geo-db's lexical search is
 * asked the same question in the background, and the two ranked lists are compared.
 *
 * Ranked lists are not equal or different, so the result is a measurement (searchMetrics.ts): of PostGraphile's
 * top ten ids, the share geo-db ranks in its top ten and in its top twenty, whether the first result is the
 * same entity, and each side's result count. Ids are compared in geo-db's id space (`conformingId`).
 *
 *   search(query: "..", spaceId: <uuid>, first: N) { id .. }
 *   searchConnection(query: "..", spaceId: <uuid>, first: N) { nodes { id .. } }
 *
 * Without `spaceId` gaia ranks every Space; geo-db's search takes several Spaces in one request and ranks across
 * them (docs/api/09 §1, a union scope scored against the summed statistics), so the shadow asks it ONE search
 * over every Space in the space map. A Space the map does not hold is not searched: the comparison measures
 * that gap too. A scoped search whose Space geo-db has not loaded counts `failed`.
 *
 * What cannot be mapped is counted `unsupported_selection` with a reason (metrics.ts), never guessed:
 *   - no_space: the space map names no Space (nothing to search over).
 *   - many_spaces: the space map holds more than MAX_EVERY_SPACE_SEARCH Spaces; geo-db searches at most 4096 in
 *     one request, and rankings of separate searches cannot be merged exactly (each is scored against its own
 *     scope). Up to 1024 Spaces are listed one by one; past that the search asks for every Space (`--space '*'`).
 *   - filter: a connection `filter` changes PostGraphile's set; geo-db's search takes only a type filter.
 *   - args: anything beside query, spaceId and first (offset, cursors, similarityThreshold), or a query the
 *     lexical search cannot take (under two characters, over 250, starting with `-`).
 *   - selection: `id` (or `nodes { id }`) is not selected, or a directive is used.
 *   - not_single_root: the field appears twice (aliases) or beside another root field.
 *
 * geo-db's `typeahead` mode (name, alias and description-prefix grams, typo tolerant) is the closest match to
 * gaia's name-similarity search and is the default; `bm25` is available through `GEODB_SEARCH_MODE`.
 * A multi-word query that finds fewer hits than the window is also searched word by word (searchWithWordFallback).
 * Other PostGraphile rules the comparison does not reproduce (the OpenSearch candidate ids, Claim and block
 * exclusions) are part of what is measured.
 */

import {type FieldNode, Kind, type SelectionSetNode, valueFromASTUntyped} from "graphql"

import {conformingId} from "../geodb/card"
import {type GeoDbCardClient, GeoDbError, MAX_LINE_SPACES} from "../geodb/client"
import {type GeoDbSearchClient, type GeoDbSearchMode, MAX_EVERY_SPACE_SEARCH, MAX_LISTED_SPACES} from "../geodb/search"
import {DEFAULT_EXCLUDED_TYPE_IDS} from "../search"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import {asRecord, type EntityShadowDeps, parseOperation, TYPES_PROPERTY, Unsupported} from "./entityShadow"
import type {FieldShadow} from "./gateway"
import {activateGatewayMetrics, recordGatewayShadow, recordGatewayUnsupported} from "./metrics"
import {
	activateSearchMetrics,
	GEODB_LIMIT,
	GEODB_OVERFETCH,
	recordSearchComparison,
	recordTypeFilter,
} from "./searchMetrics"

export const SEARCH_FIELD = "search"
export const SEARCH_CONNECTION_FIELD = "searchConnection"
export const SEARCH_FIELDS = [SEARCH_FIELD, SEARCH_CONNECTION_FIELD] as const
type SearchField = (typeof SEARCH_FIELDS)[number]

const MAX_QUERY_LENGTH = 250
const MIN_QUERY_LENGTH = 2
const MAX_FRAGMENT_DEPTH = 8
const SAMPLE = 5
/** A query's words searched one by one when the whole query fills too little of the window. */
export const MAX_QUERY_TOKENS = 4

export type SearchShadowDeps = Pick<EntityShadowDeps, "geoSpaceOf" | "log"> & {
	/** Every geo-db Space id (32 hex digits) in the space map, for a search without `spaceId`. */
	allGeoSpaces: () => Promise<string[]>
	search: GeoDbSearchClient
	mode?: GeoDbSearchMode
	/**
	 * Apply Postgres's type exclusion (DEFAULT_EXCLUDED_TYPE_IDS and Claim) to geo-db's answer: ask for GEODB_OVERFETCH
	 * hits, read their Type statements with `readCards`, drop the excluded, keep the first GEODB_LIMIT. Needs
	 * `readCards`; without it the shadow compares unfiltered. A failed card read skips the comparison.
	 */
	excludeTypes?: boolean
	readCards?: GeoDbCardClient["readCards"]
}

/** Type ids Postgres's search leaves out, in geo-db's id space. */
export const EXCLUDED_TYPES: ReadonlySet<string> = new Set(
	[...DEFAULT_EXCLUDED_TYPE_IDS, "96f859ef-a1ca-4b22-9372-c86ad58b694b"].map((id) => conformingId("entity", id)),
)

/**
 * `hits` without the entities that hold an excluded Type, in order, and at most `limit`. Reads the hits' cards in
 * batches of Spaces (a line holds MAX_LINE_SPACES). An entity whose card shows no Type is kept.
 */
export async function withoutExcludedTypes(
	readCards: NonNullable<GeoDbCardClient["readCards"]>,
	geoSpaceIds: string[],
	hits: string[],
	limit: number,
): Promise<string[]> {
	// Nothing to read: readCards refuses an empty id list (bad_request), which would abort an empty answer's comparison.
	if (hits.length === 0) return []
	const spaceChunks: string[][] = []
	for (let at = 0; at < geoSpaceIds.length; at += MAX_LINE_SPACES)
		spaceChunks.push(geoSpaceIds.slice(at, at + MAX_LINE_SPACES))
	const answers = await Promise.all(spaceChunks.map((chunk) => readCards({geoSpaceIds: chunk, entityIds: hits})))
	const excluded = new Set<string>()
	for (const row of answers.flat())
		if (row.prop === TYPES_PROPERTY && row.entity !== undefined && EXCLUDED_TYPES.has(row.raw))
			excluded.add(row.entity)
	return hits.filter((id) => !excluded.has(id)).slice(0, limit)
}

export type SearchPlan = {
	field: SearchField
	/** Response key of the field in `data`. */
	key: string
	query: string
	/** gaia space id, normalized; null when the search names none and ranks every Space. */
	spaceId: string | null
}

export type SearchAnalysis = {ok: true; plan: SearchPlan} | {ok: false; reason: Unsupported["reason"]}

const refuse = (reason: Unsupported["reason"]): never => {
	throw new Unsupported(reason)
}

function inlineFields(set: SelectionSetNode, fragments: Map<string, SelectionSetNode>, depth = 0): FieldNode[] {
	if (depth > MAX_FRAGMENT_DEPTH) return refuse("selection")
	const out: FieldNode[] = []
	for (const selection of set.selections) {
		if (selection.directives?.length) refuse("directives")
		if (selection.kind === Kind.FIELD) out.push(selection)
		else if (selection.kind === Kind.INLINE_FRAGMENT)
			out.push(...inlineFields(selection.selectionSet, fragments, depth + 1))
		else {
			const fragment = fragments.get(selection.name.value)
			if (!fragment) refuse("selection")
			else out.push(...inlineFields(fragment, fragments, depth + 1))
		}
	}
	return out
}

/** The response key of `id` in a selection, or throws when it is not selected as a plain field. */
function idKeyOf(set: SelectionSetNode | undefined, fragments: Map<string, SelectionSetNode>): string {
	if (!set) return refuse("selection")
	const id = inlineFields(set, fragments).find(
		(f) => f.name.value === "id" && !f.arguments?.length && !f.selectionSet,
	)
	return id ? (id.alias?.value ?? "id") : refuse("selection")
}

export function analyzeSearchOperation(
	field: SearchField,
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): SearchAnalysis & {idKey?: string; nodesKey?: string} {
	try {
		const parsed = parseOperation(document, operationName, variables)
		if (parsed.roots.length !== 1 || parsed.roots[0]?.name.value !== field) return refuse("not_single_root")
		const root = parsed.roots[0] as FieldNode
		const args: Record<string, unknown> = {}
		for (const arg of root.arguments ?? []) args[arg.name.value] = valueFromASTUntyped(arg.value, parsed.variables)
		if (args.filter !== undefined && args.filter !== null) refuse("filter")
		for (const name of Object.keys(args))
			if (!["query", "spaceId", "first", "filter"].includes(name)) refuse("args")
		if (args.first !== undefined && args.first !== null && (typeof args.first !== "number" || args.first < 1))
			refuse("args")
		const text = typeof args.query === "string" ? args.query.trim() : ""
		if (text.length < MIN_QUERY_LENGTH || text.length > MAX_QUERY_LENGTH || text.startsWith("-")) refuse("args")
		const unscoped = args.spaceId === undefined || args.spaceId === null
		if (!unscoped && (typeof args.spaceId !== "string" || !isValidUuid(args.spaceId))) refuse("args")
		const spaceId = unscoped ? null : normalizeUuid(args.spaceId as string)
		const key = root.alias?.value ?? field
		let idKey: string
		let nodesKey: string | undefined
		if (field === SEARCH_FIELD) idKey = idKeyOf(root.selectionSet, parsed.fragments)
		else {
			if (!root.selectionSet) return refuse("selection")
			const nodes = inlineFields(root.selectionSet, parsed.fragments).find(
				(f) => f.name.value === "nodes" && !f.arguments?.length,
			)
			if (!nodes) return refuse("selection")
			nodesKey = nodes.alias?.value ?? "nodes"
			idKey = idKeyOf(nodes.selectionSet, parsed.fragments)
		}
		return {ok: true, plan: {field, key, query: text, spaceId}, idKey, nodesKey}
	} catch (error) {
		if (error instanceof Unsupported) return {ok: false, reason: error.reason}
		throw error
	}
}

/** PostGraphile's ids in rank order, normalized, or null when the answer is not the expected shape. */
export function extractRankedIds(body: unknown, plan: SearchPlan, idKey: string, nodesKey?: string): string[] | null {
	const data = asRecord(asRecord(body)?.data)
	if (!data) return null
	let list = data[plan.key]
	if (nodesKey !== undefined) list = asRecord(list)?.[nodesKey]
	if (!Array.isArray(list)) return null
	const ids: string[] = []
	for (const item of list) {
		const id = asRecord(item)?.[idKey]
		if (typeof id !== "string" || !isValidUuid(id)) return null
		ids.push(conformingId("entity", id))
	}
	return [...new Set(ids)]
}

/** The words of a query that can be searched alone (two code points or more, not a flag), distinct, in order. */
export function queryTokens(text: string): string[] {
	const seen = new Set<string>()
	for (const word of text.split(/[\s,;|]+/u)) {
		if ([...word].length >= MIN_QUERY_LENGTH && !word.startsWith("-")) seen.add(word)
		if (seen.size === MAX_QUERY_TOKENS) break
	}
	return [...seen]
}

/**
 * geo-db's typeahead matches the WHOLE query as a prefix of a name (within two edits), so a two-word query whose
 * words belong to different names, or to a name that merely begins with the first, finds nothing, where
 * PostGraphile's search offers every name that matches some of the words. When the whole query returns fewer
 * than `limit` hits and holds several words, each word is searched too and its hits are appended after the whole
 * query's, once each, up to `limit`.
 */
export async function searchWithWordFallback(
	search: GeoDbSearchClient,
	args: Parameters<GeoDbSearchClient["searchLexical"]>[0],
): Promise<string[]> {
	const whole = await search.searchLexical(args)
	const words = queryTokens(args.text)
	if (whole.length >= args.limit || words.length < 2) return whole
	const hits = new Set(whole)
	for (const word of words) {
		if (hits.size >= args.limit) break
		for (const id of await search.searchLexical({...args, text: word})) hits.add(id)
	}
	return [...hits].slice(0, args.limit)
}

function createSearchShadow(deps: SearchShadowDeps, field: SearchField): FieldShadow {
	activateGatewayMetrics(field)
	activateSearchMetrics(field)
	const label = `gateway ${field} shadow`
	return async ({params, document, classification, response}) => {
		try {
			const analysis = analyzeSearchOperation(field, document, classification.operationName, params.variables)
			if (!analysis.ok) {
				recordGatewayShadow(field, "unsupported_selection")
				recordGatewayUnsupported(field, analysis.reason)
				return
			}
			const {plan} = analysis
			let body: unknown
			try {
				body = await response.json()
			} catch {
				return
			}
			const postgres = extractRankedIds(body, plan, analysis.idKey ?? "id", analysis.nodesKey)
			if (!postgres) {
				deps.log.info(`${label} skipped: PostGraphile answer not comparable`, {})
				return
			}
			const started = performance.now()
			const typeFilter = deps.excludeTypes === true && deps.readCards !== undefined
			let geodb: string[]
			let searched: string[] = []
			let spaces = 1
			try {
				let geoSpaceIds: string[]
				if (plan.spaceId === null) {
					geoSpaceIds = await deps.allGeoSpaces()
					spaces = geoSpaceIds.length
					if (geoSpaceIds.length === 0) {
						recordGatewayShadow(field, "unsupported_selection")
						recordGatewayUnsupported(field, "no_space")
						return
					}
					if (geoSpaceIds.length > MAX_EVERY_SPACE_SEARCH) {
						recordGatewayShadow(field, "unsupported_selection")
						recordGatewayUnsupported(field, "many_spaces")
						return
					}
				} else {
					const geoSpaceId = await deps.geoSpaceOf(plan.spaceId.replaceAll("-", ""))
					if (!geoSpaceId)
						throw new GeoDbError("unmapped_space", `space ${plan.spaceId} is not loaded in geo-db`)
					geoSpaceIds = [geoSpaceId]
				}
				searched = geoSpaceIds
				geodb = await searchWithWordFallback(deps.search, {
					geoSpaceIds,
					text: plan.query,
					mode: deps.mode ?? "typeahead",
					limit: typeFilter ? GEODB_OVERFETCH : GEODB_LIMIT,
					...(plan.spaceId === null && geoSpaceIds.length > MAX_LISTED_SPACES ? {everySpace: true} : {}),
				})
			} catch (error) {
				recordGatewayShadow(field, "failed")
				deps.log.warn(`${label} read failed`, {
					spaceId: plan.spaceId,
					reason: error instanceof GeoDbError ? error.reason : "error",
					error: String(error),
				})
				return
			}
			if (typeFilter && deps.readCards) {
				try {
					geodb = await withoutExcludedTypes(deps.readCards, searched, geodb, GEODB_LIMIT)
					recordTypeFilter(field, "applied")
				} catch (error) {
					const unavailable = error instanceof GeoDbError && error.reason === "bad_request"
					recordTypeFilter(field, unavailable ? "unavailable" : "error")
					deps.log.warn(`${label} type filter read failed`, {
						reason: error instanceof GeoDbError ? error.reason : "error",
						message: String(error),
					})
					return
				}
			} else if (deps.excludeTypes === true) recordTypeFilter(field, "unavailable")
			const verdict = recordSearchComparison(field, postgres, geodb)
			deps.log.info(`${label} compared`, {
				operationName: classification.operationName,
				spaceId: plan.spaceId,
				spaces,
				everySpace: plan.spaceId === null && spaces > MAX_LISTED_SPACES,
				mode: deps.mode ?? "typeahead",
				queryLength: plan.query.length,
				outcome: verdict.outcome,
				top10Overlap: verdict.top10Overlap,
				recallInGeoDb: verdict.recallInGeoDb,
				postgres: postgres.length,
				geodb: geodb.length,
				samplePostgres: postgres.slice(0, SAMPLE),
				sampleGeoDb: geodb.slice(0, SAMPLE),
				geoDbMs: Math.round(performance.now() - started),
			})
		} catch (error) {
			try {
				recordGatewayShadow(field, "failed")
				deps.log.warn(`${label} errored`, {error: String(error)})
			} catch {}
		}
	}
}

/** The shadows for the given search root fields, by field. Registers only their metric series. */
export function createSearchShadows(
	deps: SearchShadowDeps,
	fields: Iterable<SearchField> = SEARCH_FIELDS,
): Map<string, FieldShadow> {
	return new Map([...fields].map((field) => [field, createSearchShadow(deps, field)]))
}
