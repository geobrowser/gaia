import * as Sentry from "@sentry/node"
import {SpanStatusCode, trace} from "@opentelemetry/api"
import {print} from "graphql"
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

export function useGraphQLInstrumentation(): Plugin {
	return {
		onExecute({args}) {
			const operationName = args.operationName || "anonymous"
			const query = print(args.document)
			const requestId = getRequestId(args.contextValue)

			// Get tracer lazily at request time (not module load) to ensure OTEL SDK is initialized
			const tracer = trace.getTracer("gaia.api.graphql")
			const span = tracer.startSpan(`graphql ${operationName}`, {
				attributes: {
					"graphql.operation_name": operationName,
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
									"graphql.operation_name": operationName,
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
