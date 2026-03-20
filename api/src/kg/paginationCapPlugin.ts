/**
 * PostGraphile plugin that caps `first`, `last`, and `offset` pagination
 * arguments on all connections and simple collections.
 *
 * PostGraphile 4 passes `first`/`last` directly to SQL LIMIT without any cap,
 * meaning any client can request unbounded result sets. This is especially
 * expensive for queries that fan out into nested lists (valuesList, relationsList).
 *
 * Uses `makeWrapResolversPlugin` (method 2) to intercept resolved argument
 * values before they reach the resolver and its arg data generators. This works
 * regardless of whether values are passed as variables or inline literals.
 *
 * Values exceeding the cap are clamped (not rejected) so that well-intentioned
 * clients don't get hard failures - they just get fewer results than requested.
 *
 * Per PostGraphile docs, resolver wrapping only reliably influences SQL
 * generation for root-level resolvers. This is fine for our use case - the
 * expensive queries we're protecting against (e.g. `entities(first: 5000)`)
 * are root-level collection fields. Nested sub-collections (valuesList,
 * relationsList) don't typically accept user-controlled `first` arguments.
 */

import {makeWrapResolversPlugin} from "graphile-utils"

const MAX_PAGINATION_LIMIT = 1000

const PaginationCapPlugin = makeWrapResolversPlugin(
	(context) => {
		if (context.scope.isPgFieldConnection || context.scope.isPgFieldSimpleCollection) {
			return {}
		}
		return null
	},
	() => (resolve, source, args, context, resolveInfo) => {
		const clampedArgs = {...args}

		if (typeof clampedArgs.first === "number" && clampedArgs.first > MAX_PAGINATION_LIMIT) {
			clampedArgs.first = MAX_PAGINATION_LIMIT
		}
		if (typeof clampedArgs.last === "number" && clampedArgs.last > MAX_PAGINATION_LIMIT) {
			clampedArgs.last = MAX_PAGINATION_LIMIT
		}
		if (typeof clampedArgs.offset === "number" && clampedArgs.offset > MAX_PAGINATION_LIMIT) {
			clampedArgs.offset = MAX_PAGINATION_LIMIT
		}

		return resolve(source, clampedArgs, context, resolveInfo)
	},
)

export default PaginationCapPlugin
