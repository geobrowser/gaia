/**
 * PostGraphile plugin that bounds pagination on all connections and simple
 * collections — at every nesting level.
 *
 * Does two things, uniformly across root and nested fields:
 * 1. Rejects oversized `first`/`last`/`offset` values (> MAX_PAGINATION_LIMIT)
 *    so abusive or buggy clients fail fast with BAD_USER_INPUT.
 * 2. Injects a default `first = DEFAULT_PAGINATION_LIMIT` when neither `first`
 *    nor `last` is supplied, so a bare `{ entity(id: ...) { relationsList } }`
 *    cannot resolve an unbounded sub-collection. Without this, PostGraphile 4
 *    produces SQL without any LIMIT and returns every row — the Claim entity
 *    has 75K+ inbound relations, which is the worst-case memory/CPU path.
 *
 * The default (100) is decoupled from the cap (1000): clients that genuinely
 * need a large page can still opt in by passing an explicit `first` up to
 * MAX_PAGINATION_LIMIT, but clients that omit pagination get a sensibly small
 * page rather than 1000 rows of unrequested payload.
 *
 * Implementation: hooks `GraphQLObjectType:fields:field:args` and registers an
 * arg data generator on every connection / simple-collection field. The
 * generator's `pgQuery(queryBuilder)` runs during SQL construction at the
 * field's own nesting level, so a nested `relationsList` inside an `entity`
 * selection is capped exactly like a top-level `relations` query.
 *
 * This supersedes the earlier `makeWrapResolversPlugin` approach, which per
 * PostGraphile docs only reliably influences SQL for root-level resolvers —
 * nested sub-collections were uncapped, which is exactly the pattern
 * geogenesis' EntityPage hits when a user opens a hub entity like Claim.
 */
import {type FieldNode, GraphQLError, type ValidationContext} from "graphql"

export const MAX_PAGINATION_LIMIT = 1000
export const DEFAULT_PAGINATION_LIMIT = 100

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
 * GraphQL validation rule that rejects `first` + `last` on the same field.
 *
 * PostGraphile's `PgConnectionArgFirstLastBeforeAfter` plugin also enforces
 * this, but it runs during SQL construction (pgQuery hook) and throws a plain
 * `Error` that reaches the client without any extension code and returns HTTP
 * 200. That path sent the error to Sentry as a server issue and gave the
 * client an unhelpful response.
 *
 * Running it as a validation rule catches the misuse during the validate
 * phase — before any resolver or SQL runs — and surfaces it as a structured
 * BAD_USER_INPUT / 400 on the response.
 *
 * No schema-type introspection needed: the GraphQL schema only exposes `last`
 * on fields that legitimately paginate, so checking argument presence alone
 * is sufficient.
 */
export function NoFirstAndLastRule(context: ValidationContext) {
	return {
		Field(node: FieldNode) {
			const args = node.arguments ?? []
			const hasFirst = args.some((a) => a.name.value === "first")
			const hasLast = args.some((a) => a.name.value === "last")
			if (hasFirst && hasLast) {
				context.reportError(
					new GraphQLError(`Cannot specify both "first" and "last" on field "${node.name.value}"`, {
						nodes: [node],
						extensions: {
							code: "BAD_USER_INPUT",
							http: {status: 400},
						},
					}),
				)
			}
		},
	}
}

/**
 * If the client supplied neither `first` nor `last`, inject `first` set to the
 * default page size (DEFAULT_PAGINATION_LIMIT). Returns the original args
 * object when either is set so we don't silently override an explicit
 * `last`-only pagination.
 *
 * Kept as a pure helper so unit tests can exercise the policy without
 * standing up a full PostGraphile schema.
 */
export function applyDefaultFirstIfOmitted(args: Record<string, unknown>): Record<string, unknown> {
	const hasFirst = typeof args.first === "number"
	const hasLast = typeof args.last === "number"
	if (hasFirst || hasLast) {
		return args
	}
	return {...args, first: DEFAULT_PAGINATION_LIMIT}
}

const PaginationCapPlugin = (builder: any) => {
	builder.hook("GraphQLObjectType:fields:field:args", (args: any, _build: any, context: any) => {
		const {
			scope: {isPgFieldConnection, isPgFieldSimpleCollection},
			addArgDataGenerator,
		} = context

		if (!isPgFieldConnection && !isPgFieldSimpleCollection) {
			return args
		}
		if (typeof addArgDataGenerator !== "function") {
			return args
		}

		addArgDataGenerator((fieldArgs: {first?: number; last?: number; offset?: number}) => ({
			pgQuery: (queryBuilder: any) => {
				assertPaginationWithinLimit(fieldArgs as Record<string, unknown>)

				const hasFirst = typeof fieldArgs.first === "number"
				const hasLast = typeof fieldArgs.last === "number"
				if (!hasFirst && !hasLast) {
					queryBuilder.first(DEFAULT_PAGINATION_LIMIT)
				}
			},
		}))

		return args
	})
}

export default PaginationCapPlugin
