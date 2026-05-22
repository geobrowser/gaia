/**
 * Custom PostGraphile plugin that rewrites filters on the computed
 * `entity.name` and `entity.description` fields to use indexed EXISTS
 * subqueries on the `values` table — same pattern as `EntitySpaceFilterPlugin`.
 *
 * NOT a text-search primitive:
 *   This plugin makes equality / existence / `in` filters fast (the dominant
 *   prod use cases). Pattern-matching ops (`includes`, `like`, `startsWith`,
 *   `endsWith`, and their `Insensitive` / negated variants) get the same
 *   indexed (entity_id, property_id) lookup floor, but the LIKE/ILIKE part
 *   inside the EXISTS still scans the matching value rows row-by-row and
 *   may not exploit the trigram GIN index on `values.text` for broad
 *   entity scans. For real full-text search, use the `/search` endpoint
 *   (OpenSearch-backed) — it has a dedicated index path with ranking and
 *   stop-word handling. We emit a warn-level log when a caller uses
 *   `name`/`description` with a pattern-matching op so we can surface
 *   "this should be a /search call" in observability.
 *
 * Why:
 *   `entities.name` and `entities.description` are not real columns. They're
 *   exposed by the `entities_name(entity)` / `entities_description(entity)`
 *   STABLE SQL functions in migration `0027_property-entity-functions.sql`,
 *   which each do:
 *
 *     SELECT text FROM values WHERE entity_id = entity.id
 *                               AND property_id = NAME_PROPERTY LIMIT 1
 *
 *   `postgraphile-plugin-connection-filter` auto-generates an
 *   `EntityFilter.name`/`EntityFilter.description` field whose default
 *   resolver translates to `WHERE entities_name(e) <op> $1`. Because the
 *   function is STABLE (not IMMUTABLE), Postgres re-evaluates it for every
 *   candidate row and cannot use any index on it. Empirically this turns
 *   `name: {isNull: false, isNot: ""}` into a 25-second filter on a busy
 *   space.
 *
 *   This plugin overrides the default resolver for those two fields so the
 *   generated SQL is one merged EXISTS containing every operator's predicate
 *   AND-ed inside it:
 *
 *     EXISTS (
 *       SELECT 1 FROM public.values v
 *       WHERE v.entity_id = e.id
 *         AND v.property_id = $NAME_PROPERTY
 *         AND <pred_1> AND <pred_2> AND … <pred_N>
 *       LIMIT 1
 *     )
 *
 *   That uses `values_entity_property_idx (entity_id, property_id)` and
 *   short-circuits at the first match. Same `entity.name` semantics on the
 *   read path; just a different SQL shape for the filter path.
 *
 * Multi-operator semantics (the merge):
 *   The previous design emitted a separate `EXISTS (…)` per operator and
 *   AND-ed them. That broke for cross-space multi-value entities: with
 *   names {"Alice" in space A, "Beta-Z" in space B}, the filter
 *   `{startsWith: "A", endsWith: "Z"}` would match — startsWith finds
 *   "Alice", endsWith finds "Beta-Z" — even though no single value satisfies
 *   both predicates. The original `entities_name(e) LIKE 'A%' AND
 *   entities_name(e) LIKE '%Z'` evaluates against ONE arbitrarily-chosen
 *   value (LIMIT 1) and produces no match in that case.
 *
 *   Merging all positive/negative operator predicates into a single EXISTS
 *   restores "at least one value satisfies every operator" semantics — the
 *   same row must pass every predicate. `isNull: true` ("entity has no
 *   value at all") and any unknown-operator fallbacks remain as standalone
 *   top-level clauses since they assert facts that don't apply to a single
 *   row.
 *
 * Implementation:
 *   `connectionFilterRegisterResolver(typeName, fieldName, resolve)` is
 *   exposed by `PgConnectionArgFilterPlugin` in its `build` hook. The
 *   computed-columns plugin then registers the default resolver during the
 *   `GraphQLInputObjectType:fields` phase. Because graphile's `extend`
 *   throws on duplicate keys, calling register twice for the same field is
 *   not allowed.
 *
 *   Strategy: wrap `connectionFilterRegisterResolver` in our `build` hook
 *   (we run AFTER the connection-filter plugin's `build` hook, since we're
 *   appended after `ConnectionFilterPlugin` in `postgraphile.ts`), so when
 *   the computed-columns plugin later calls register for `EntityFilter.name`
 *   our wrapper substitutes our EXISTS-form resolver instead. The original
 *   register is bypassed for those two field names; everything else passes
 *   through unchanged.
 *
 * See also:
 *   - `entitySpaceFilterPlugin.ts` — same EXISTS pattern for spaceId / typeId
 *   - postgraphile-plugin-connection-filter source
 *     `dist/PgConnectionArgFilterComputedColumnsPlugin.js`
 */

import {SystemIds} from "@geoprotocol/geo-sdk"
import {log} from "../services/telemetry"

// Pulled from the SDK's canonical IDs; UUIDs annotated for grep-ability.
const NAME_PROPERTY_ID = SystemIds.NAME_PROPERTY // a126ca53-0c8e-48d5-b888-82c734c38935
const DESCRIPTION_PROPERTY_ID = SystemIds.DESCRIPTION_PROPERTY // 9b1f76ff-9711-404c-861e-59dc3fa7d037

const PROPERTY_FOR_FIELD: Record<string, string> = {
	name: NAME_PROPERTY_ID,
	description: DESCRIPTION_PROPERTY_ID,
}

const TARGET_FILTER_TYPE = "EntityFilter"

/**
 * Operators that imply the caller is doing text search on a name/description.
 * These should really go through the OpenSearch-backed `/search` endpoint
 * instead of the GraphQL graph traversal. We still translate them (to give
 * the same speedup floor as equality filters), but emit a warn-level log
 * each time so we can surface usage in Sentry / kubectl / Axiom.
 */
const SEARCH_LIKE_OPERATORS: ReadonlySet<string> = new Set([
	"includes",
	"includesInsensitive",
	"notIncludes",
	"notIncludesInsensitive",
	"like",
	"likeInsensitive",
	"notLike",
	"notLikeInsensitive",
	"startsWith",
	"startsWithInsensitive",
	"notStartsWith",
	"notStartsWithInsensitive",
	"endsWith",
	"endsWithInsensitive",
	"notEndsWith",
	"notEndsWithInsensitive",
])

// biome-ignore lint/suspicious/noExplicitAny: graphile build object is untyped
type Sql = any

/**
 * Result of translating a single operator.
 *   - `merge`: the operator contributes an inner predicate that gets
 *     AND-ed with other `merge` predicates inside one shared EXISTS,
 *     so they must all match the SAME value row.
 *   - `standalone`: the operator already produced a top-level fragment
 *     (e.g. `NOT EXISTS (…)` for `isNull: true`) that's AND-ed alongside
 *     the merged EXISTS at the outer level.
 *   - `null`: unknown operator — caller falls back to the default resolver.
 */
type OpResult = {kind: "merge"; innerPred: Sql} | {kind: "standalone"; fragment: Sql} | null

const merge = (innerPred: Sql): OpResult => ({kind: "merge", innerPred})
const standalone = (fragment: Sql): OpResult => ({kind: "standalone", fragment})

/**
 * Build the merged EXISTS subquery: a single EXISTS containing all the
 * `merge`-kind predicates AND-ed together. All predicates apply to the
 * same value row, which is what guarantees "some single value satisfies
 * every operator."
 */
function buildMergedExists(sql: Sql, sourceAlias: Sql, propertyId: string, innerPreds: Sql[]) {
	// pg-sql2's `sql.join` requires a string separator (it wraps it in
	// makeRawNode internally and throws "Invalid separator - must be a
	// string" on anything else — including a fragment).
	const predClause = innerPreds.length === 1 ? innerPreds[0] : sql.join(innerPreds, " AND ")
	return sql.fragment`
		EXISTS (
			SELECT 1 FROM public.values v
			WHERE v.entity_id = ${sourceAlias}.id
			  AND v.property_id = ${sql.value(propertyId)}::uuid
			  AND ${predClause}
			LIMIT 1
		)
	`
}

/**
 * Standalone "no value exists" fragment, used by `isNull: true`. It
 * cannot share a row with merge-kind predicates because it asserts the
 * absence of any qualifying row at all.
 */
function buildNoValueFragment(sql: Sql, sourceAlias: Sql, propertyId: string) {
	return sql.fragment`
		NOT EXISTS (
			SELECT 1 FROM public.values v
			WHERE v.entity_id = ${sourceAlias}.id
			  AND v.property_id = ${sql.value(propertyId)}::uuid
			  AND v.text IS NOT NULL
			LIMIT 1
		)
	`
}

/**
 * Translate a single TextFilter operator + its value into either an inner
 * predicate (to merge) or a standalone fragment (to emit at the top level).
 *
 * Returns `null` for unknown operators so the caller can fall back to the
 * default resolver. The full set of String operators connection-filter
 * generates is enumerated explicitly; anything else (e.g. `distinctFrom`,
 * `inInsensitive`, future additions) takes the slow per-row path.
 *
 * NULL / missing-value semantics for negative operators:
 *   The unrewritten `WHERE entities_name(e) NOT LIKE 'foo'` form excludes
 *   entities without a name, because `NULL NOT LIKE x` evaluates to NULL
 *   under SQL three-valued logic. To preserve that, every "not <op>"
 *   below produces `v.text IS NOT NULL AND NOT (<op>)` so a name-less
 *   entity (no row) returns FALSE and is correctly excluded. `isNot` /
 *   `notEqualTo` already get this for free because `v.text <> $val` is
 *   NULL on NULL rows.
 */
function operatorPredicate(sql: Sql, sourceAlias: Sql, propertyId: string, op: string, val: unknown): OpResult {
	const valLiteral = (v: unknown) => sql.value(v as string)
	// Inner predicate for "v.text IS NOT NULL AND NOT (<inner>)" — the
	// shape every negative operator needs to preserve SQL NULL semantics.
	const negPred = (inner: Sql) => sql.fragment`v.text IS NOT NULL AND NOT (${inner})`

	switch (op) {
		// --- Existence ---
		case "isNull":
			// {isNull: false} → "entity has at least one non-null value" — merges as a
			// predicate alongside other ops (so they must all match a non-null row).
			// {isNull: true}  → "no non-null value exists" — standalone, can't share
			// a row with merge-kind predicates.
			return val === true
				? standalone(buildNoValueFragment(sql, sourceAlias, propertyId))
				: merge(sql.fragment`v.text IS NOT NULL`)

		// --- Equality ---
		case "is":
		case "equalTo":
			return merge(sql.fragment`v.text = ${valLiteral(val)}`)
		case "isNot":
		case "notEqualTo":
			// `v.text <> $val` is NULL on NULL rows → already excludes them
			// without an explicit IS NOT NULL.
			return merge(sql.fragment`v.text <> ${valLiteral(val)}`)

		// --- equalTo / notEqualTo insensitive (renamed via connectionFilterOperatorNames) ---
		case "isInsensitive":
		case "equalToInsensitive":
			return merge(sql.fragment`lower(v.text) = lower(${valLiteral(val)})`)
		case "isNotInsensitive":
		case "notEqualToInsensitive":
			return merge(negPred(sql.fragment`lower(v.text) = lower(${valLiteral(val)})`))

		case "in":
			return merge(sql.fragment`v.text = ANY(${sql.value(val)}::text[])`)
		case "notIn":
			return merge(negPred(sql.fragment`v.text = ANY(${sql.value(val)}::text[])`))

		// --- Comparisons ---
		case "lessThan":
			return merge(sql.fragment`v.text < ${valLiteral(val)}`)
		case "lessThanOrEqualTo":
			return merge(sql.fragment`v.text <= ${valLiteral(val)}`)
		case "greaterThan":
			return merge(sql.fragment`v.text > ${valLiteral(val)}`)
		case "greaterThanOrEqualTo":
			return merge(sql.fragment`v.text >= ${valLiteral(val)}`)

		// --- Pattern matching (LIKE / ILIKE) ---
		case "includes":
			return merge(sql.fragment`v.text LIKE ${sql.value(`%${val}%`)}`)
		case "includesInsensitive":
			return merge(sql.fragment`v.text ILIKE ${sql.value(`%${val}%`)}`)
		case "notIncludes":
			return merge(negPred(sql.fragment`v.text LIKE ${sql.value(`%${val}%`)}`))
		case "notIncludesInsensitive":
			return merge(negPred(sql.fragment`v.text ILIKE ${sql.value(`%${val}%`)}`))
		case "startsWith":
			return merge(sql.fragment`v.text LIKE ${sql.value(`${val}%`)}`)
		case "startsWithInsensitive":
			return merge(sql.fragment`v.text ILIKE ${sql.value(`${val}%`)}`)
		case "notStartsWith":
			return merge(negPred(sql.fragment`v.text LIKE ${sql.value(`${val}%`)}`))
		case "notStartsWithInsensitive":
			return merge(negPred(sql.fragment`v.text ILIKE ${sql.value(`${val}%`)}`))
		case "endsWith":
			return merge(sql.fragment`v.text LIKE ${sql.value(`%${val}`)}`)
		case "endsWithInsensitive":
			return merge(sql.fragment`v.text ILIKE ${sql.value(`%${val}`)}`)
		case "notEndsWith":
			return merge(negPred(sql.fragment`v.text LIKE ${sql.value(`%${val}`)}`))
		case "notEndsWithInsensitive":
			return merge(negPred(sql.fragment`v.text ILIKE ${sql.value(`%${val}`)}`))
		case "like":
			return merge(sql.fragment`v.text LIKE ${valLiteral(val)}`)
		case "likeInsensitive":
			return merge(sql.fragment`v.text ILIKE ${valLiteral(val)}`)
		case "notLike":
			return merge(negPred(sql.fragment`v.text LIKE ${valLiteral(val)}`))
		case "notLikeInsensitive":
			return merge(negPred(sql.fragment`v.text ILIKE ${valLiteral(val)}`))

		default:
			// Unknown operator (e.g. distinctFrom, notDistinctFrom,
			// inInsensitive, *Insensitive comparisons, future additions).
			// Caller falls back to the default per-row entities_<field>()
			// function call for this op only.
			return null
	}
}

/**
 * Build a hybrid resolver for a given (fieldName, propertyId) pair.
 *
 * Per-op classification:
 *   - merge ops contribute predicates collected into ONE EXISTS, so all
 *     must match the same value row;
 *   - standalone ops emit their own top-level fragment;
 *   - unknown ops fall back to the default resolver per-op (correct, slow).
 *
 * All produced fragments are AND-joined.
 */
function buildResolver(
	graphqlFieldName: string,
	propertyId: string,
	sql: Sql,
	// biome-ignore lint/suspicious/noExplicitAny: connection-filter resolver shape
	defaultResolve: any,
) {
	// biome-ignore lint/suspicious/noExplicitAny: connection-filter resolver shape
	return (input: any): Sql | null => {
		const {sourceAlias, fieldValue} = input
		if (fieldValue == null || typeof fieldValue !== "object") return null

		const innerPreds: Sql[] = []
		const standaloneFragments: Sql[] = []

		for (const [op, val] of Object.entries(fieldValue)) {
			if (SEARCH_LIKE_OPERATORS.has(op)) {
				// Surface this usage so we can spot callers that should be
				// using `/search` instead. Truncates the value to 200 chars
				// in case someone passes a giant pattern, and never logs the
				// result set — this is purely a "you used the wrong endpoint"
				// signal. log.warn = breadcrumb in Sentry + stdout, no issue.
				const truncatedValue = typeof val === "string" ? val.slice(0, 200) : val
				log.warn("GraphQL pattern-match filter on entity name/description — prefer /search", {
					field: graphqlFieldName,
					operator: op,
					value: truncatedValue,
				})
			}
			const result = operatorPredicate(sql, sourceAlias, propertyId, op, val)
			if (result == null) {
				// Unknown op → delegate to the default resolver with a single-op
				// view of the filter so it produces SQL only for this one op.
				const slowFragment = defaultResolve({...input, fieldValue: {[op]: val}})
				if (slowFragment != null) standaloneFragments.push(slowFragment)
			} else if (result.kind === "merge") {
				innerPreds.push(result.innerPred)
			} else {
				standaloneFragments.push(result.fragment)
			}
		}

		const fragments = [...standaloneFragments]
		if (innerPreds.length > 0) {
			fragments.push(buildMergedExists(sql, sourceAlias, propertyId, innerPreds))
		}

		if (fragments.length === 0) return null
		if (fragments.length === 1) return fragments[0]
		return sql.fragment`(${sql.join(fragments, " AND ")})`
	}
}

/**
 * Plugin entry point. Wraps `connectionFilterRegisterResolver` so that
 * the `EntityFilter.name` and `EntityFilter.description` registrations
 * (made later by PgConnectionArgFilterComputedColumnsPlugin) are replaced
 * with our EXISTS-based resolvers. All other registrations pass through.
 */
// biome-ignore lint/suspicious/noExplicitAny: graphile builder is untyped
export const EntityComputedTextFilterPlugin = (builder: any) => {
	// biome-ignore lint/suspicious/noExplicitAny: build hook is untyped
	builder.hook("build", (build: any) => {
		const sql = build.pgSql
		const original = build.connectionFilterRegisterResolver
		if (typeof original !== "function" || !sql) {
			// connection-filter plugin not loaded (or older shape) — no-op.
			return build
		}

		// Replace `build.connectionFilterRegisterResolver` with a wrapper
		// that substitutes a hybrid resolver for known (typeName, fieldName)
		// pairs — falling back to the default resolver per-op for any ops
		// our fast-path doesn't handle. Mutates `build` directly because
		// `build.extend` would trip on the existing key.
		// biome-ignore lint/suspicious/noExplicitAny: registering with untyped graphile callback
		build.connectionFilterRegisterResolver = (typeName: string, fieldName: string, defaultResolve: any) => {
			const propertyId = typeName === TARGET_FILTER_TYPE ? PROPERTY_FOR_FIELD[fieldName] : undefined
			if (propertyId) {
				const hybrid = buildResolver(fieldName, propertyId, sql, defaultResolve)
				return original(typeName, fieldName, hybrid)
			}
			return original(typeName, fieldName, defaultResolve)
		}

		return build
	})
}

export default EntityComputedTextFilterPlugin

// Test-only exports — used by unit tests to verify the SQL fragments.
export const __testExports = {
	NAME_PROPERTY_ID,
	DESCRIPTION_PROPERTY_ID,
	buildResolver,
	operatorPredicate,
	buildMergedExists,
}
