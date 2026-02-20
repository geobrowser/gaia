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
import {
	SearchError,
	type SearchQuery,
	type SearchResponse,
	type SearchResult,
	type SearchResultSpace,
	type SearchResultType,
	type SearchScope,
} from "./types"
import {normalizeUuid, toDashedUuid} from "../../utils/uuid"

/**
 * UUID regex patterns for detecting ID-based queries.
 * Supports both dashed (36 chars) and dashless (32 chars) formats.
 */
const UUID_DASHED_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const UUID_DASHLESS_PATTERN = /^[0-9a-f]{32}$/i

/**
 * Return both dashed and dashless forms of a UUID for OpenSearch term queries.
 * The index may contain either format during migration, so we match both.
 */
function uuidTermVariants(uuid: string): [string, string] {
	const dashless = normalizeUuid(uuid) as string
	const dashed = toDashedUuid(uuid)
	return [dashed, dashless]
}

/**
 * Default average score for entities without a specific score.
 * When an entity has no score value (missing or empty), this default is used.
 * Scores are normalized to [0, 1] with 0.5 being average.
 */
export const DEFAULT_AVERAGE_SCORE = 0.5

/**
 * Minimum score threshold for search boosting.
 * Any score below this threshold will be clamped to this value.
 * Scores are normalized to [0, 1] with 0.5 being average.
 * A threshold of 0 prevents negative results from script_score (OpenSearch requirement).
 */
export const MIN_SCORE_THRESHOLD = 0.0

/**
 * Score shift value to ensure all scores are positive.
 * With scores in [0, 1], a shift of 1 ensures the minimum boost is
 * always positive: (0 + 1) * SCORE_BOOST > 0.
 */
export const SCORE_SHIFT = 1.0

/**
 * Score boost multiplier for score fields.
 * Applied to entity_global_score, space_score, and entity_space_score fields.
 * With scores in [0, 1], this multiplier controls how much global/space scores
 * influence ranking relative to text match quality.
 *
 * Formula: (max(score, 0) + 1) * 10
 *   score=0.0 → boost=10, score=0.5 → boost=15, score=1.0 → boost=20
 *
 * This produces a boost range of 10 across the full score spectrum, which is
 * large enough for high-score entities to outrank low-score entities that have
 * moderately better text matches (e.g., exact single-word name match vs multi-word name).
 */
export const SCORE_BOOST = 10.0

/**
 * Boost value for name field in match_phrase_prefix queries.
 * Strongly boosts documents where the query matches as a phrase prefix in the name.
 *
 * Set to 5.0 (vs DESCRIPTION_PREFIX_BOOST=1.5) to ensure name matches consistently
 * outscore description-only matches. A lower ratio (e.g. 2.0/1.5=1.33x) is insufficient
 * because BM25 field length normalization can amplify description match scores when
 * descriptions are short relative to the index-wide average description length.
 */
export const NAME_PREFIX_BOOST = 5.0

/**
 * Boost value for description field in match_phrase_prefix queries.
 * Moderately boosts documents where the query matches as a phrase prefix in the description.
 * Matches any position in the description, not just the start of the string — e.g. query
 * "Quant" matches "Applied Quantum Physics" because a word starts with the prefix.
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
 * Boost value for exact token match on the name field.
 * Uses a standard `match` query (not prefix/phrase_prefix) to boost documents
 * where the query terms exactly match analyzed tokens in the name.
 *
 * This ensures that an entity named "Geo" ranks above "geojson_preview_tool"
 * for query "geo", because `match` requires exact token equality — "geo" matches
 * the token "geo" but does NOT match "geojson". Without this, accumulated prefix
 * matches in name and description (e.g. "geojson.io", "GeoJSON") can outscore
 * a short exact name match.
 *
 * Set to 5.0 to create a meaningful signal for exact token matches while
 * keeping the boost modest enough that score field differences (SCORE_BOOST=10)
 * can still override text match gaps between single-word and multi-word names.
 */
export const NAME_EXACT_TOKEN_BOOST = 5.0

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
 * GRC-20 relation type IDs used to identify avatar and cover relations.
 */
const TYPE_RELATION_TYPE_ID = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
const AVATAR_RELATION_TYPE_ID = "1155beff-fad5-49b7-a2e0-da4777b8792c"
const COVER_RELATION_TYPE_ID = "34f53507-2e6b-42c5-a844-43981a77cfa2"

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
		const searchBody = this.buildSearchBody(query) as Record<string, unknown>

		// When script_fields is present, OpenSearch suppresses _source by default
		if (searchBody.script_fields) {
			searchBody._source = true
		}

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
			fields?: Record<string, number[]>
		}>

		// Collect unique entity IDs for batch resolution
		const allTypeEntityIds = new Set<string>()
		const allSpaceTopicEntityIds = new Set<string>()
		const allImageEntityIds = new Set<string>()

		// First pass: extract IDs from hits
		const hitData = hits.map((hit) => {
			const relations = hit._source.relations as Array<{relation_type: string; to_entity_id: string}> | undefined
			const typeIds = relations
				?.filter((rel) => normalizeUuid(rel.relation_type) === TYPE_RELATION_TYPE_ID)
				.map((rel) => normalizeUuid(rel.to_entity_id) as string)
			typeIds?.forEach((id) => allTypeEntityIds.add(id))

			// Extract avatar/cover image entity IDs from relations
			const avatarImageEntityId = relations
				?.find((rel) => normalizeUuid(rel.relation_type) === AVATAR_RELATION_TYPE_ID)
				?.to_entity_id
			const coverImageEntityId = relations
				?.find((rel) => normalizeUuid(rel.relation_type) === COVER_RELATION_TYPE_ID)
				?.to_entity_id
			const normalizedAvatarId = avatarImageEntityId ? (normalizeUuid(avatarImageEntityId) as string) : undefined
			const normalizedCoverId = coverImageEntityId ? (normalizeUuid(coverImageEntityId) as string) : undefined
			if (normalizedAvatarId) allImageEntityIds.add(normalizedAvatarId)
			if (normalizedCoverId) allImageEntityIds.add(normalizedCoverId)

			const spaceTopicEntityId = hit._source.space_topic_entity_id as string | undefined
			if (spaceTopicEntityId) {
				allSpaceTopicEntityIds.add(normalizeUuid(spaceTopicEntityId) as string)
			}

			return {hit, typeIds, spaceTopicEntityId, avatarImageEntityId: normalizedAvatarId, coverImageEntityId: normalizedCoverId}
		})

		// Batch-resolve type names, space metadata, and image URLs in parallel
		const [typeNameMap, spaceMetadataMap, imageUrlMap] = await Promise.all([
			this.resolveTypeNames([...allTypeEntityIds]),
			this.resolveSpaceMetadata([...allSpaceTopicEntityIds]),
			this.resolveImageUrls([...allImageEntityIds]),
		])

		// Second pass: build results with enriched data
		const results: SearchResult[] = hitData.map(({hit, typeIds, spaceTopicEntityId, avatarImageEntityId, coverImageEntityId}) => {
			// Compute relevanceScore and textMatchScore
			const relevanceScore = hit._score
			const scoreBoost = hit.fields?.score_boost?.[0]
			const textMatchScore = scoreBoost !== undefined ? Math.max(0, relevanceScore - scoreBoost) : relevanceScore

			// Build enriched types array
			const types: SearchResultType[] | undefined = typeIds?.length
				? typeIds.map((id) => ({id, name: typeNameMap.get(id)}))
				: undefined

			// Build enriched space object
			const spaceId = normalizeUuid(hit._source.space_id as string) as string
			const normalizedTopicId = spaceTopicEntityId
				? (normalizeUuid(spaceTopicEntityId) as string)
				: undefined
			const spaceMeta = normalizedTopicId ? spaceMetadataMap.get(normalizedTopicId) : undefined
			const space: SearchResultSpace = {
				id: spaceId,
				...(spaceMeta && {
					name: spaceMeta.name,
					description: spaceMeta.description,
					avatar: spaceMeta.avatar,
					cover: spaceMeta.cover,
				}),
			}

			// Resolve avatar/cover from image entity URLs
			const avatar = avatarImageEntityId ? imageUrlMap.get(avatarImageEntityId) : undefined
			const cover = coverImageEntityId ? imageUrlMap.get(coverImageEntityId) : undefined

			return {
				entityId: normalizeUuid(hit._source.entity_id as string) as string,
				space,
				name: hit._source.name as string | undefined,
				description: hit._source.description as string | undefined,
				avatar,
				cover,
				types,
				entityGlobalScore: hit._source.entity_global_score as number | undefined,
				spaceScore: hit._source.space_score as number | undefined,
				entitySpaceScore: hit._source.entity_space_score as number | undefined,
				relevanceScore,
				textMatchScore,
			}
		})

		return {
			results,
			total: typeof body.hits.total === "number" ? body.hits.total : (body.hits.total?.value ?? 0),
			tookMs: body.took,
		}
	}

	/**
	 * Batch-fetch type entity names from the index.
	 * Returns a map of typeEntityId → name.
	 */
	private async resolveTypeNames(typeEntityIds: string[]): Promise<Map<string, string>> {
		if (typeEntityIds.length === 0) return new Map()

		// Include both dashed and dashless variants since the index may store either format
		const termVariants = typeEntityIds.flatMap((id) => uuidTermVariants(id))

		const response = await this.client.search({
			index: this.indexName,
			body: {
				query: {terms: {entity_id: termVariants}},
				_source: ["entity_id", "name"],
				size: typeEntityIds.length,
			},
		})

		const nameMap = new Map<string, string>()
		for (const hit of response.body.hits.hits) {
			const source = hit._source as Record<string, unknown>
			const entityId = normalizeUuid(source.entity_id as string) as string
			if (source.name && !nameMap.has(entityId)) {
				nameMap.set(entityId, source.name as string)
			}
		}
		return nameMap
	}

	/**
	 * Batch-fetch space metadata from topic entities in the index.
	 * Returns a map of topicEntityId → { name, description, avatar, cover }.
	 */
	private async resolveSpaceMetadata(
		topicEntityIds: string[],
	): Promise<Map<string, {name?: string; description?: string; avatar?: string; cover?: string}>> {
		if (topicEntityIds.length === 0) return new Map()

		// Include both dashed and dashless variants since the index may store either format
		const termVariants = topicEntityIds.flatMap((id) => uuidTermVariants(id))

		const response = await this.client.search({
			index: this.indexName,
			body: {
				query: {terms: {entity_id: termVariants}},
				_source: ["entity_id", "name", "description", "avatar", "cover"],
				size: topicEntityIds.length,
			},
		})

		const metadataMap = new Map<
			string,
			{name?: string; description?: string; avatar?: string; cover?: string}
		>()
		for (const hit of response.body.hits.hits) {
			const source = hit._source as Record<string, unknown>
			const entityId = normalizeUuid(source.entity_id as string) as string
			if (!metadataMap.has(entityId)) {
				metadataMap.set(entityId, {
					name: source.name as string | undefined,
					description: source.description as string | undefined,
					avatar: source.avatar as string | undefined,
					cover: source.cover as string | undefined,
				})
			}
		}
		return metadataMap
	}

	/**
	 * Batch-fetch image URLs from image entities in the index.
	 * Avatar/cover relations point to image entities; this resolves their image_url field.
	 * Returns a map of imageEntityId → imageUrl.
	 */
	private async resolveImageUrls(imageEntityIds: string[]): Promise<Map<string, string>> {
		if (imageEntityIds.length === 0) return new Map()

		// Include both dashed and dashless variants since the index may store either format
		const termVariants = imageEntityIds.flatMap((id) => uuidTermVariants(id))

		const response = await this.client.search({
			index: this.indexName,
			body: {
				query: {terms: {entity_id: termVariants}},
				_source: ["entity_id", "image_url"],
				size: imageEntityIds.length,
			},
		})

		const urlMap = new Map<string, string>()
		for (const hit of response.body.hits.hits) {
			const source = hit._source as Record<string, unknown>
			const entityId = normalizeUuid(source.entity_id as string) as string
			const imageUrl = source.image_url as string | undefined
			if (imageUrl && !urlMap.has(entityId)) {
				urlMap.set(entityId, imageUrl)
			}
		}
		return urlMap
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
	 * - Score field boosts use field_value_factor to incorporate entity/space scores
	 * - Field-level boosts prioritize name matches over description matches
	 * - Prefix matching strongly indicates user intent (especially for names)
	 * - Fuzzy matching is reduced to prevent false positives from typos
	 * - Score hierarchy: exact name > name prefix > description prefix > fuzzy > scored fields
	 * - Empty queries return top ranked results based on scope-specific score fields
	 */
	buildSearchBody(query: SearchQuery): object {
		const includeDeleted = query.include_deleted ?? false

		// Check if the query is empty or whitespace-only
		const trimmedQuery = query.query.trim()
		if (trimmedQuery.length === 0) {
			// For empty queries, return top ranked results based on scope
			return this.buildTopRankedQuery(query.scope, query.space_id, query.type_ids, includeDeleted)
		}

		// Check if the query is a UUID for direct ID lookup (dashed or dashless)
		if (UUID_DASHED_PATTERN.test(trimmedQuery) || UUID_DASHLESS_PATTERN.test(trimmedQuery)) {
			return this.buildUuidQuery(trimmedQuery, query.scope, query.space_id, query.type_ids, includeDeleted)
		}

		// Build base text search query
		const baseTextQuery = this.buildBaseTextQuery(trimmedQuery)

		// Apply scope-specific query building
		switch (query.scope) {
			case "GLOBAL":
				return this.buildGlobalQuery(baseTextQuery, query.type_ids, includeDeleted)

			case "GLOBAL_BY_SPACE_SCORE":
				return this.buildGlobalBySpaceScoreQuery(baseTextQuery, query.type_ids, includeDeleted)

			case "GLOBAL_BY_ENTITY_SPACE_SCORE":
				return this.buildGlobalByEntitySpaceScoreQuery(baseTextQuery, query.type_ids, includeDeleted)

			case "SPACE_SINGLE": {
				if (!query.space_id) {
					throw SearchError.validationError("SPACE_SINGLE scope requires space_id")
				}
				return this.buildSingleSpaceQuery(baseTextQuery, query.space_id, query.type_ids, includeDeleted)
			}

			case "SPACE": {
				if (!query.space_id) {
					throw SearchError.validationError("SPACE scope requires space_id")
				}
				// SPACE scope: Search within a space and its subspaces
				// Currently implemented as single space query - future enhancement
				// will expand to include hierarchical space relationships
				return this.buildSingleSpaceQuery(baseTextQuery, query.space_id, query.type_ids, includeDeleted)
			}

			default:
				return this.buildGlobalQuery(baseTextQuery, query.type_ids, includeDeleted)
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
	buildUuidQuery(
		uuid: string,
		scope: SearchScope,
		space_id?: string,
		typeIds?: string[],
		includeDeleted: boolean = false,
	): object {
		// Match both dashed and dashless forms (index may contain either during migration)
		const baseUuidQuery = {
			terms: {entity_id: uuidTermVariants(uuid)},
		}

		const typeFilter = this.buildTypeFilter(typeIds)
		const filters: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (typeFilter) filters.push(typeFilter)

		// Apply scope-specific filtering
		switch (scope) {
			case "GLOBAL":
			case "GLOBAL_BY_SPACE_SCORE":
			case "GLOBAL_BY_ENTITY_SPACE_SCORE":
				return {
					query: {
						bool: {
							must: [baseUuidQuery],
							filter: filters,
						},
					},
				}

			case "SPACE_SINGLE":
			case "SPACE":
				if (space_id) {
					filters.push({terms: {space_id: uuidTermVariants(space_id)}})
				}
				return {
					query: {
						bool: {
							must: [baseUuidQuery],
							filter: filters,
						},
					},
				}

			default:
				return {
					query: {
						bool: {
							must: [baseUuidQuery],
							filter: filters,
						},
					},
				}
		}
	}

	/**
	 * Build a query for returning top ranked results without text matching.
	 * Used when no search query is provided - returns results ranked by scope-specific score fields.
	 */
	buildTopRankedQuery(
		scope: SearchScope,
		space_id?: string,
		typeIds?: string[],
		includeDeleted: boolean = false,
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const filters: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (typeFilter) filters.push(typeFilter)

		// Apply scope-specific filtering and sorting
		switch (scope) {
			case "GLOBAL":
				return {
					query: {
						function_score: {
							query: {
								bool: {
									must: [{match_all: {}}],
									filter: filters,
								},
							},
							functions: [this.buildScoreBoostFunction("entity_global_score")],
							boost_mode: "replace",
							score_mode: "sum",
						},
					},
					script_fields: this.buildScoreBoostScriptFields("entity_global_score"),
				}

			case "GLOBAL_BY_SPACE_SCORE":
				return {
					query: {
						function_score: {
							query: {
								bool: {
									must: [{match_all: {}}],
									filter: filters,
								},
							},
							functions: [this.buildScoreBoostFunction("space_score")],
							boost_mode: "replace",
							score_mode: "sum",
						},
					},
					script_fields: this.buildScoreBoostScriptFields("space_score"),
				}

			case "GLOBAL_BY_ENTITY_SPACE_SCORE":
				return {
					query: {
						function_score: {
							query: {
								bool: {
									must: [{match_all: {}}],
									filter: filters,
								},
							},
							functions: [this.buildGlobalByEntitySpaceBoost()],
							boost_mode: "replace",
							score_mode: "sum",
						},
					},
				}

			case "SPACE_SINGLE":
			case "SPACE":
				if (space_id) {
					filters.push({terms: {space_id: uuidTermVariants(space_id)}})
				}
				return {
					query: {
						function_score: {
							query: {
								bool: {
									must: [{match_all: {}}],
									filter: filters,
								},
							},
							functions: [this.buildScoreBoostFunction("entity_space_score")],
							boost_mode: "replace",
							score_mode: "sum",
						},
					},
					script_fields: this.buildScoreBoostScriptFields("entity_space_score"),
				}

			default:
				return {
					query: {
						function_score: {
							query: {
								bool: {
									must: [{match_all: {}}],
									filter: filters,
								},
							},
							functions: [this.buildScoreBoostFunction("entity_global_score")],
							boost_mode: "replace",
							score_mode: "sum",
						},
					},
					script_fields: this.buildScoreBoostScriptFields("entity_global_score"),
				}
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
						// Exact token match on name — strongly boosts documents where
						// query terms match full analyzed tokens in the name field.
						// e.g. query "geo" matches name "Geo" (token "geo") but NOT
						// "geojson_preview_tool" (token "geojson" ≠ "geo").
						match: {
							name: {
								query: queryText,
								boost: NAME_EXACT_TOKEN_BOOST,
							},
						},
					},
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
	 * Build the Painless script for computing a score boost value.
	 * Reused by buildScoreBoostFunction (for function_score) and
	 * buildScoreBoostScriptFields (for returning the boost value in results).
	 *
	 * Formula: (max(score, 0.0) + 1.0) * 10.0
	 */
	buildScoreBoostScript(scoreField: string): string {
		return `
			def scoreValue = doc.containsKey('${scoreField}') && !doc['${scoreField}'].empty
				? doc['${scoreField}'].value
				: ${DEFAULT_AVERAGE_SCORE};
			def clampedScore = Math.max(scoreValue, ${MIN_SCORE_THRESHOLD});
			return (clampedScore + ${SCORE_SHIFT}) * ${SCORE_BOOST};
		`
	}

	/**
	 * Build a score boost function for use in function_score queries.
	 */
	buildScoreBoostFunction(scoreField: string): object {
		return {
			script_score: {
				script: {
					source: this.buildScoreBoostScript(scoreField),
				},
			},
		}
	}

	/**
	 * Build script_fields to return the computed score boost value alongside each hit.
	 * Used to derive textMatchScore = relevanceScore - scoreBoost.
	 */
	buildScoreBoostScriptFields(scoreField: string): object {
		return {
			score_boost: {
				script: {
					source: this.buildScoreBoostScript(scoreField),
				},
			},
		}
	}

	/**
	 * Build a score boost function that multiplies entity_space_score by space_score.
	 * Uses script_score to compute entity_space_score * space_score, then clamp and shift.
	 *
	 * Strategy:
	 * 1. Read both entity_space_score and space_score (default to 0 if missing)
	 * 2. Multiply them together
	 * 3. Clamp at MIN_SCORE_THRESHOLD (-10) to limit extreme outliers
	 * 4. Shift by SCORE_SHIFT (10) to ensure positive values
	 * 5. Apply SCORE_BOOST multiplier
	 */
	buildGlobalByEntitySpaceBoost(): object {
		return {
			script_score: {
				script: {
					source: `
						def entitySpaceScore = doc.containsKey('entity_space_score') && !doc['entity_space_score'].empty
							? doc['entity_space_score'].value
							: ${DEFAULT_AVERAGE_SCORE};
						def spaceScore = doc.containsKey('space_score') && !doc['space_score'].empty
							? doc['space_score'].value
							: ${DEFAULT_AVERAGE_SCORE};
						def product = entitySpaceScore * spaceScore;
						def clampedScore = Math.max(product, ${MIN_SCORE_THRESHOLD});
						return (clampedScore + ${SCORE_SHIFT}) * ${SCORE_BOOST};
					`,
				},
			},
		}
	}

	/**
	 * Build a global search query.
	 * Boosts results by entity_global_score using function_score.
	 */
	buildGlobalQuery(baseTextQuery: object, typeIds?: string[], includeDeleted: boolean = false): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const filters: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (typeFilter) filters.push(typeFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
						},
					},
					functions: [this.buildScoreBoostFunction("entity_global_score")],
					boost_mode: "sum",
					score_mode: "sum",
				},
			},
			script_fields: this.buildScoreBoostScriptFields("entity_global_score"),
		}
	}

	/**
	 * Build a global search query ranked by space score.
	 * Boosts results by space_score using function_score.
	 */
	buildGlobalBySpaceScoreQuery(baseTextQuery: object, typeIds?: string[], includeDeleted: boolean = false): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const filters: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (typeFilter) filters.push(typeFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
						},
					},
					functions: [this.buildScoreBoostFunction("space_score")],
					boost_mode: "sum",
					score_mode: "sum",
				},
			},
			script_fields: this.buildScoreBoostScriptFields("space_score"),
		}
	}

	/**
	 * Build a global search query ranked by entity space score * space score.
	 * Boosts results by entity_space_score * space_score using function_score.
	 */
	buildGlobalByEntitySpaceScoreQuery(
		baseTextQuery: object,
		typeIds?: string[],
		includeDeleted: boolean = false,
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const filters: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (typeFilter) filters.push(typeFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
						},
					},
					functions: [this.buildGlobalByEntitySpaceBoost()],
					boost_mode: "sum",
					score_mode: "sum",
				},
			},
		}
	}

	/**
	 * Build a single-space filtered query.
	 * Filters by a single space_id and boosts by entity_space_score.
	 */
	buildSingleSpaceQuery(
		baseTextQuery: object,
		spaceId: string,
		typeIds?: string[],
		includeDeleted: boolean = false,
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const filters: object[] = [{terms: {space_id: uuidTermVariants(spaceId)}}]
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (typeFilter) filters.push(typeFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
						},
					},
					functions: [this.buildScoreBoostFunction("entity_space_score")],
					boost_mode: "sum",
					score_mode: "sum",
				},
			},
			script_fields: this.buildScoreBoostScriptFields("entity_space_score"),
		}
	}

	/**
	 * Build a type filter for filtering by type relation IDs.
	 * Returns null if no typeIds are provided.
	 */
	buildTypeFilter(typeIds?: string[]): object | null {
		if (!typeIds || typeIds.length === 0) {
			return null
		}

		// Include both dashed and dashless forms (index may contain either during migration)
		const allVariants = typeIds.flatMap((id) => uuidTermVariants(id))

		return {
			nested: {
				path: "relations",
				query: {
					terms: {
						"relations.to_entity_id": allVariants,
					},
				},
			},
		}
	}

	/**
	 * Build a filter to select non-deleted entities.
	 * Matches documents where deleted is false or the deleted field doesn't exist.
	 */
	buildNonDeletedFilter(): object {
		return {
			bool: {
				should: [{term: {deleted: false}}, {bool: {must_not: [{exists: {field: "deleted"}}]}}],
				minimum_should_match: 1,
			},
		}
	}
}
