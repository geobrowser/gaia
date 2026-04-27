import {ROOT_CONTEXT, SpanStatusCode, trace} from "@opentelemetry/api"
import * as Sentry from "@sentry/node"
import {
	type ASTNode,
	type FieldNode,
	GraphQLError,
	isTypeSystemDefinitionNode,
	isTypeSystemExtensionNode,
	Kind,
	type OperationDefinitionNode,
	print,
} from "graphql"
import type {Plugin} from "graphql-yoga"
import {graphqlQueryFingerprint} from "../services/queryFingerprint"
import {log} from "../services/telemetry"

// GraphQL error codes that always indicate a client-side problem (bad input,
// syntax/validation errors). These are the expected consequence of a
// misbehaving client and should not alert Sentry.
const CLIENT_ERROR_CODES = new Set(["BAD_USER_INPUT", "GRAPHQL_PARSE_FAILED", "GRAPHQL_VALIDATION_FAILED"])

// True if an AST node is part of a client-authored executable document
// (queries, mutations, subscriptions, fragments, values) as opposed to
// server-side schema definitions. graphql-js attaches these nodes to errors
// that originate from parse/validate/coerce stages before resolvers run.
function isClientDocumentNode(node: ASTNode): boolean {
	return !isTypeSystemDefinitionNode(node) && !isTypeSystemExtensionNode(node)
}

/**
 * Classify whether an error was caused by bad client input and therefore should
 * not alert Sentry.
 *
 * Detection strategy, in priority order:
 *   1. Structured `extensions.code` on the error (or its `originalError`) is one
 *      of the well-known client codes.
 *   2. The error carries AST nodes from the client's executable document AND
 *      has no execution `path` AND didn't come from a resolver throw. This
 *      catches parse, validation, and variable / argument coercion errors from
 *      graphql-js even when they lack an extension code.
 */
export function isClientError(error: unknown): boolean {
	if (error === null || typeof error !== "object") return false

	const err = error as {
		extensions?: {code?: string}
		originalError?: unknown
		message?: string
		nodes?: readonly ASTNode[]
		path?: readonly (string | number)[]
	}

	// 1. Structured code (direct or via originalError)
	const code = err.extensions?.code
	if (typeof code === "string" && CLIENT_ERROR_CODES.has(code)) return true

	const original = err.originalError
	if (original instanceof GraphQLError) {
		const origCode = original.extensions?.code
		if (typeof origCode === "string" && CLIENT_ERROR_CODES.has(origCode)) return true
	}

	// 2. AST-node inspection. Two signals narrow this to *pre-execution* errors
	//    (parse, validate, variable/argument coercion) and exclude anything
	//    thrown from a resolver:
	//      - `path` is only set by graphql-js during execution — parse, validate,
	//        and coerce errors all lack it.
	//      - Plain resolver throws wrap the raw Error in `originalError`.
	//    Without the `path` guard, a resolver that does
	//    `throw new GraphQLError("db timed out")` would satisfy the node-kind
	//    check and silently skip Sentry.
	const hasResolverOrigin = original instanceof Error && !(original instanceof GraphQLError)
	if (!hasResolverOrigin && !err.path && err.nodes?.length) {
		if (err.nodes.some(isClientDocumentNode)) return true
	}

	return false
}

const SLOW_QUERY_THRESHOLD_MS = 3000
const LARGE_RESPONSE_THRESHOLD_BYTES = 1_000_000 // 1 MB

type TraceContext = {
	traceId: string
	spanId: string
	traceFlags: number
}

/**
 * Extract request ID from context.
 * Checks common headers, falls back to generating a UUID.
 */
function getRequestId(ctx: unknown): string {
	const c = ctx as {request?: Request}
	const request = c?.request

	if (!request?.headers) {
		return crypto.randomUUID()
	}

	return (
		request.headers.get("x-request-id") ||
		request.headers.get("x-correlation-id") ||
		request.headers.get("traceparent")?.split("-")[1] ||
		crypto.randomUUID()
	)
}

/**
 * Extract trace context passed from HTTP middleware.
 */
function getTraceContext(ctx: unknown): TraceContext | undefined {
	const c = ctx as {traceContext?: TraceContext}
	return c?.traceContext
}

/**
 * Extract a descriptive operation name from the GraphQL document.
 * Returns the explicit operation name if present, otherwise derives one from
 * the operation type and first selected field (e.g., "query spaces", "mutation createEntity").
 */
function getOperationLabel(args: {operationName?: string | null; document: {definitions: readonly unknown[]}}): string {
	if (args.operationName) {
		return args.operationName
	}

	// Find the operation definition
	const operationDef = args.document.definitions.find(
		(def): def is OperationDefinitionNode =>
			typeof def === "object" && def !== null && "kind" in def && def.kind === Kind.OPERATION_DEFINITION,
	)

	if (!operationDef) {
		return "anonymous"
	}

	const operationType = operationDef.operation // "query", "mutation", "subscription"
	const firstField = operationDef.selectionSet.selections.find((sel): sel is FieldNode => sel.kind === Kind.FIELD)

	if (firstField) {
		return `${operationType} ${firstField.name.value}`
	}

	return operationType
}

export function useGraphQLInstrumentation(): Plugin {
	return {
		onExecute({args}) {
			const operationName = args.operationName
			const requestId = getRequestId(args.contextValue)

			// Skip introspection queries - they're from dev tooling, not useful to trace
			if (operationName === "IntrospectionQuery") {
				return {}
			}

			const operationLabel = getOperationLabel(args)
			const ctxWithSetter = args.contextValue as {setGraphqlOperationName?: (operationName: string) => void}
			ctxWithSetter.setGraphqlOperationName?.(operationLabel)
			const query = print(args.document)
			const queryFingerprint = graphqlQueryFingerprint(query)
			const variables = args.variableValues ? JSON.stringify(args.variableValues).slice(0, 2000) : undefined

			// Get tracer lazily at request time (not module load) to ensure OTEL SDK is initialized
			const tracer = trace.getTracer("gaia-api-graphql")

			// Build parent context from trace context passed by HTTP middleware
			// (OTEL async context doesn't propagate through graphql-yoga)
			const traceCtx = getTraceContext(args.contextValue)
			let parentContext = ROOT_CONTEXT
			if (traceCtx) {
				parentContext = trace.setSpanContext(ROOT_CONTEXT, {
					traceId: traceCtx.traceId,
					spanId: traceCtx.spanId,
					traceFlags: traceCtx.traceFlags,
					isRemote: false,
				})
			}

			const executeStartMs = Date.now()

			const span = tracer.startSpan(
				`graphql ${operationLabel}`,
				{
					attributes: {
						"graphql.operation_name": operationLabel,
						"graphql.query_fingerprint": queryFingerprint,
						"graphql.document": query.slice(0, 2000),
						...(variables && {"graphql.variables": variables}),
						"http.request_id": requestId,
					},
				},
				parentContext,
			)

			return {
				onExecuteDone({result}) {
					const durationMs = Date.now() - executeStartMs
					const errors = "errors" in result ? result.errors : undefined
					const hasErrors = errors && errors.length > 0

					// Measure serialized response size for large payload detection.
					// Only stringify data (not errors) since that's the memory-heavy part.
					// Guard behind a 1s duration check to avoid the stringify cost on fast queries.
					const data = "data" in result ? result.data : undefined
					let responseSizeBytes: number | undefined
					if (data && durationMs >= 1000) {
						try {
							responseSizeBytes = JSON.stringify(data).length
						} catch {
							// If stringify fails (circular refs, etc.), skip size measurement
						}
					}

					// Log the full query (no truncation): the missing tail is
					// the part you need when triaging a slow / large response.
					// Sentry / OTEL paths still cap at their own limits below.
					if (responseSizeBytes !== undefined && responseSizeBytes >= LARGE_RESPONSE_THRESHOLD_BYTES) {
						log.warn("Large GraphQL response", {
							operationName: operationLabel,
							queryFingerprint,
							responseSizeBytes,
							responseSizeMB: Math.round((responseSizeBytes / 1_000_000) * 100) / 100,
							durationMs,
							query,
							variables: args.variableValues,
							requestId,
						})
					}

					if (durationMs >= SLOW_QUERY_THRESHOLD_MS) {
						log.warn("Slow GraphQL query", {
							operationName: operationLabel,
							queryFingerprint,
							durationMs,
							responseSizeBytes,
							query,
							variables: args.variableValues,
							requestId,
						})
					}

					if (hasErrors) {
						span.setStatus({code: SpanStatusCode.ERROR, message: "GraphQL errors"})
						span.setAttribute("graphql.error_count", errors.length)

						for (const error of errors) {
							// Client-caused errors (bad pagination args, syntax errors, validation
							// failures) are expected noise and should not alert Sentry.
							if (isClientError(error)) continue
							Sentry.captureException(error.originalError || error, {
								tags: {
									"graphql.operation_name": operationLabel,
									"graphql.query_fingerprint": queryFingerprint,
									request_id: requestId,
								},
								extra: {
									queryFingerprint,
									query: query.slice(0, 2000),
									variables: args.variableValues,
									path: error.path,
									requestId,
								},
							})
						}
					}

					span.setAttribute("graphql.duration_ms", durationMs)
					if (responseSizeBytes !== undefined) {
						span.setAttribute("graphql.response_size_bytes", responseSizeBytes)
					}

					// Sentry metrics for dashboards and alerting
					Sentry.metrics.distribution("graphql.duration_ms", durationMs, {
						attributes: {operation: operationLabel},
						unit: "millisecond",
					})
					if (responseSizeBytes !== undefined) {
						Sentry.metrics.distribution("graphql.large_response_size_bytes", responseSizeBytes, {
							attributes: {operation: operationLabel},
							unit: "byte",
						})
					}

					span.end()
				},
			}
		},
	}
}
