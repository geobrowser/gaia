import type {GraphQLError} from "graphql"

// Error codes whose GraphQL response should be sent to the client verbatim
// (not masked to "Unexpected error."). These convey actionable information:
// BAD_USER_INPUT tells the caller to fix their query; SERVICE_UNAVAILABLE
// tells them to retry.
const UNMASKED_ERROR_CODES = new Set(["BAD_USER_INPUT", "SERVICE_UNAVAILABLE"])

/**
 * How far to follow `originalError` looking for an unmaskable code.
 *
 * This used to check the error and exactly one level of `originalError`, which
 * is enough for an error raised at the root of an operation but not for one
 * raised while building a NESTED field's SQL: graphql-js wraps at each level it
 * propagates through, so a `BAD_USER_INPUT` thrown inside a nested collection's
 * `pgQuery` arrives two or more wrappers deep.
 *
 * Bounded rather than unbounded: the chain's length follows the query's own
 * nesting, which is caller-controlled, and a cycle must not turn error handling
 * into a hang.
 */
const MAX_ORIGINAL_ERROR_DEPTH = 10

/**
 * True for anything shaped like a GraphQLError carrying `extensions`.
 *
 * Deliberately structural rather than `instanceof GraphQLError`. The api's
 * dependency tree contains several copies of the `graphql` package — its own,
 * plus nested copies under `postgraphile` and `graphile-utils` — and a class
 * from one copy never satisfies `instanceof` against another's. Errors cross
 * that boundary routinely: the error is thrown by a plugin using the api's
 * copy, then wrapped by whichever copy is running execution.
 *
 * The consequence was silent and one-directional: `shouldUnmaskError` answered
 * false for errors it was written to pass through, and the caller got
 * "Unexpected error." instead of a message telling them what to fix. It only
 * ever masked too much, never too little, which is why it went unnoticed.
 *
 * Structural checking is the right tool here regardless of the duplicate
 * copies, because what this decision is actually about is the `extensions.code`
 * contract, not class identity.
 */
function extensionsCodeOf(value: unknown): string | undefined {
	if (typeof value !== "object" || value === null) return undefined
	const code = (value as {extensions?: {code?: unknown}}).extensions?.code
	return typeof code === "string" ? code : undefined
}

export function shouldUnmaskError(error: unknown): error is GraphQLError {
	if (!(error instanceof Error)) {
		return false
	}

	let current: unknown = error
	for (let depth = 0; current && depth <= MAX_ORIGINAL_ERROR_DEPTH; depth++) {
		const code = extensionsCodeOf(current)
		if (code !== undefined && UNMASKED_ERROR_CODES.has(code)) {
			return true
		}
		current = (current as {originalError?: unknown}).originalError
	}

	return false
}
