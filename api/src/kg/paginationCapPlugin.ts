/**
 * PostGraphile plugin that bounds pagination on all connections and simple
 * collections.
 *
 * Does two things:
 * 1. Rejects oversized `first`/`last`/`offset` values (> MAX_PAGINATION_LIMIT)
 *    so abusive or buggy clients fail fast with BAD_USER_INPUT.
 * 2. Injects a default `first` when neither `first` nor `last` is supplied,
 *    so a bare `{ entities { id } }` cannot resolve an unbounded collection.
 *    Without this, PostGraphile 4 produces SQL without any LIMIT and returns
 *    every row, which is the worst-case memory/CPU path.
 *
 * Uses `makeWrapResolversPlugin` (method 2) to intercept resolved argument
 * values before they reach the resolver and its arg data generators. This works
 * regardless of whether values are passed as variables or inline literals.
 *
 * Per PostGraphile docs, resolver wrapping only reliably influences SQL
 * generation for root-level resolvers. This is fine for our use case - the
 * expensive queries we're protecting against (e.g. `entities(first: 5000)`)
 * are root-level collection fields. Nested sub-collections (valuesList,
 * relationsList) don't typically accept user-controlled `first` arguments.
 */
import {makeWrapResolversPlugin} from "graphile-utils"
import {GraphQLError} from "graphql"

export const MAX_PAGINATION_LIMIT = 1000

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

/**
 * If the client supplied neither `first` nor `last`, inject `first` set to the
 * cap. This prevents unbounded collection reads from bare queries like
 * `{ entities { id } }`. Returns the original args object when either is set
 * so we don't silently override an explicit `last`-only pagination.
 */
export function applyDefaultFirstIfOmitted(args: Record<string, unknown>): Record<string, unknown> {
	const hasFirst = typeof args.first === "number"
	const hasLast = typeof args.last === "number"
	if (hasFirst || hasLast) {
		return args
	}
	return {...args, first: MAX_PAGINATION_LIMIT}
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
		const boundedArgs = applyDefaultFirstIfOmitted(args)
		return resolve(source, boundedArgs, context, resolveInfo)
	},
)

export default PaginationCapPlugin
