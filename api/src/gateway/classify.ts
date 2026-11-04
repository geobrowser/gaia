import {type DocumentNode, Kind, parse, type SelectionSetNode} from "graphql"

export type Classification = {
	operationName: string | null
	operationType: "query" | "mutation" | "subscription" | null
	/** Distinct response-level root field names (not aliases), in document order. Empty when the request did not parse. */
	rootFields: string[]
}

const UNPARSED: Classification = {operationName: null, operationType: null, rootFields: []}

type GraphqlParams = {query?: unknown; operationName?: unknown; variables?: unknown}

export type GraphqlRequestParams = {
	query: string
	operationName: string | null
	/** The request's variables when they are a JSON object, otherwise empty. */
	variables: Record<string, unknown>
}

const asVariables = (raw: unknown): Record<string, unknown> => {
	if (typeof raw === "string") {
		try {
			return asVariables(JSON.parse(raw))
		} catch {
			return {}
		}
	}
	return raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
}

/** Reads the query and operation name from a GraphQL-over-HTTP request without consuming the request. */
export async function readGraphqlParams(request: Request): Promise<GraphqlRequestParams | null> {
	let params: GraphqlParams | undefined
	try {
		if (request.method === "GET") {
			const url = new URL(request.url)
			params = {
				query: url.searchParams.get("query") ?? undefined,
				operationName: url.searchParams.get("operationName"),
				variables: url.searchParams.get("variables") ?? undefined,
			}
		} else if (request.method === "POST") {
			const body: unknown = await request.clone().json()
			// A JSON array is a batch; the gateway leaves batches to PostGraphile.
			params = body && typeof body === "object" && !Array.isArray(body) ? (body as GraphqlParams) : undefined
		}
	} catch {
		return null
	}
	if (typeof params?.query !== "string") return null
	return {
		query: params.query,
		operationName: typeof params.operationName === "string" ? params.operationName : null,
		variables: asVariables(params.variables),
	}
}

function collectRootFields(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	seen: Set<string>,
	out: string[],
): void {
	for (const selection of set.selections) {
		if (selection.kind === Kind.FIELD) {
			const name = selection.name.value
			if (name !== "__typename" && !seen.has(name)) {
				seen.add(name)
				out.push(name)
			}
		} else if (selection.kind === Kind.INLINE_FRAGMENT) {
			collectRootFields(selection.selectionSet, fragments, seen, out)
		} else {
			const fragment = fragments.get(selection.name.value)
			const visitedKey = `...${selection.name.value}`
			if (fragment && !seen.has(visitedKey)) {
				seen.add(visitedKey)
				collectRootFields(fragment, fragments, seen, out)
			}
		}
	}
}

/** Names the root fields the operation selects, following fragments. Never throws. */
export function classifyDocument(document: DocumentNode, operationName: string | null): Classification {
	const fragments = new Map<string, SelectionSetNode>()
	const operations = []
	for (const definition of document.definitions) {
		if (definition.kind === Kind.FRAGMENT_DEFINITION) fragments.set(definition.name.value, definition.selectionSet)
		else if (definition.kind === Kind.OPERATION_DEFINITION) operations.push(definition)
	}
	const operation = operationName
		? operations.find((op) => op.name?.value === operationName)
		: operations.length === 1
			? operations[0]
			: undefined
	if (!operation) return UNPARSED
	const rootFields: string[] = []
	collectRootFields(operation.selectionSet, fragments, new Set(), rootFields)
	return {operationName: operation.name?.value ?? null, operationType: operation.operation, rootFields}
}

/** A classified request with what it was classified from, so a caller that needs the document parses nothing twice. */
export type ClassifiedRequest = {
	classification: Classification
	params: GraphqlRequestParams | null
	document: DocumentNode | null
}

export async function classifyRequestWithDocument(request: Request): Promise<ClassifiedRequest> {
	const params = await readGraphqlParams(request)
	if (!params) return {classification: UNPARSED, params: null, document: null}
	try {
		const document = parse(params.query, {noLocation: true})
		return {classification: classifyDocument(document, params.operationName), params, document}
	} catch {
		return {classification: UNPARSED, params, document: null}
	}
}

export async function classifyRequest(request: Request): Promise<Classification> {
	return (await classifyRequestWithDocument(request)).classification
}
