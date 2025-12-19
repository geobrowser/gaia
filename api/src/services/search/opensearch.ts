/**
 * OpenSearch client implementation.
 *
 * This module provides the concrete implementation of the SearchClient
 * interface using OpenSearch as the backend.
 * Note: This implementation is read-only - it only queries the search index.
 * All indexing/updating/deleting is done by the Rust search-indexer service.
 */

import {Client} from "@opensearch-project/opensearch"
import type {SearchClient} from "./client"
import {SearchError, type SearchQuery, type SearchResponse, type SearchResult, type SearchScope} from "./types"

/**
 * UUID regex pattern for detecting ID-based queries.
 */
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/**
 * Score boost value for rank_feature queries.
 * Applied to entity_global_score, space_score, and entity_space_score fields.
 */
export const SCORE_BOOST = 1.3

/**
 * Boost value for name field in match_phrase_prefix queries.
 * Strongly boosts documents where the name starts with the query text.
 */
export const NAME_PREFIX_BOOST = 2.0

/**
 * Boost value for description field in match_phrase_prefix queries.
 * Moderately boosts documents where the description starts with the query text.
 */
export const DESCRIPTION_PREFIX_BOOST = 1.5

/**
 * Boost value for name field in multi_match field-level boosts.
 * Applied to name fields in bool_prefix queries to give higher weight to name matches.
 *
 * Rationale: Name field matches are generally more important than description matches
 * for user intent. 1.5x boost on name fields (applied to n-grams and exact matches)
 * ensures name-centric queries rank name matches higher while allowing description
 * matches to still contribute to overall relevance.
 */
export const NAME_FIELD_BOOST = 1.5

/**
 * Boost value for fuzzy text match queries.
 * Reduces the weight of fuzzy matches compared to exact/prefix matches.
 */
export const FUZZY_REDUCTION_BOOST = 0.6

/**
 * Default number of results to return when no limit is specified.
 */
export const DEFAULT_PAGE_SIZE = 20

/**
 * Maximum number of results that can be requested in a single query.
 */
export const MAX_PAGE_SIZE = 100

/**
 * OpenSearch client implementation.
 *
 * @example
 * ```typescript
 * const client = new OpenSearchClient("http://localhost:9200");
 * await client.healthCheck();
 *
 * const results = await client.search({
 *   query: "blockchain",
 *   scope: "GLOBAL",
 * });
 * ```
 */
export class OpenSearchClient implements SearchClient {
	private client: Client
	private indexName: string

	/**
	 * Create a new OpenSearch client.
	 *
	 * @param nodeUrl - The OpenSearch server URL
	 * @param indexName - The index name to use (default: "entities")
	 */
	constructor(nodeUrl: string, indexName: string = "entities") {
		this.client = new Client({node: nodeUrl})
		this.indexName = indexName
	}

	/**
	 * Execute a search query against the index.
	 */
	async search(query: SearchQuery): Promise<SearchResponse> {
		const searchBody = this.buildSearchBody(query)

		const response = await this.client.search({
			index: this.indexName,
			body: searchBody,
			from: query.offset ?? 0,
			size: Math.min(query.limit ?? DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE),
		})

		const body = response.body
		const hits = body.hits.hits as Array<{
			_source: Record<string, unknown>
			_score: number
		}>

		const results: SearchResult[] = hits.map((hit) => ({
			entityId: hit._source.entity_id as string,
			spaceId: hit._source.space_id as string,
			name: hit._source.name as string | undefined,
			description: hit._source.description as string | undefined,
			avatar: hit._source.avatar as string | undefined,
			cover: hit._source.cover as string | undefined,
			entityGlobalScore: hit._source.entity_global_score as number | undefined,
			spaceScore: hit._source.space_score as number | undefined,
			entitySpaceScore: hit._source.entity_space_score as number | undefined,
		}))

		return {
			results,
			total: typeof body.hits.total === "number" ? body.hits.total : (body.hits.total?.value ?? 0),
			tookMs: body.took,
		}
	}

	/**
	 * Check if the search engine is healthy.
	 */
	async healthCheck(): Promise<boolean> {
		try {
			const health = await this.client.cluster.health({})
			return health.statusCode === 200
		} catch {
			return false
		}
	}

	/**
	 * Build the OpenSearch query body based on search parameters.
	 *
	 * Boost Strategy Overview:
	 * - rank_feature boosts use logarithmic saturation to prevent score inflation
	 * - Field-level boosts prioritize name matches over description matches
	 * - Prefix matching strongly indicates user intent (especially for names)
	 * - Fuzzy matching is reduced to prevent false positives from typos
	 * - Score hierarchy: exact name > name prefix > description prefix > fuzzy > scored fields
	 */
	buildSearchBody(query: SearchQuery): object {
		// Check if the query is a UUID for direct ID lookup
		if (UUID_PATTERN.test(query.query)) {
			return this.buildUuidQuery(query.query, query.scope, query.space_id)
		}

		// Build base text search query
		const baseTextQuery = this.buildBaseTextQuery(query.query)

		// Apply scope-specific query building
		switch (query.scope) {
			case "GLOBAL":
				return this.buildGlobalQuery(baseTextQuery)

			case "GLOBAL_BY_SPACE_SCORE":
				return this.buildGlobalBySpaceScoreQuery(baseTextQuery)

			case "SPACE_SINGLE": {
				if (!query.space_id) {
					throw SearchError.validationError("SPACE_SINGLE scope requires space_id")
				}
				return this.buildSingleSpaceQuery(baseTextQuery, query.space_id)
			}

			case "SPACE": {
				if (!query.space_id) {
					throw SearchError.validationError("SPACE scope requires space_id")
				}
				// SPACE scope: Search within a space and its subspaces
				// Currently implemented as single space query - future enhancement
				// will expand to include hierarchical space relationships
				return this.buildSingleSpaceQuery(baseTextQuery, query.space_id)
			}

			default:
				return this.buildGlobalQuery(baseTextQuery)
		}
	}

	/**
	 * Build a query for UUID-based lookups.
	 * Performs a direct lookup on entity_id field with scope filtering applied.
	 *
	 * Uses `term` query which is the correct query type for exact matches
	 * on keyword fields. The entity_id field is indexed as a keyword type
	 * in the OpenSearch index mapping.
	 */
	buildUuidQuery(uuid: string, scope: SearchScope, space_id?: string): object {
		// term query is correct for keyword fields - performs exact match lookup
		const baseUuidQuery = {
			term: {entity_id: uuid},
		}

		const baseUuidQueryObject = {
			query: baseUuidQuery,
		}

		// Apply scope-specific filtering
		switch (scope) {
			case "GLOBAL":
			case "GLOBAL_BY_SPACE_SCORE":
				return baseUuidQueryObject

			case "SPACE_SINGLE":
				if (space_id) {
					return {
						query: {
							bool: {
								must: [baseUuidQuery],
								filter: [{term: {space_id}}],
							},
						},
					}
				}
				return baseUuidQueryObject

			case "SPACE":
				if (space_id) {
					// For SPACE, we would need to expand to include subspaces
					// For now, treat it as a single space query
					return {
						query: {
							bool: {
								must: [baseUuidQuery],
								filter: [{term: {space_id}}],
							},
						},
					}
				}
				return baseUuidQueryObject

			default:
				return baseUuidQueryObject
		}
	}

	/**
	 * Build the base text search query used across all scopes.
	 *
	 * Uses:
	 * - multi_match with bool_prefix for autocomplete on search_as_you_type fields
	 * - Fuzzy multi_match for typo tolerance
	 * - match_phrase_prefix for strong prefix matching on name and description
	 */
	buildBaseTextQuery(queryText: string): object {
		return {
			bool: {
				should: [
					{
						// Autocomplete-style match over n-grams with higher weight on name
						multi_match: {
							query: queryText,
							type: "bool_prefix",
							fields: [
								`name^${NAME_FIELD_BOOST}`,
								`name._2gram^${NAME_FIELD_BOOST}`,
								`name._3gram^${NAME_FIELD_BOOST}`,
								"description",
								"description._2gram",
								"description._3gram",
							],
						},
					},
					{
						// Fuzzy text match to tolerate minor typos
						// AUTO fuzziness: 1-2 chars: 0 edits, 3-4 chars: 1 edit, 5+ chars: 2 edits
						multi_match: {
							query: queryText,
							fields: ["name", "description"],
							fuzziness: "AUTO",
							boost: FUZZY_REDUCTION_BOOST,
						},
					},
					{
						// Strongly boost documents where the name starts with the query text
						match_phrase_prefix: {
							name: {
								query: queryText,
								boost: NAME_PREFIX_BOOST,
							},
						},
					},
					{
						// Moderately boost documents where the description starts with the query text
						match_phrase_prefix: {
							description: {
								query: queryText,
								boost: DESCRIPTION_PREFIX_BOOST,
							},
						},
					},
				],
				minimum_should_match: 1,
			},
		}
	}

	/**
	 * Build a global search query.
	 * Boosts results by entity_global_score using rank_feature.
	 */
	buildGlobalQuery(baseTextQuery: object): object {
		return {
			query: {
				bool: {
					must: [baseTextQuery],
					should: [
						{
							rank_feature: {
								field: "entity_global_score",
								boost: SCORE_BOOST,
							},
						},
					],
				},
			},
		}
	}

	/**
	 * Build a global search query ranked by space score.
	 * Boosts results by space_score using rank_feature.
	 */
	buildGlobalBySpaceScoreQuery(baseTextQuery: object): object {
		return {
			query: {
				bool: {
					must: [baseTextQuery],
					should: [
						{
							rank_feature: {
								field: "space_score",
								boost: SCORE_BOOST,
							},
						},
					],
				},
			},
		}
	}

	/**
	 * Build a single-space filtered query.
	 * Filters by a single space_id and boosts by entity_space_score.
	 */
	buildSingleSpaceQuery(baseTextQuery: object, spaceId: string): object {
		return {
			query: {
				bool: {
					must: [baseTextQuery],
					filter: [{term: {space_id: spaceId}}],
					should: [
						{
							rank_feature: {
								field: "entity_space_score",
								boost: SCORE_BOOST,
							},
						},
					],
				},
			},
		}
	}
}
