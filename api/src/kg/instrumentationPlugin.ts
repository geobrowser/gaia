import * as Sentry from "@sentry/node"
import {print} from "graphql"
import type {Plugin} from "graphql-yoga"

/**
 * Extract request ID from context.
 * Checks common headers, falls back to generating a UUID.
 */
function getRequestId(context: unknown): string {
	const ctx = context as {request?: Request}
	const request = ctx?.request

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

			// Wrap GraphQL execution in a Sentry span
			return Sentry.startSpan(
				{
					name: `graphql ${operationName}`,
					op: "graphql.execute",
					attributes: {
						"graphql.operation_name": operationName,
						"graphql.document": query.slice(0, 2000),
						"http.request_id": requestId,
					},
				},
				(span) => {
					return {
						onExecuteDone({result}) {
							const errors = "errors" in result ? result.errors : undefined
							const hasErrors = errors && errors.length > 0

							if (hasErrors) {
								span.setStatus({code: 2, message: "error"})
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
						},
					}
				},
			)
		},
	}
}
