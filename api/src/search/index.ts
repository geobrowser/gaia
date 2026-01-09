/**
 * Search route handler.
 *
 * Provides HTTP endpoints for full-text search across the Knowledge Graph.
 */

import {Hono} from "hono"

import type {SearchClient, SearchScope} from "../services/search"
import {isValidUuid} from "../utils/uuid"

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
 * Minimum length for search queries.
 */
const MIN_QUERY_LENGTH = 2

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
const VALID_PARAMS: Set<string> = new Set(["query", "q", "scope", "space_id", "type_ids", "limit", "offset"])

/**
 * Create the search router with dependency-injected search client.
 *
 * @param searchClient - The search client to use for queries
 * @returns Configured Hono router
 *
 * @example
 * ```typescript
 * import { createSearchRouter } from "./src/search";
 * import { OpenSearchClient } from "./src/services/search";
 *
 * const searchClient = new OpenSearchClient("http://localhost:9200");
 * app.route("/search", createSearchRouter(searchClient));
 * ```
 */
export function createSearchRouter(searchClient: SearchClient) {
	const router = new Hono()

	/**
	 * GET /search
	 *
	 * Search for entities across the Knowledge Graph.
	 *
	 * Query Parameters:
	 * - query or q: Search query string (required, 2-500 characters)
	 * - scope: Search scope (optional, default: GLOBAL)
	 *   - GLOBAL: Search across all spaces, boosted by entity_global_score
	 *   - GLOBAL_BY_SPACE_SCORE: Search across all spaces, boosted by space_score
	 *   - SPACE_SINGLE: Search within a single specific space, boosted by entity_space_score
	 *   - SPACE: Search within a space and its subspaces (currently implemented as single space)
	 * - space_id: Space ID for space-scoped searches (required for SPACE_* scopes, max 36 chars)
	 * - type_ids: Comma-separated list of type IDs to filter by (optional, max 10 IDs)
	 * - limit: Maximum results (optional, default: 20, max: 100)
	 * - offset: Pagination offset (optional, default: 0, max: 1000)
	 *
	 * Response:
	 * - 200: SearchResponse with results
	 *   - results[]: Array of search results
	 *     - entityId: The entity's unique identifier
	 *     - spaceId: The space this entity belongs to
	 *     - name?: Optional entity display name
	 *     - description?: Optional description text
	 *     - avatar?: Optional avatar image URL
	 *     - cover?: Optional cover image URL
	 *     - typeIds?: Array of type IDs associated with this entity (from type_relations)
	 *     - entityGlobalScore?: Global entity score
	 *     - spaceScore?: Space score
	 *     - entitySpaceScore?: Entity-space score
	 *   - total: Total number of matching documents
	 *   - tookMs: Time taken to execute the search in milliseconds
	 * - 400: Invalid request parameters
	 * - 500: Search failed
	 */
	router.get("/", async (c) => {
		// Check for unrecognized query parameters
		const allParams = Object.keys(c.req.query())
		const unrecognizedParams = allParams.filter((param) => !VALID_PARAMS.has(param))
		if (unrecognizedParams.length > 0) {
			return c.json(
				{
					error: "Unrecognized parameter",
					message: `Unrecognized query parameter(s): ${unrecognizedParams.join(", ")}. Valid parameters are: ${Array.from(VALID_PARAMS).join(", ")}`,
				},
				400,
			)
		}

		// Extract query parameters (accepts "query" first or "q" second)
		const query = c.req.query("query") ?? c.req.query("q")
		const scopeParam = c.req.query("scope") ?? "GLOBAL"
		const spaceId = c.req.query("space_id")
		const typeIdsParam = c.req.query("type_ids")
		const limitParam = c.req.query("limit")
		const offsetParam = c.req.query("offset")

		// Validate query
		if (!query || query.trim().length === 0) {
			return c.json(
				{
					error: "Missing required parameter",
					message: "Query parameter 'query' or 'q' is required",
				},
				400,
			)
		}

		// Validate query length
		const trimmedQuery = query.trim()
		if (trimmedQuery.length < MIN_QUERY_LENGTH) {
			return c.json(
				{
					error: "Invalid parameter",
					message: `Query must be at least ${MIN_QUERY_LENGTH} characters`,
				},
				400,
			)
		}

		if (trimmedQuery.length > MAX_QUERY_LENGTH) {
			return c.json(
				{
					error: "Invalid parameter",
					message: `Query must not exceed ${MAX_QUERY_LENGTH} characters`,
				},
				400,
			)
		}

		// Validate scope
		if (!VALID_SCOPES.has(scopeParam as SearchScope)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: `Invalid scope '${scopeParam}'. Valid values: ${Array.from(VALID_SCOPES).join(", ")}`,
				},
				400,
			)
		}
		const scope = scopeParam as SearchScope

		// Validate space_id for space-scoped searches
		if (scope === "SPACE_SINGLE" || scope === "SPACE") {
			if (!spaceId) {
				return c.json(
					{
						error: "Missing required parameter",
						message: `space_id is required for ${scope} scope`,
					},
					400,
				)
			}

			if (spaceId.length > MAX_SPACE_ID_LENGTH) {
				return c.json(
					{
						error: "Invalid parameter",
						message: `space_id must not exceed ${MAX_SPACE_ID_LENGTH} characters`,
					},
					400,
				)
			}

			if (!isValidUuid(spaceId)) {
				return c.json(
					{
						error: "Invalid parameter",
						message: "space_id must be a valid UUID",
					},
					400,
				)
			}
		}

		// Parse and validate limit
		let limit = DEFAULT_LIMIT
		if (limitParam) {
			const parsedLimit = parseInt(limitParam, 10)
			if (Number.isNaN(parsedLimit) || parsedLimit < 1) {
				return c.json(
					{
						error: "Invalid parameter",
						message: "limit must be a positive integer",
					},
					400,
				)
			}
			limit = Math.min(parsedLimit, MAX_LIMIT)
		}

		// Parse and validate offset
		let offset = 0
		if (offsetParam) {
			const parsedOffset = parseInt(offsetParam, 10)
			if (Number.isNaN(parsedOffset) || parsedOffset < 0) {
				return c.json(
					{
						error: "Invalid parameter",
						message: "offset must be a non-negative integer",
					},
					400,
				)
			}
			if (parsedOffset > MAX_OFFSET) {
				return c.json(
					{
						error: "Invalid parameter",
						message: `offset must not exceed ${MAX_OFFSET}`,
					},
					400,
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
				return c.json(
					{
						error: "Invalid parameter",
						message: `type_ids must not contain more than ${MAX_TYPE_IDS} IDs`,
					},
					400,
				)
			}

			// Validate each type ID is a valid UUID
			for (const typeId of typeIds) {
				if (!isValidUuid(typeId)) {
					return c.json(
						{
							error: "Invalid parameter",
							message: `type_ids must contain valid UUIDs, got invalid ID: ${typeId}`,
						},
						400,
					)
				}
			}
		}

		// Execute search
		try {
			const response = await searchClient.search({
				query: trimmedQuery,
				scope,
				space_id: spaceId,
				type_ids: typeIds,
				limit,
				offset,
			})

			return c.json(response)
		} catch (error) {
			console.error("[SEARCH] Search error:", error)
			return c.json(
				{
					error: "Search failed",
					message: error instanceof Error ? error.message : "An unexpected error occurred",
				},
				500,
			)
		}
	})

	/**
	 * GET /search/health
	 *
	 * Check the health of the search service.
	 *
	 * Response:
	 * - 200: { status: "healthy" }
	 * - 503: { status: "unhealthy" }
	 */
	router.get("/health", async (c) => {
		try {
			const healthy = await searchClient.healthCheck()
			if (healthy) {
				return c.json({status: "healthy"})
			}
			return c.json({status: "unhealthy"}, 503)
		} catch (error) {
			console.error("[SEARCH] Health check error:", error)
			return c.json({status: "unhealthy"}, 503)
		}
	})

	return router
}
