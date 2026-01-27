import * as Sentry from "@sentry/node"
import {SpanStatusCode, trace} from "@opentelemetry/api"
import {Kind, print, type OperationDefinitionNode, type FieldNode} from "graphql"
import type {Plugin} from "graphql-yoga"

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
	const firstField = operationDef.selectionSet.selections.find(
		(sel): sel is FieldNode => sel.kind === Kind.FIELD,
	)

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
			const query = print(args.document)

			// Get tracer lazily at request time (not module load) to ensure OTEL SDK is initialized
			const tracer = trace.getTracer("gaia-api-graphql")
			const span = tracer.startSpan(`graphql ${operationLabel}`, {
				attributes: {
					"graphql.operation_name": operationLabel,
					"graphql.document": query.slice(0, 2000),
					"http.request_id": requestId,
				},
			})

			return {
				onExecuteDone({result}) {
					const errors = "errors" in result ? result.errors : undefined
					const hasErrors = errors && errors.length > 0

					if (hasErrors) {
						span.setStatus({code: SpanStatusCode.ERROR, message: "GraphQL errors"})
						span.setAttribute("graphql.error_count", errors.length)

						for (const error of errors) {
							Sentry.captureException(error.originalError || error, {
								tags: {
									"graphql.operation_name": operationLabel,
									request_id: requestId,
								},
								extra: {
									query: query.slice(0, 2000),
									path: error.path,
									requestId,
								},
							})
						}
					}

					span.end()
				},
			}
		},
	}
}
