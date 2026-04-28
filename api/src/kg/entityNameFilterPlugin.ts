/**
 * Custom PostGraphile plugin that rewrites filters on the computed
 * `entity.name` and `entity.description` fields to use indexed EXISTS
 * subqueries on the `values` table — same pattern as `EntitySpaceFilterPlugin`.
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
 *   generated SQL is:
 *
 *     EXISTS (
 *       SELECT 1 FROM public.values v
 *       WHERE v.entity_id = e.id
 *         AND v.property_id = $NAME_PROPERTY
 *         AND v.text <op> $1   -- per operator
 *       LIMIT 1
 *     )
 *
 *   That uses `values_entity_property_idx (entity_id, property_id)` and
 *   short-circuits at the first match. Same `entity.name` semantics on the
 *   read path; just a different SQL shape for the filter path.
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

const NAME_PROPERTY_ID = "a126ca53-0c8e-48d5-b888-82c734c38935"
const DESCRIPTION_PROPERTY_ID = "9b1f76ff-9711-404c-861e-59dc3fa7d037"

const PROPERTY_FOR_FIELD: Record<string, string> = {
	name: NAME_PROPERTY_ID,
	description: DESCRIPTION_PROPERTY_ID,
}

const TARGET_FILTER_TYPE = "EntityFilter"

// biome-ignore lint/suspicious/noExplicitAny: graphile build object is untyped
type Sql = any

/**
 * Build a SQL fragment that ANDs into a base EXISTS subquery on `values`,
 * filtering for a given (entity_id, property_id) lookup with an extra
 * predicate on `v.text` (or none, for pure existence checks).
 */
function existsFragment(sql: Sql, sourceAlias: Sql, propertyId: string, textPredicate: Sql | null) {
	const base = sql.fragment`
		EXISTS (
			SELECT 1 FROM public.values v
			WHERE v.entity_id = ${sourceAlias}.id
			  AND v.property_id = ${sql.value(propertyId)}::uuid
			  ${textPredicate ?? sql.fragment``}
			LIMIT 1
		)
	`
	return base
}

const negate = (sql: Sql, fragment: Sql) => sql.fragment`NOT ${fragment}`

/**
 * Translate a single TextFilter operator + its value into a SQL fragment
 * that semantically matches "this entity has at least one (or zero, for
 * negative ops) value row matching the predicate."
 *
 * Returns `null` for unknown operators so the caller skips them rather
 * than failing the whole filter.
 */
function operatorFragment(sql: Sql, sourceAlias: Sql, propertyId: string, op: string, val: unknown): Sql | null {
	const exists = (textPred: Sql | null) => existsFragment(sql, sourceAlias, propertyId, textPred)

	const textPred = (cond: Sql) => sql.fragment`AND ${cond}`

	const valLiteral = (v: unknown) => sql.value(v as string)

	switch (op) {
		// --- Existence ---
		case "isNull":
			// {isNull: true}  → no row exists with non-null text  → entity has no name
			// {isNull: false} → at least one row with non-null text → entity has a name
			return val === true
				? negate(sql, exists(textPred(sql.fragment`v.text IS NOT NULL`)))
				: exists(textPred(sql.fragment`v.text IS NOT NULL`))

		// --- Equality ---
		case "is":
		case "equalTo":
			return exists(textPred(sql.fragment`v.text = ${valLiteral(val)}`))
		case "isNot":
		case "notEqualTo":
			// Special-case `isNot: ""` to "has at least one non-empty text"
			// rather than the literal "≠ ''" semantics, since callers use
			// it that way in practice. Both interpretations produce the
			// same EXISTS form.
			return exists(textPred(sql.fragment`v.text <> ${valLiteral(val)}`))

		case "in":
			return exists(textPred(sql.fragment`v.text = ANY(${sql.value(val)}::text[])`))
		case "notIn":
			return negate(sql, exists(textPred(sql.fragment`v.text = ANY(${sql.value(val)}::text[])`)))

		// --- Comparisons ---
		case "lessThan":
			return exists(textPred(sql.fragment`v.text < ${valLiteral(val)}`))
		case "lessThanOrEqualTo":
			return exists(textPred(sql.fragment`v.text <= ${valLiteral(val)}`))
		case "greaterThan":
			return exists(textPred(sql.fragment`v.text > ${valLiteral(val)}`))
		case "greaterThanOrEqualTo":
			return exists(textPred(sql.fragment`v.text >= ${valLiteral(val)}`))

		// --- Pattern matching (LIKE / ILIKE) ---
		case "includes":
			return exists(textPred(sql.fragment`v.text LIKE ${sql.value(`%${val}%`)}`))
		case "includesInsensitive":
			return exists(textPred(sql.fragment`v.text ILIKE ${sql.value(`%${val}%`)}`))
		case "notIncludes":
			return negate(sql, exists(textPred(sql.fragment`v.text LIKE ${sql.value(`%${val}%`)}`)))
		case "notIncludesInsensitive":
			return negate(sql, exists(textPred(sql.fragment`v.text ILIKE ${sql.value(`%${val}%`)}`)))
		case "startsWith":
			return exists(textPred(sql.fragment`v.text LIKE ${sql.value(`${val}%`)}`))
		case "startsWithInsensitive":
			return exists(textPred(sql.fragment`v.text ILIKE ${sql.value(`${val}%`)}`))
		case "endsWith":
			return exists(textPred(sql.fragment`v.text LIKE ${sql.value(`%${val}`)}`))
		case "endsWithInsensitive":
			return exists(textPred(sql.fragment`v.text ILIKE ${sql.value(`%${val}`)}`))
		case "like":
			return exists(textPred(sql.fragment`v.text LIKE ${valLiteral(val)}`))
		case "likeInsensitive":
			return exists(textPred(sql.fragment`v.text ILIKE ${valLiteral(val)}`))
		case "notLike":
			return negate(sql, exists(textPred(sql.fragment`v.text LIKE ${valLiteral(val)}`)))
		case "notLikeInsensitive":
			return negate(sql, exists(textPred(sql.fragment`v.text ILIKE ${valLiteral(val)}`)))

		default:
			// Unknown operator (e.g. distinctFrom, range ops). Fall through
			// to the default proc-call resolver by returning null — the
			// wrapped resolver will signal "not handled".
			return null
	}
}

/**
 * Build the resolver function we register with connection-filter for a
 * given (fieldName, propertyId) pair. The resolver is called once per
 * filter usage with the `fieldValue` being the operators bag like
 * `{isNull: false, isNot: ""}`.
 */
function buildResolver(propertyId: string, sql: Sql) {
	// biome-ignore lint/suspicious/noExplicitAny: connection-filter resolver shape
	return (input: any): Sql | null => {
		const {sourceAlias, fieldValue} = input
		if (fieldValue == null || typeof fieldValue !== "object") return null

		const fragments: Sql[] = []
		for (const [op, val] of Object.entries(fieldValue)) {
			const fragment = operatorFragment(sql, sourceAlias, propertyId, op, val)
			if (fragment) fragments.push(fragment)
		}

		if (fragments.length === 0) return null
		if (fragments.length === 1) return fragments[0]
		// Multiple operators in one filter object are AND-ed together
		// (e.g. `{isNull: false, isNot: ""}` means "has a value AND it's not '").
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
export const EntityNameFilterPlugin = (builder: any) => {
	// biome-ignore lint/suspicious/noExplicitAny: build hook is untyped
	builder.hook("build", (build: any) => {
		const sql = build.pgSql
		const original = build.connectionFilterRegisterResolver
		if (typeof original !== "function" || !sql) {
			// connection-filter plugin not loaded (or older shape) — no-op.
			return build
		}

		const overrides = new Map<string, ReturnType<typeof buildResolver>>()
		for (const [fieldName, propertyId] of Object.entries(PROPERTY_FOR_FIELD)) {
			overrides.set(`${TARGET_FILTER_TYPE}.${fieldName}`, buildResolver(propertyId, sql))
		}

		// Replace `build.connectionFilterRegisterResolver` with a wrapper
		// that substitutes our resolver for known (typeName, fieldName)
		// pairs. Mutates `build` directly because `build.extend` would
		// trip on the existing key.
		// biome-ignore lint/suspicious/noExplicitAny: registering with untyped graphile callback
		build.connectionFilterRegisterResolver = (typeName: string, fieldName: string, resolve: any) => {
			const key = `${typeName}.${fieldName}`
			const override = overrides.get(key)
			if (override) {
				return original(typeName, fieldName, override)
			}
			return original(typeName, fieldName, resolve)
		}

		return build
	})
}

export default EntityNameFilterPlugin

// Test-only exports — used by unit tests to verify the SQL fragments.
export const __testExports = {
	NAME_PROPERTY_ID,
	DESCRIPTION_PROPERTY_ID,
	buildResolver,
	operatorFragment,
}
