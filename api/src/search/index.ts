/**
 * Search route handler.
 *
 * Provides HTTP endpoints for full-text search across the Knowledge Graph.
 */

import {Data, Effect, Either} from "effect"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"

import type {AppRuntime} from "../services/runtime"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

import type {SearchClient, SearchResponse, SearchScope} from "../services/search"
import {isValidUuid, toUuid} from "../utils/uuid"

/**
 * Valid search scope values.
 */
const VALID_SCOPES: Set<SearchScope> = new Set(["GLOBAL", "GLOBAL_BY_SPACE_SCORE", "SPACE_SINGLE", "SPACE"])

/**
 * Default limit for search results.
 */
const DEFAULT_LIMIT = 20

/**
 * Maximum limit for search results.
 */
const MAX_LIMIT = 100

/**
 * Maximum length for search queries to prevent abuse.
 */
const MAX_QUERY_LENGTH = 500

/**
 * Maximum length for space_id parameter (UUID format).
 */
const MAX_SPACE_ID_LENGTH = 36

/**
 * Maximum offset to prevent excessive pagination.
 */
const MAX_OFFSET = 1000

/**
 * Maximum number of type IDs to prevent abuse.
 */
const MAX_TYPE_IDS = 10

/**
 * Valid query parameter names for the search endpoint.
 */
const VALID_PARAMS: Set<string> = new Set([
	"query",
	"q",
	"scope",
	"space_id",
	"type_ids",
	"limit",
	"offset",
	"include_deleted",
])

// Error types for search operations
class SearchValidationError extends Data.TaggedError("SearchValidationError")<{
	message: string
	status: 400
}> {}

class SearchExecutionError extends Data.TaggedError("SearchExecutionError")<{
	message: string
	status: 500
}> {}

type SearchError = SearchValidationError | SearchExecutionError

/**
 * Create the search router with dependency-injected search client.
 *
 * @param searchClient - The search client to use for queries
 * @param runtime - Effect runtime with telemetry and other services
 * @returns Configured Hono router
 */
export function createSearchRouter(searchClient: SearchClient, runtime: AppRuntime) {
	const router = new Hono<AppEnv>()

	/**
	 * GET /search
	 *
	 * Search for entities across the Knowledge Graph.
	 */
	router.get(
		"/",
		describeRoute({
			tags: ["Search"],
			summary: "Search for entities",
			description: "Search for entities across the Knowledge Graph with optional filters",
			parameters: [
				{
					name: "query",
					in: "query",
					description: "Search query string (alias: q)",
					required: false,
					schema: {type: "string", maxLength: 500},
				},
				{
					name: "q",
					in: "query",
					description: "Search query string (alias for query)",
					required: false,
					schema: {type: "string", maxLength: 500},
				},
				{
					name: "scope",
					in: "query",
					description: "Search scope",
					required: false,
					schema: {
						type: "string",
						enum: ["GLOBAL", "GLOBAL_BY_SPACE_SCORE", "SPACE_SINGLE", "SPACE"],
						default: "GLOBAL",
					},
				},
				{
					name: "space_id",
					in: "query",
					description: "Space UUID (required for SPACE_SINGLE and SPACE scopes)",
					required: false,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "type_ids",
					in: "query",
					description: "Comma-separated list of type UUIDs to filter by (max 10)",
					required: false,
					schema: {type: "string"},
				},
				{
					name: "limit",
					in: "query",
					description: "Maximum number of results to return",
					required: false,
					schema: {type: "integer", minimum: 1, maximum: 100, default: 20},
				},
				{
					name: "offset",
					in: "query",
					description: "Number of results to skip for pagination",
					required: false,
					schema: {type: "integer", minimum: 0, maximum: 1000, default: 0},
				},
			],
			responses: {
				200: {
					description: "Search results",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									results: {
										type: "array",
										items: {
											type: "object",
											properties: {
												id: {type: "string", format: "uuid"},
												name: {type: "string"},
												description: {type: "string"},
												types: {type: "array", items: {type: "string"}},
												spaces: {type: "array", items: {type: "string", format: "uuid"}},
												score: {type: "number"},
											},
										},
									},
									total: {type: "integer"},
								},
							},
						},
					},
				},
				400: {
					description: "Invalid parameter",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				500: {
					description: "Search failed",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
			},
		}),
		async (c) => {
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Check for unrecognized query parameters
				const allParams = Object.keys(c.req.query())
				const unrecognizedParams = allParams.filter((param) => !VALID_PARAMS.has(param))
				if (unrecognizedParams.length > 0) {
					return yield* Effect.fail(
						new SearchValidationError({
							message: `Unrecognized query parameter(s): ${unrecognizedParams.join(", ")}. Valid parameters are: ${Array.from(VALID_PARAMS).join(", ")}`,
							status: 400,
						}),
					)
				}

				// Extract query parameters
				const query = c.req.query("query") ?? c.req.query("q")
				const scopeParam = c.req.query("scope") ?? "GLOBAL"
				const spaceId = c.req.query("space_id")
				const typeIdsParam = c.req.query("type_ids")
				const limitParam = c.req.query("limit")
				const offsetParam = c.req.query("offset")
				const includeDeletedParam = c.req.query("include_deleted")

				// Validate query length
				const trimmedQuery = query?.trim() ?? ""
				if (trimmedQuery.length > MAX_QUERY_LENGTH) {
					return yield* Effect.fail(
						new SearchValidationError({
							message: `Query must not exceed ${MAX_QUERY_LENGTH} characters`,
							status: 400,
						}),
					)
				}

				// Validate scope
				if (!VALID_SCOPES.has(scopeParam as SearchScope)) {
					return yield* Effect.fail(
						new SearchValidationError({
							message: `Invalid scope '${scopeParam}'. Valid values: ${Array.from(VALID_SCOPES).join(", ")}`,
							status: 400,
						}),
					)
				}
				const scope = scopeParam as SearchScope

				// Validate space_id for space-scoped searches
				if (scope === "SPACE_SINGLE" || scope === "SPACE") {
					if (!spaceId) {
						return yield* Effect.fail(
							new SearchValidationError({
								message: `space_id is required for ${scope} scope`,
								status: 400,
							}),
						)
					}

					if (spaceId.length > MAX_SPACE_ID_LENGTH) {
						return yield* Effect.fail(
							new SearchValidationError({
								message: `space_id must not exceed ${MAX_SPACE_ID_LENGTH} characters`,
								status: 400,
							}),
						)
					}

					if (!isValidUuid(spaceId)) {
						return yield* Effect.fail(
							new SearchValidationError({
								message: "space_id must be a valid UUID",
								status: 400,
							}),
						)
					}
				}

				// Parse and validate limit
				let limit = DEFAULT_LIMIT
				if (limitParam) {
					const parsedLimit = parseInt(limitParam, 10)
					if (Number.isNaN(parsedLimit) || parsedLimit < 1) {
						return yield* Effect.fail(
							new SearchValidationError({
								message: "limit must be a positive integer",
								status: 400,
							}),
						)
					}
					limit = Math.min(parsedLimit, MAX_LIMIT)
				}

				// Parse and validate offset
				let offset = 0
				if (offsetParam) {
					const parsedOffset = parseInt(offsetParam, 10)
					if (Number.isNaN(parsedOffset) || parsedOffset < 0) {
						return yield* Effect.fail(
							new SearchValidationError({
								message: "offset must be a non-negative integer",
								status: 400,
							}),
						)
					}
					if (parsedOffset > MAX_OFFSET) {
						return yield* Effect.fail(
							new SearchValidationError({
								message: `offset must not exceed ${MAX_OFFSET}`,
								status: 400,
							}),
						)
					}
					offset = parsedOffset
				}

				// Parse and validate typeIds
				let typeIds: string[] | undefined
				if (typeIdsParam) {
					typeIds = typeIdsParam
						.split(",")
						.map((id) => id.trim())
						.filter((id) => id.length > 0)

					if (typeIds.length > MAX_TYPE_IDS) {
						return yield* Effect.fail(
							new SearchValidationError({
								message: `type_ids must not contain more than ${MAX_TYPE_IDS} IDs`,
								status: 400,
							}),
						)
					}

					for (const typeId of typeIds) {
						if (!isValidUuid(typeId)) {
							return yield* Effect.fail(
								new SearchValidationError({
									message: `type_ids must contain valid UUIDs, got invalid ID: ${typeId}`,
									status: 400,
								}),
							)
						}
					}
				}

				// Parse include_deleted flag (default: false)
				const includeDeleted = includeDeletedParam === "true"

				// Normalize IDs to dashed hex for OpenSearch term queries.
				// OpenSearch stores UUIDs as dashed hex keywords (from Rust Uuid::to_string()).
				const dashedSpaceId = spaceId ? toUuid(spaceId) : undefined
				const dashedTypeIds = typeIds?.map(toUuid)

				// Execute search - only include optional params when defined
				const searchQuery = {
					query: trimmedQuery,
					scope,
					limit,
					offset,
					...(dashedSpaceId && {space_id: dashedSpaceId}),
					...(dashedTypeIds && {type_ids: dashedTypeIds}),
					...(includeDeleted && {include_deleted: true}),
				}

				const response = yield* Effect.tryPromise({
					try: () => searchClient.search(searchQuery),
					catch: (error) =>
						new SearchExecutionError({
							message: error instanceof Error ? error.message : "An unexpected error occurred",
							status: 500,
						}),
				}).pipe(Effect.withSpan("search.execute"))

				return response
			}).pipe(Effect.withSpan("GET /search"), Effect.annotateLogs({requestId, route: "/search"}))

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: SearchError) => {
					if (error._tag === "SearchValidationError") {
						return c.json({error: "Invalid parameter", message: error.message}, 400)
					}
					return c.json({error: "Search failed", message: error.message}, 500)
				},
				onRight: (response: SearchResponse) => c.json(response),
			})
		},
	)

	/**
	 * GET /search/health
	 *
	 * Check the health of the search service.
	 */
	router.get(
		"/health",
		describeRoute({
			tags: ["Search"],
			summary: "Check search service health",
			description: "Returns the health status of the OpenSearch service",
			responses: {
				200: {
					description: "Search service is healthy",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									status: {type: "string", enum: ["healthy"]},
								},
								required: ["status"],
							},
						},
					},
				},
				503: {
					description: "Search service is unhealthy",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									status: {type: "string", enum: ["unhealthy"]},
								},
								required: ["status"],
							},
						},
					},
				},
			},
		}),
		async (c) => {
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				const healthy = yield* Effect.tryPromise({
					try: () => searchClient.healthCheck(),
					catch: () => new SearchExecutionError({message: "Health check failed", status: 500}),
				}).pipe(Effect.withSpan("search.healthCheck"))

				return healthy
			}).pipe(Effect.withSpan("GET /search/health"), Effect.annotateLogs({requestId}))

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: () => c.json({status: "unhealthy"}, 503),
				onRight: (healthy) => (healthy ? c.json({status: "healthy"}) : c.json({status: "unhealthy"}, 503)),
			})
		},
	)

	return router
}
