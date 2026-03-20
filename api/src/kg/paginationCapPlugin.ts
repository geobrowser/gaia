/**
 * PostGraphile plugin that rejects oversized `first`, `last`, and `offset`
 * pagination arguments on all connections and simple collections.
 *
 * PostGraphile 4 passes `first`/`last` directly to SQL LIMIT without any cap,
 * meaning any client can request unbounded result sets. This is especially
 * expensive for queries that fan out into nested lists (valuesList, relationsList).
 *
 * Uses `makeWrapResolversPlugin` (method 2) to intercept resolved argument
 * values before they reach the resolver and its arg data generators. This works
 * regardless of whether values are passed as variables or inline literals.
 *
 * Values exceeding the cap are rejected so abusive or buggy clients fail fast
 * instead of silently issuing very large collection reads.
 *
 * Per PostGraphile docs, resolver wrapping only reliably influences SQL
 * generation for root-level resolvers. This is fine for our use case - the
 * expensive queries we're protecting against (e.g. `entities(first: 5000)`)
 * are root-level collection fields. Nested sub-collections (valuesList,
 * relationsList) don't typically accept user-controlled `first` arguments.
 */
import {GraphQLError} from "graphql"
import {makeWrapResolversPlugin} from "graphile-utils"

const MAX_PAGINATION_LIMIT = 1000

export function assertPaginationWithinLimit(args: Record<string, unknown>) {
	for (const key of ["first", "last", "offset"] as const) {
		const value = args[key]
		if (typeof value === "number" && value > MAX_PAGINATION_LIMIT) {
			throw new GraphQLError(
				`Pagination argument "${key}" cannot exceed ${MAX_PAGINATION_LIMIT}; received ${value}`,
				{
					extensions: {
						code: "BAD_USER_INPUT",
						http: {
							status: 400,
						},
					},
				},
			)
		}
	}
}

const PaginationCapPlugin = makeWrapResolversPlugin(
	(context) => {
		if (context.scope.isPgFieldConnection || context.scope.isPgFieldSimpleCollection) {
			return {}
		}
		return null
	},
	() => (resolve, source, args, context, resolveInfo) => {
		assertPaginationWithinLimit(args)
		return resolve(source, args, context, resolveInfo)
	},
)

export default PaginationCapPlugin
