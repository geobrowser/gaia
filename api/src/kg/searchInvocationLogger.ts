/**
 * Yoga plugin that emits `log.warn` whenever a GraphQL operation invokes
 * the `search` or `searchConnection` root fields.
 *
 * Motivation: we want per-invocation visibility on the text-search path so
 * we can (a) measure how heavily the GraphQL search is used vs. the REST
 * `/search` endpoint, (b) spot clients that still route through
 * `entities(filter: {name: ...})` (the slow path) vs. the purpose-built
 * `search()` pg function, and (c) have a caller IP captured for abuse
 * investigations without standing up a full rate-limit system.
 *
 * AST-only detection — no schema introspection, no realm-crossing issues.
 * Walks each operation's selection set, follows inline fragments and
 * fragment spreads, and emits one `log.warn` per search-field selection.
 * Multiple `search` calls in a single document all get logged.
 *
 * Client IP extraction (see `extractClientIp`) prefers `X-Real-IP`
 * because it's the unspoofable value in this cluster's topology:
 * DO LoadBalancer → ingress-nginx with `externalTrafficPolicy: Local`
 * and L4 TCP passthrough preserves the real client source IP to nginx,
 * and nginx sets `X-Real-IP = $remote_addr` (single-valued, overwrites
 * any client-supplied header). `X-Forwarded-For` is a fallback: nginx
 * appends its observed `$remote_addr` to the right of any client-supplied
 * value, so the *rightmost* entry is trustworthy and leftmost entries are
 * client-controlled / spoofable. Never reading leftmost XFF here.
 */
import {
	type DocumentNode,
	type FieldNode,
	type FragmentDefinitionNode,
	Kind,
	type OperationDefinitionNode,
	type SelectionSetNode,
	type ValueNode,
} from "graphql"
import type {Plugin} from "graphql-yoga"
import {log} from "../services/telemetry"

const SEARCH_FIELD_NAMES: ReadonlySet<string> = new Set(["search", "searchConnection"])

export type SearchInvocation = {
	field: string
	query?: string
	spaceId?: string
	first?: number
	similarityThreshold?: number
}

/**
 * Extract the real client IP from request headers.
 *
 * Priority:
 *   1. `X-Real-IP` — set by nginx to `$remote_addr`, single-valued,
 *      overwrites any client-supplied value. Unspoofable via HTTP in
 *      our cluster config.
 *   2. Rightmost entry of `X-Forwarded-For` — nginx appends its own
 *      `$remote_addr` to the end of the XFF chain via
 *      `$proxy_add_x_forwarded_for`. Leftmost entries are client-
 *      controlled and NOT trusted; we only read the rightmost.
 *
 * Returns `null` if neither header is present.
 */
export function extractClientIp(headers: Headers): string | null {
	const xRealIp = headers.get("x-real-ip")?.trim()
	if (xRealIp) return xRealIp

	const xff = headers.get("x-forwarded-for")
	if (xff) {
		const parts = xff
			.split(",")
			.map((s) => s.trim())
			.filter(Boolean)
		const rightmost = parts[parts.length - 1]
		if (rightmost) return rightmost
	}
	return null
}

/**
 * Walk a GraphQL document and return every invocation of `search` or
 * `searchConnection` on an operation, resolved against the variables map.
 */
export function findSearchInvocations(
	document: DocumentNode,
	variables: Record<string, unknown> = {},
): SearchInvocation[] {
	const fragments: Record<string, FragmentDefinitionNode> = {}
	for (const def of document.definitions) {
		if (def.kind === Kind.FRAGMENT_DEFINITION) {
			fragments[def.name.value] = def
		}
	}

	const invocations: SearchInvocation[] = []

	const walk = (selectionSet: SelectionSetNode): void => {
		for (const sel of selectionSet.selections) {
			if (sel.kind === Kind.FIELD) {
				if (SEARCH_FIELD_NAMES.has(sel.name.value)) {
					invocations.push({
						field: sel.name.value,
						query: readStringArg(sel, "query", variables),
						spaceId: readStringArg(sel, "spaceId", variables),
						first: readNumericArg(sel, "first", variables),
						similarityThreshold: readNumericArg(sel, "similarityThreshold", variables),
					})
				}
				if (sel.selectionSet) walk(sel.selectionSet)
			} else if (sel.kind === Kind.INLINE_FRAGMENT) {
				if (sel.selectionSet) walk(sel.selectionSet)
			} else if (sel.kind === Kind.FRAGMENT_SPREAD) {
				const frag = fragments[sel.name.value]
				if (frag) walk(frag.selectionSet)
			}
		}
	}

	for (const def of document.definitions) {
		if (def.kind === Kind.OPERATION_DEFINITION) {
			walk(def.selectionSet)
		}
	}
	return invocations
}

function readStringArg(field: FieldNode, argName: string, variables: Record<string, unknown>): string | undefined {
	const arg = field.arguments?.find((a) => a.name.value === argName)
	if (!arg) return undefined
	return readStringValue(arg.value, variables)
}

function readNumericArg(field: FieldNode, argName: string, variables: Record<string, unknown>): number | undefined {
	const arg = field.arguments?.find((a) => a.name.value === argName)
	if (!arg) return undefined
	return readNumericValue(arg.value, variables)
}

function readStringValue(value: ValueNode, variables: Record<string, unknown>): string | undefined {
	if (value.kind === Kind.STRING || value.kind === Kind.ENUM) return value.value
	if (value.kind === Kind.VARIABLE) {
		const v = variables[value.name.value]
		return typeof v === "string" ? v : undefined
	}
	return undefined
}

function readNumericValue(value: ValueNode, variables: Record<string, unknown>): number | undefined {
	if (value.kind === Kind.INT || value.kind === Kind.FLOAT) {
		const n = Number(value.value)
		return Number.isFinite(n) ? n : undefined
	}
	if (value.kind === Kind.VARIABLE) {
		const v = variables[value.name.value]
		return typeof v === "number" ? v : undefined
	}
	return undefined
}

function getOperationName(document: DocumentNode, fallback?: string | null): string {
	if (fallback) return fallback
	const op = document.definitions.find(
		(def): def is OperationDefinitionNode => def.kind === Kind.OPERATION_DEFINITION,
	)
	return op?.name?.value ?? "anonymous"
}

/**
 * Cap logged field values so a malicious/large argument can't blow up
 * our log line or Sentry breadcrumb.
 */
function truncate(value: string | undefined, max = 256): string | undefined {
	if (value === undefined) return undefined
	return value.length > max ? `${value.slice(0, max)}…` : value
}

/**
 * Registered in postgraphile.ts's sharedPlugins. Returns early on
 * documents that don't invoke a search field.
 */
export function useSearchInvocationLogger(): Plugin {
	return {
		onExecute({args}) {
			const invocations = findSearchInvocations(args.document, args.variableValues ?? {})
			if (invocations.length === 0) return

			const ctx = args.contextValue as {request?: Request; requestId?: string}
			const headers = ctx.request?.headers
			const clientIp = headers ? extractClientIp(headers) : null
			const userAgent = headers?.get("user-agent") ?? null
			const origin = headers?.get("origin") ?? null
			const operationName = getOperationName(args.document, args.operationName)

			for (const inv of invocations) {
				log.warn("GraphQL search field invoked", {
					field: inv.field,
					query: truncate(inv.query),
					spaceId: inv.spaceId,
					first: inv.first,
					similarityThreshold: inv.similarityThreshold,
					clientIp,
					origin,
					userAgent: truncate(userAgent ?? undefined),
					requestId: ctx.requestId,
					operationName,
				})
			}
		},
	}
}

export default useSearchInvocationLogger
