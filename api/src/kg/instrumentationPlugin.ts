import {ROOT_CONTEXT, SpanStatusCode, trace} from "@opentelemetry/api"
import * as Sentry from "@sentry/node"
import {type FieldNode, Kind, type OperationDefinitionNode, print} from "graphql"
import type {Plugin} from "graphql-yoga"
import {graphqlQueryFingerprint} from "../services/queryFingerprint"
import {log} from "../services/telemetry"

const SLOW_QUERY_THRESHOLD_MS = 3000

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

					if (durationMs >= SLOW_QUERY_THRESHOLD_MS) {
						log.warn("Slow GraphQL query", {
							operationName: operationLabel,
							queryFingerprint,
							durationMs,
							query: query.slice(0, 5000),
							variables: args.variableValues,
							requestId,
						})
					}

					if (hasErrors) {
						span.setStatus({code: SpanStatusCode.ERROR, message: "GraphQL errors"})
						span.setAttribute("graphql.error_count", errors.length)

						for (const error of errors) {
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
					span.end()
				},
			}
		},
	}
}
