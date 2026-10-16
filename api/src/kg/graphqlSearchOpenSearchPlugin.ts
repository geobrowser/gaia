/**
 * Yoga plugin that takes the candidate ids for GraphQL `search` / `searchConnection` from
 * OpenSearch instead of from the pg_trgm scan inside `public.search`.
 *
 * `public.search` costs 9-10 s per call whatever `first` is; the REST `/search` route answers the
 * same question from OpenSearch in ~0.5 s. Migration 0110 gave the function an `ids uuid[]` argument
 * that short-circuits the trigram scan and returns those entities in the order given. This plugin
 * asks OpenSearch for the ids and writes them into the field's arguments before execution, so the
 * result is still real `entities` rows: nested selections, the connection `filter`, `offset`,
 * `first` and cursor pagination all keep working natively.
 *
 * Behaviour:
 *   - Only root `search` / `searchConnection` fields of query operations are touched. A field that
 *     already passes `ids`, or has no usable query string, is left alone.
 *   - OpenSearch is asked for offset + first + slack ids (more when a `filter` is present, because
 *     SQL applies the filter after, so recall needs headroom), capped at the client's page size.
 *   - The REST route's default type exclusions apply, plus Claims, unless the caller's `filter`
 *     names those types. Without the Claim exclusion a query with no exact-name match ranks long
 *     claim sentences containing the query first, and entity-linking callers link to claims.
 *   - `filter.typeIds.in` and a single `filter.spaceIds` are passed to OpenSearch as hints for
 *     recall only; SQL still enforces the filter.
 *   - On an OpenSearch error or timeout `ids` is NOT injected, so the legacy body runs (the
 *     behaviour before this plugin). One warning per request.
 *   - `GRAPHQL_SEARCH_OPENSEARCH=0` turns the plugin off. Default on.
 *
 * The parsed document may be shared across requests by Yoga's parser cache, so it is never
 * mutated: a modified copy is built (graphql `visit` is copy-on-write) and handed to the executor.
 */
import {
	type ArgumentNode,
	type DocumentNode,
	type FieldNode,
	type FragmentDefinitionNode,
	Kind,
	type SelectionSetNode,
	valueFromASTUntyped,
	visit,
} from "graphql"
import type {Plugin} from "graphql-yoga"
import {DEFAULT_EXCLUDED_TYPE_IDS} from "../search"
import type {SearchClient, SearchQuery} from "../services/search"
import {log} from "../services/telemetry"
import {isValidUuid, normalizeUuid} from "../utils/uuid"

const SEARCH_FIELD_NAMES: ReadonlySet<string> = new Set(["search", "searchConnection"])

/** Claim type entity (dashed, as in migration 0103). Claims are excluded unless asked for. */
export const CLAIM_TYPE_ID = "96f859ef-a1ca-4b22-9372-c86ad58b694b"

/** Ids requested beyond offset + first, so a few dropped candidates do not leave a short page. */
const FETCH_SLACK = 20
/** Minimum ids requested when a `filter` is present: SQL filters after OpenSearch ranked. */
const FILTERED_MIN_FETCH = 100
/** OpenSearchClient clamps `limit` to its MAX_PAGE_SIZE (100); asking for more buys nothing. */
const MAX_FETCH = 100
/** The REST route rejects more than this many type ids. */
const MAX_TYPE_HINTS = 10
const DEFAULT_TIMEOUT_MS = 3000

let searchClient: SearchClient | null = null

/** main.ts calls this once the OpenSearch client exists; the plugin is inert until then. */
export function setGraphqlSearchClient(client: SearchClient | null): void {
	searchClient = client
}

function enabled(): boolean {
	return process.env.GRAPHQL_SEARCH_OPENSEARCH !== "0"
}

function timeoutMs(): number {
	const n = Number(process.env.GRAPHQL_SEARCH_OPENSEARCH_TIMEOUT_MS)
	return Number.isFinite(n) && n > 0 ? n : DEFAULT_TIMEOUT_MS
}

// ---------------------------------------------------------------------------
// Pure helpers (unit tested)
// ---------------------------------------------------------------------------

export type SearchFieldParams = {
	query: string
	spaceId?: string
	first?: number
	offset?: number
	/** `after` / `before` / `last` present: the page position is not knowable from offset + first. */
	cursorPaged: boolean
	filter?: Record<string, unknown>
}

function asNonNegativeInt(v: unknown): number | undefined {
	return typeof v === "number" && Number.isFinite(v) && v >= 0 ? Math.floor(v) : undefined
}

/** How many ids to request from OpenSearch for one field. */
export function computeFetchSize(p: Pick<SearchFieldParams, "first" | "offset" | "cursorPaged" | "filter">): number {
	if (p.cursorPaged || p.first === undefined) return MAX_FETCH
	const want = (p.offset ?? 0) + p.first + FETCH_SLACK
	const sized = p.filter && Object.keys(p.filter).length > 0 ? Math.max(want, FILTERED_MIN_FETCH) : want
	return Math.min(Math.max(sized, 1), MAX_FETCH)
}

/** Every UUID-looking string anywhere under `value`. */
function collectUuids(value: unknown, out: Set<string>): void {
	if (typeof value === "string") {
		if (isValidUuid(value)) out.add(normalizeUuid(value))
	} else if (Array.isArray(value)) {
		for (const v of value) collectUuids(v, out)
	} else if (value && typeof value === "object") {
		for (const v of Object.values(value)) collectUuids(v, out)
	}
}

function uuidList(value: unknown): string[] | undefined {
	if (!Array.isArray(value)) return undefined
	const out = value.filter((v): v is string => typeof v === "string" && isValidUuid(v)).map((v) => normalizeUuid(v))
	return out.length === value.length ? out : undefined
}

/** Translate one resolved search field into an OpenSearch query, or null to leave it on SQL. */
export function buildOpenSearchQuery(p: SearchFieldParams): SearchQuery | null {
	const query = p.query.trim()
	if (query.length === 0) return null

	const filter = p.filter
	const typeFilter = filter?.typeIds
	const spaceFilter = filter?.spaceIds

	// Types the caller mentions anywhere in `typeIds` are not excluded by default.
	const mentioned = new Set<string>()
	collectUuids(typeFilter, mentioned)

	const include =
		typeFilter && typeof typeFilter === "object" ? uuidList((typeFilter as {in?: unknown}).in) : undefined
	const typeIds = include && include.length > 0 && include.length <= MAX_TYPE_HINTS ? include : undefined

	const excluded = [...DEFAULT_EXCLUDED_TYPE_IDS, CLAIM_TYPE_ID]
		.map((id) => normalizeUuid(id))
		.filter((id, i, all) => all.indexOf(id) === i && !mentioned.has(id))

	// A space hint: the explicit `spaceId` argument, else a filter naming exactly one space.
	let spaceId: string | undefined
	if (p.spaceId && isValidUuid(p.spaceId)) {
		spaceId = normalizeUuid(p.spaceId)
	} else if (spaceFilter && typeof spaceFilter === "object") {
		const f = spaceFilter as {anyEqualTo?: unknown; in?: unknown}
		const ids =
			typeof f.anyEqualTo === "string" && isValidUuid(f.anyEqualTo)
				? [normalizeUuid(f.anyEqualTo)]
				: uuidList(f.in)
		if (ids && ids.length === 1) spaceId = ids[0]
	}

	return {
		query,
		scope: spaceId ? "SPACE_SINGLE" : "GLOBAL",
		limit: computeFetchSize(p),
		...(spaceId && {space_id: spaceId}),
		...(typeIds && {type_ids: typeIds}),
		...(excluded.length > 0 && {exclude_type_ids: excluded}),
	}
}

// ---------------------------------------------------------------------------
// AST handling
// ---------------------------------------------------------------------------

/** Root-level search fields of query operations, following top-level fragments. */
export function findRootSearchFields(document: DocumentNode): FieldNode[] {
	const fragments: Record<string, FragmentDefinitionNode> = {}
	for (const def of document.definitions) {
		if (def.kind === Kind.FRAGMENT_DEFINITION) fragments[def.name.value] = def
	}
	const found: FieldNode[] = []
	const seen = new Set<string>()
	const walk = (set: SelectionSetNode): void => {
		for (const sel of set.selections) {
			if (sel.kind === Kind.FIELD) {
				if (SEARCH_FIELD_NAMES.has(sel.name.value)) found.push(sel)
			} else if (sel.kind === Kind.INLINE_FRAGMENT) {
				walk(sel.selectionSet)
			} else if (sel.kind === Kind.FRAGMENT_SPREAD) {
				const frag = fragments[sel.name.value]
				if (frag && !seen.has(frag.name.value)) {
					seen.add(frag.name.value)
					walk(frag.selectionSet)
				}
			}
		}
	}
	for (const def of document.definitions) {
		if (def.kind === Kind.OPERATION_DEFINITION && def.operation === "query") walk(def.selectionSet)
	}
	return found
}

/** Read a field's arguments, resolving variables. Null when the field should stay on SQL. */
export function readSearchField(field: FieldNode, variables: Record<string, unknown>): SearchFieldParams | null {
	const args = field.arguments ?? []
	if (args.some((a) => a.name.value === "ids")) return null

	const get = (name: string): unknown => {
		const a = args.find((x) => x.name.value === name)
		return a ? valueFromASTUntyped(a.value, variables) : undefined
	}
	const query = get("query")
	if (typeof query !== "string") return null

	const spaceId = get("spaceId")
	const filter = get("filter")
	return {
		query,
		spaceId: typeof spaceId === "string" ? spaceId : undefined,
		first: asNonNegativeInt(get("first")),
		offset: asNonNegativeInt(get("offset")),
		cursorPaged: ["after", "before", "last"].some((n) => get(n) != null),
		filter:
			filter && typeof filter === "object" && !Array.isArray(filter)
				? (filter as Record<string, unknown>)
				: undefined,
	}
}

/** A copy of `document` with `ids: [...]` appended to each mapped field. The input is not mutated. */
export function injectIds(document: DocumentNode, idsByField: ReadonlyMap<FieldNode, string[]>): DocumentNode {
	if (idsByField.size === 0) return document
	return visit(document, {
		Field(node) {
			const ids = idsByField.get(node)
			if (!ids) return undefined
			const arg: ArgumentNode = {
				kind: Kind.ARGUMENT,
				name: {kind: Kind.NAME, value: "ids"},
				value: {
					kind: Kind.LIST,
					values: ids.map((id) => ({kind: Kind.STRING, value: id}) as const),
				},
			}
			return {...node, arguments: [...(node.arguments ?? []), arg]}
		},
	})
}

async function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
	let timer: ReturnType<typeof setTimeout> | undefined
	const timeout = new Promise<never>((_, reject) => {
		timer = setTimeout(() => reject(new Error(`OpenSearch did not answer within ${ms} ms`)), ms)
	})
	try {
		return await Promise.race([p, timeout])
	} finally {
		if (timer) clearTimeout(timer)
	}
}

/**
 * Resolve ids for every eligible search field. Fields whose lookup fails are absent from the map
 * (they run the legacy SQL body). Logs at most one warning per call.
 */
export async function resolveSearchIds(
	document: DocumentNode,
	variables: Record<string, unknown>,
	client: SearchClient,
	timeout: number = timeoutMs(),
): Promise<Map<FieldNode, string[]>> {
	const result = new Map<FieldNode, string[]>()
	const cache = new Map<string, Promise<string[]>>()
	let warned = false

	await Promise.all(
		findRootSearchFields(document).map(async (field) => {
			const params = readSearchField(field, variables)
			if (!params) return
			const q = buildOpenSearchQuery(params)
			if (!q) return
			const key = JSON.stringify(q)
			let pending = cache.get(key)
			if (!pending) {
				pending = withTimeout(client.search(q), timeout).then((r) =>
					r.results.map((x) => normalizeUuid(x.entityId)),
				)
				cache.set(key, pending)
			}
			try {
				result.set(field, await pending)
			} catch (error) {
				if (!warned) {
					warned = true
					log.warn("GraphQL search: OpenSearch lookup failed, falling back to the SQL search", {
						error: error instanceof Error ? error.message : String(error),
					})
				}
			}
		}),
	)
	return result
}

export function useGraphqlSearchOpenSearch(): Plugin {
	return {
		onExecute({args, executeFn, setExecuteFn}) {
			const client = searchClient
			if (!client || !enabled()) return
			if (findRootSearchFields(args.document).length === 0) return

			setExecuteFn(async (execArgs) => {
				let document = execArgs.document
				try {
					const ids = await resolveSearchIds(
						execArgs.document,
						(execArgs.variableValues ?? {}) as Record<string, unknown>,
						client,
					)
					document = injectIds(execArgs.document, ids)
				} catch (error) {
					log.warn("GraphQL search: could not prepare OpenSearch ids, using the SQL search", {
						error: error instanceof Error ? error.message : String(error),
					})
				}
				return executeFn({...execArgs, document})
			})
		},
	}
}

export default useGraphqlSearchOpenSearch
