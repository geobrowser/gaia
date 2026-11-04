import {type DocumentNode, Kind, parse, type SelectionSetNode} from "graphql"

export type Classification = {
	operationName: string | null
	operationType: "query" | "mutation" | "subscription" | null
	/** Distinct response-level root field names (not aliases), in document order. Empty when the request did not parse. */
	rootFields: string[]
}

const UNPARSED: Classification = {operationName: null, operationType: null, rootFields: []}

type GraphqlParams = {query?: unknown; operationName?: unknown}

/** Reads the query and operation name from a GraphQL-over-HTTP request without consuming the request. */
export async function readGraphqlParams(
	request: Request,
): Promise<{query: string; operationName: string | null} | null> {
	let params: GraphqlParams | undefined
	try {
		if (request.method === "GET") {
			const url = new URL(request.url)
			params = {
				query: url.searchParams.get("query") ?? undefined,
				operationName: url.searchParams.get("operationName"),
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
	return {query: params.query, operationName: typeof params.operationName === "string" ? params.operationName : null}
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

export async function classifyRequest(request: Request): Promise<Classification> {
	const params = await readGraphqlParams(request)
	if (!params) return UNPARSED
	try {
		return classifyDocument(parse(params.query, {noLocation: true}), params.operationName)
	} catch {
		return UNPARSED
	}
}
