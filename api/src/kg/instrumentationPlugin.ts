import * as Sentry from "@sentry/node"
import {print} from "graphql"
import type {Plugin} from "graphql-yoga"

export function useGraphQLInstrumentation(): Plugin {
	return {
		onExecute({args}) {
			const operationName = args.operationName || "anonymous"
			const query = print(args.document)

			// Wrap GraphQL execution in a Sentry span
			return Sentry.startSpan(
				{
					name: `graphql ${operationName}`,
					op: "graphql.execute",
					attributes: {
						"graphql.operation_name": operationName,
						"graphql.document": query.slice(0, 2000),
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
										tags: {"graphql.operation_name": operationName},
										extra: {
											query: query.slice(0, 2000),
											path: error.path,
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
