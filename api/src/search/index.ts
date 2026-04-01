/**
 * Search route handler.
 *
 * Provides HTTP endpoints for full-text search across the Knowledge Graph.
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {Data, Effect, Either} from "effect"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"

import type {AppRuntime} from "../services/runtime"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

import type {BoostOverrides, SearchClient, SearchResponse, SearchScope} from "../services/search"
import {isValidUuid} from "../utils/uuid"

/**
 * Valid search scope values.
 */
const VALID_SCOPES: Set<SearchScope> = new Set([
	"GLOBAL",
	"GLOBAL_BY_SPACE_SCORE",
	"GLOBAL_BY_ENTITY_SPACE_SCORE",
	"SPACE_SINGLE",
	"SPACE",
])

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
 * Maximum length for space_id parameter (UUID format: 36 dashed, 32 dashless).
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
 * Entity types excluded from search results by default.
 * These are block/media types that are not useful as standalone search results.
 * Users can override this by passing an explicit `exclude_type_ids` parameter.
 */
/**
 * Comment type entity ID. Not yet in the GRC-20 npm SDK.
 * Dashless format to match SystemIds convention.
 * Also defined in sdk/src/core/content_ids.rs (dashed, for Rust services).
 */
const COMMENT_TYPE = "82f6123a03234c6ca811701c5bc026e9"

const DEFAULT_EXCLUDED_TYPE_IDS: string[] = [
	SystemIds.TEXT_BLOCK,
	SystemIds.IMAGE_BLOCK,
	SystemIds.DATA_BLOCK,
	SystemIds.IMAGE_TYPE,
	SystemIds.VIDEO_TYPE,
	SystemIds.VIDEO_BLOCK,
	COMMENT_TYPE,
]

/**
 * Valid query parameter names for the search endpoint.
 */
const BOOST_PARAMS = [
	"score_boost",
	"name_prefix_boost",
	"description_prefix_boost",
	"name_field_boost",
	"name_exact_token_boost",
	"name_raw_exact_boost",
	"name_raw_case_insensitive_boost",
	"fuzzy_reduction_boost",
] as const

const VALID_PARAMS: Set<string> = new Set([
	"query",
	"q",
	"scope",
	"space_id",
	"type_ids",
	"exclude_type_ids",
	"limit",
	"offset",
	"include_deleted",
	"include_non_canonical",
	...BOOST_PARAMS,
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
						enum: [
							"GLOBAL",
							"GLOBAL_BY_SPACE_SCORE",
							"GLOBAL_BY_ENTITY_SPACE_SCORE",
							"SPACE_SINGLE",
							"SPACE",
						],
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
					name: "exclude_type_ids",
					in: "query",
					description:
						"Comma-separated list of type UUIDs to exclude from results (max 10). When omitted, default block/media types are excluded. Pass an empty string to disable default exclusions.",
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
				{
					name: "include_non_canonical",
					in: "query",
					description:
						"Whether to include entities from spaces outside the canonical graph. Defaults to true (all entities returned). Set to false to restrict results to canonical spaces only. The canonical graph is the trust-based subset of spaces rooted at the configured root space, connected by verified/related/editor/member edges.",
					required: false,
					schema: {type: "boolean", default: true},
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
												entityId: {type: "string", format: "uuid"},
												space: {
													type: "object",
													description:
														"The space this entity belongs to, with optional metadata",
													properties: {
														id: {type: "string", format: "uuid"},
														name: {type: "string"},
														description: {type: "string"},
														avatar: {type: "string"},
														cover: {type: "string"},
													},
													required: ["id"],
												},
												name: {type: "string"},
												description: {type: "string"},
												avatar: {type: "string"},
												cover: {type: "string"},
												types: {
													type: "array",
													items: {
														type: "object",
														properties: {
															id: {type: "string", format: "uuid"},
															name: {type: "string"},
														},
														required: ["id"],
													},
													description:
														"Types associated with this entity, with optional names",
												},
												entityGlobalScore: {type: "number", description: "Global entity score"},
												spaceScore: {type: "number", description: "Space score"},
												entitySpaceScore: {type: "number", description: "Entity-space score"},
												relevanceScore: {
													type: "number",
													description:
														"Final relevance score after all boosts including entity/space score factors",
												},
												textMatchScore: {
													type: "number",
													description:
														"Text matching score without score field boosts, reflecting pure query relevance",
												},
												inCanonicalGraph: {
													type: "boolean",
													description:
														"Whether this entity's space is part of the canonical graph (trust-based subset of spaces)",
												},
											},
										},
									},
									total: {type: "integer", description: "Total number of matching documents"},
									tookMs: {
										type: "number",
										description: "Time taken to execute the search in milliseconds",
									},
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
				const excludeTypeIdsParam = c.req.query("exclude_type_ids")
				const limitParam = c.req.query("limit")
				const offsetParam = c.req.query("offset")
				const includeDeletedParam = c.req.query("include_deleted")
				const includeNonCanonicalParam = c.req.query("include_non_canonical")

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

				// Parse and validate excludeTypeIds
				// - undefined (param not provided): use default exclusions
				// - empty string (param provided with no value): no exclusions
				// - comma-separated IDs: exclude those specific types
				let excludeTypeIds: string[] | undefined
				if (excludeTypeIdsParam !== undefined) {
					if (excludeTypeIdsParam === "") {
						// Explicitly empty — disable default exclusions
						excludeTypeIds = []
					} else {
						excludeTypeIds = excludeTypeIdsParam
							.split(",")
							.map((id) => id.trim())
							.filter((id) => id.length > 0)

						if (excludeTypeIds.length > MAX_TYPE_IDS) {
							return yield* Effect.fail(
								new SearchValidationError({
									message: `exclude_type_ids must not contain more than ${MAX_TYPE_IDS} IDs`,
									status: 400,
								}),
							)
						}

						for (const typeId of excludeTypeIds) {
							if (!isValidUuid(typeId)) {
								return yield* Effect.fail(
									new SearchValidationError({
										message: `exclude_type_ids must contain valid UUIDs, got invalid ID: ${typeId}`,
										status: 400,
									}),
								)
							}
						}
					}
				} else {
					// Default: exclude block/media types
					excludeTypeIds = DEFAULT_EXCLUDED_TYPE_IDS
				}

				// Resolve conflicts between type_ids and exclude_type_ids
				if (typeIds && excludeTypeIds && excludeTypeIds.length > 0) {
					const includeSet = new Set(typeIds)
					if (excludeTypeIdsParam === undefined) {
						// Default exclusions: explicit type_ids take priority — strip overlaps
						excludeTypeIds = excludeTypeIds.filter((id) => !includeSet.has(id))
					} else {
						// User-supplied exclusions: conflicting with type_ids is an error
						const conflicting = excludeTypeIds.filter((id) => includeSet.has(id))
						if (conflicting.length > 0) {
							return yield* Effect.fail(
								new SearchValidationError({
									message: `type_ids and exclude_type_ids must not contain the same IDs: ${conflicting.join(", ")}`,
									status: 400,
								}),
							)
						}
					}
				}

				// Parse include_deleted flag (default: false)
				const includeDeleted = includeDeletedParam === "true"

				// Parse include_non_canonical flag (default: true)
				const includeNonCanonical = includeNonCanonicalParam !== "false"

				// Parse optional boost overrides (undocumented — for internal testing)
				const boosts: BoostOverrides = {}
				for (const param of BOOST_PARAMS) {
					const raw = c.req.query(param)
					if (raw !== undefined) {
						const value = Number.parseFloat(raw)
						if (Number.isNaN(value) || value < 0) {
							return yield* Effect.fail(
								new SearchValidationError({
									message: `${param} must be a non-negative number`,
									status: 400,
								}),
							)
						}
						boosts[param] = value
					}
				}
				const hasBoosts = Object.keys(boosts).length > 0

				// Execute search - only include optional params when defined
				const searchQuery = {
					query: trimmedQuery,
					scope,
					limit,
					offset,
					...(spaceId && {space_id: spaceId}),
					...(typeIds && {type_ids: typeIds}),
					...(excludeTypeIds && excludeTypeIds.length > 0 && {exclude_type_ids: excludeTypeIds}),
					...(includeDeleted && {include_deleted: true}),
					...(!includeNonCanonical && {include_non_canonical: false}),
					...(hasBoosts && {boosts}),
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
