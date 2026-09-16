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
 * The default is decoupled from the cap (1000): clients that genuinely need a
 * large page can still opt in by passing an explicit `first` up to
 * MAX_PAGINATION_LIMIT, but clients that omit pagination get a sensibly small
 * page rather than 1000 rows of unrequested payload.
 *
 * The injected default also *tapers with nesting depth* — see
 * DEFAULT_PAGINATION_LIMITS_BY_DEPTH — because a single default applied at
 * every level is not a bound, it is an exponent.
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
 * Default page size injected at each level of collection nesting, when the
 * client supplies neither `first` nor `last`.
 *
 * Index 0 is a root collection, index 1 a collection inside it, and so on; past
 * the end of the array the last entry applies.
 *
 * Why this is not one number
 * --------------------------
 * It was, and the number was 100 at every level — which reads as a bound but
 * multiplies. A query asking for entities, each entity's relations, and each
 * relation's target values, while declaring no pagination below the root, was
 * granted 100 x 100 x 100.
 *
 * Measured in production on 2026-09-16 (3h, 6 api pods, every response over
 * 1s): request duration is strongly linear in response bytes — r = 0.83, slope
 * 446 ms/MB — and the api moves about 2 MB/s. The cost is per row hydrated, so
 * the only thing that makes a big response cheap is fewer rows.
 *
 * The two worst query shapes in that window were 69% of all bytes, and both had
 * the same signature: an explicit `first` at the root and **nothing at all**
 * below it. Their authors had not asked for 100 nested rows; the server handed
 * it to them. One was 19 MB and took 10.9 s.
 *
 * So the defaults now taper. An explicit `first` is still honoured anywhere up
 * to MAX_PAGINATION_LIMIT, which is the point: a caller that genuinely wants
 * 100 nested rows says so and gets them. What changes is only what an
 * unannotated query is given by default, and a default that multiplies should
 * be small.
 *
 * Tunable with GRAPHQL_NESTED_PAGINATION_DEFAULTS (comma-separated, e.g.
 * "100,25,10") so the taper can be adjusted or flattened without a deploy of
 * new code.
 */
export const DEFAULT_PAGINATION_LIMITS_BY_DEPTH: readonly number[] = parseDepthLimits(
	process.env.GRAPHQL_NESTED_PAGINATION_DEFAULTS,
	[DEFAULT_PAGINATION_LIMIT, 25, 10],
)

/**
 * Parse the env override, falling back whole rather than partially.
 *
 * A malformed override is ignored entirely instead of being merged: a half
 * applied pagination policy is harder to reason about than either the default
 * or the override, and this runs once at module load where nothing is watching.
 */
export function parseDepthLimits(raw: string | undefined, fallback: readonly number[]): readonly number[] {
	if (raw === undefined || raw.trim() === "") return fallback
	const parts = raw.split(",").map((part) => Number.parseInt(part.trim(), 10))
	if (parts.length === 0) return fallback
	if (parts.some((n) => !Number.isFinite(n) || n <= 0 || n > MAX_PAGINATION_LIMIT)) return fallback
	return parts
}

/**
 * The default page size for a collection nested `depth` levels deep, where
 * depth 0 is a root collection.
 */
export function defaultLimitForDepth(depth: number): number {
	const limits = DEFAULT_PAGINATION_LIMITS_BY_DEPTH
	const index = Math.min(Math.max(depth, 0), limits.length - 1)
	return limits[index] as number
}

/**
 * If the client supplied neither `first` nor `last`, inject `first` set to the
 * default page size for this nesting depth. Returns the original args object
 * when either is set so we don't silently override an explicit `last`-only
 * pagination.
 *
 * Kept as a pure helper so unit tests can exercise the policy without
 * standing up a full PostGraphile schema.
 */
export function applyDefaultFirstIfOmitted(args: Record<string, unknown>, depth = 0): Record<string, unknown> {
	const hasFirst = typeof args.first === "number"
	const hasLast = typeof args.last === "number"
	if (hasFirst || hasLast) {
		return args
	}
	return {...args, first: defaultLimitForDepth(depth)}
}

/**
 * Marks a QueryBuilder with the collection depth this plugin assigned it, so a
 * nested collection can find its nearest *collection* ancestor.
 *
 * Walking `parentQueryBuilder` and counting every link would be wrong: PostGraphile
 * also builds child query builders for singular relations (`toEntity { ... }`),
 * which add no row multiplication at all. Counting those would tighten a
 * shallow query as if it were deep. Tagging only the builders this plugin has
 * actually capped means the count is of collections, whatever else sits between
 * them.
 *
 * A symbol rather than a property name so it cannot collide with anything
 * graphile-build-pg puts on the builder.
 */
const COLLECTION_DEPTH = Symbol.for("gaia.paginationCap.collectionDepth")

type TaggedQueryBuilder = {
	parentQueryBuilder?: TaggedQueryBuilder
	[COLLECTION_DEPTH]?: number
}

/**
 * Depth of this collection, counting only enclosing collections.
 *
 * Relies on a parent's `pgQuery` having run before its children's, which holds
 * because PostGraphile builds the query top-down — a child query builder is
 * constructed while resolving the parent's selection set. If that ever stopped
 * being true the symbol would be missing and the depth would read as 0, i.e.
 * the pre-existing behaviour: this fails toward the old, looser default rather
 * than toward an unexpectedly tight one.
 */
export function collectionDepthOf(queryBuilder: TaggedQueryBuilder): number {
	let ancestor = queryBuilder.parentQueryBuilder
	while (ancestor) {
		const tagged = ancestor[COLLECTION_DEPTH]
		if (typeof tagged === "number") return tagged + 1
		ancestor = ancestor.parentQueryBuilder
	}
	return 0
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

const injectedDefaultsByDepth = new Map<number, number>()

/** Prometheus lines for how often a default was injected, by nesting depth. */
export function renderPaginationDefaultMetrics(): string {
	const lines = [
		"# HELP gaia_api_pagination_default_injected_total Collections given a default `first` because the client supplied no pagination, by collection nesting depth (0 = root).",
		"# TYPE gaia_api_pagination_default_injected_total counter",
	]
	for (const [depth, count] of [...injectedDefaultsByDepth.entries()].sort((a, b) => a[0] - b[0])) {
		lines.push(
			`gaia_api_pagination_default_injected_total{depth="${depth}",limit="${defaultLimitForDepth(depth)}"} ${count}`,
		)
	}
	return `${lines.join("\n")}\n`
}

/** Reset accumulated metric state. Tests only. */
export function __resetPaginationMetricsForTests(): void {
	injectedDefaultsByDepth.clear()
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

				const depth = collectionDepthOf(queryBuilder as TaggedQueryBuilder)
				// Tag before doing anything else, so children see this level even
				// if no default is injected here (an explicit `first` still counts
				// as a collection for the purpose of depth).
				;(queryBuilder as TaggedQueryBuilder)[COLLECTION_DEPTH] = depth

				const hasFirst = typeof fieldArgs.first === "number"
				const hasLast = typeof fieldArgs.last === "number"
				if (!hasFirst && !hasLast) {
					injectedDefaultsByDepth.set(depth, (injectedDefaultsByDepth.get(depth) ?? 0) + 1)
					queryBuilder.first(defaultLimitForDepth(depth))
				}
			},
		}))

		return args
	})
}

export default PaginationCapPlugin
