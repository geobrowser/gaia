import {GraphQLError} from "graphql"

// Error codes whose GraphQL response should be sent to the client verbatim
// (not masked to "Unexpected error."). These convey actionable information:
// BAD_USER_INPUT tells the caller to fix their query; SERVICE_UNAVAILABLE
// tells them to retry.
const UNMASKED_ERROR_CODES = new Set(["BAD_USER_INPUT", "SERVICE_UNAVAILABLE"])

export function shouldUnmaskError(error: unknown): error is GraphQLError {
	if (!(error instanceof GraphQLError)) {
		return false
	}

	const code = error.extensions?.code
	if (typeof code === "string" && UNMASKED_ERROR_CODES.has(code)) {
		return true
	}

	if (error.originalError instanceof GraphQLError) {
		const origCode = error.originalError.extensions?.code
		if (typeof origCode === "string" && UNMASKED_ERROR_CODES.has(origCode)) {
			return true
		}
	}

	return false
}
