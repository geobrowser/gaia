/**
 * OpenSearch client implementation.
 *
 * This module provides the concrete implementation of the SearchClient
 * interface using OpenSearch as the backend.
 * Note: This implementation is read-only - it only queries the search index.
 * All indexing/updating/deleting is done by the Rust search-indexer service.
 */

import {ContentIds, SystemIds} from "@geoprotocol/geo-sdk"
import {Client} from "@opensearch-project/opensearch"
import {normalizeUuid, toDashedUuid} from "../../utils/uuid"
import type {SearchClient} from "./client"
import {
	type BoostOverrides,
	SearchError,
	type SearchQuery,
	type SearchResponse,
	type SearchResult,
	type SearchResultSpace,
	type SearchResultType,
	type SearchScope,
} from "./types"

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
 * Default score for entities without a specific score (global, space, or entity).
 * When an entity has no score value (missing or empty), this default is used.
 * Set to 0.08 so unscored entities rank below most scored entities without
 * being completely invisible in results.
 */
export const DEFAULT_AVERAGE_SCORE = 0.08

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
 * Formula: (max(score, 0) + 1) * 75
 *   score=0.0 → boost=75, score=0.5 → boost=112.5, score=1.0 → boost=150
 *
 * This produces a boost range of 75 across the full score spectrum. A ~0.5
 * score difference generates a ~37.5-point boost gap, enough to decisively
 * outrank entities with stronger text matches but lower popularity.
 * A strong exact name match (~20 BM25) can still beat a small score
 * advantage, preserving text relevance for close score matchups.
 */
export const SCORE_BOOST = 75.0

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
 * Set to 8.0 to create a strong signal for exact token matches while
 * keeping the boost proportional to SCORE_BOOST=75 so that score field
 * differences can still override text match gaps between entities.
 */
export const NAME_EXACT_TOKEN_BOOST = 30.0

/**
 * Boost value for exact raw name match on the name_raw keyword field.
 * Uses a `term` query against name_raw which preserves the original string
 * (case-sensitive), so "World affairs" matches "World affairs" but NOT
 * "world-affairs" or "World Affairs".
 * The standard analyzer treats these identically (both tokenize to ["world", "affairs"]),
 * so this is the only way to prefer the exact string match.
 * Note: name_raw is a separate top-level field because search_as_you_type ignores
 * custom subfields (name.raw does not work).
 */
export const NAME_RAW_EXACT_BOOST = 10.0

/**
 * Boost value for case-insensitive raw name match on the name_raw keyword field.
 * Uses a `term` query with case_insensitive: true, so "world affairs" matches
 * "World affairs", "WORLD AFFAIRS", etc. but NOT "world-affairs" (different string).
 * This BM25 clause is paired with a constant_score clause (flat 50 points) to ensure
 * exact full name matches get a predictable boost that isn't diluted by IDF.
 */
export const NAME_RAW_CASE_INSENSITIVE_BOOST = 5.0

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

// System IDs from the SDK are already dashless — use directly for OpenSearch queries
const TYPE_RELATION_TYPE_ID = SystemIds.TYPES_PROPERTY as string
const AVATAR_RELATION_TYPE_ID = ContentIds.AVATAR_PROPERTY as string
const COVER_RELATION_TYPE_ID = SystemIds.COVER_PROPERTY as string

interface SubspacesResult {
	subspaces: string[]
	isRoot: boolean
}

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
	private topologyServiceUrl: string | null
	private subspaceCache: Map<string, {result: SubspacesResult; expiry: number}>
	private rootSpaceId: string | null
	private activeBoosts: BoostOverrides | undefined

	/**
	 * Create a new OpenSearch client.
	 *
	 * @param nodeUrl - The OpenSearch server URL
	 * @param indexName - The index name to use (default: "entities")
	 * @param topologyServiceUrl - The topology service URL for subspace lookups (optional)
	 */
	constructor(nodeUrl: string, indexName: string = "entities", topologyServiceUrl?: string) {
		this.client = new Client({node: nodeUrl})
		this.indexName = indexName
		this.topologyServiceUrl = topologyServiceUrl ?? process.env.TOPOLOGY_SERVICE_URL ?? null
		this.subspaceCache = new Map()
		this.rootSpaceId = null
		this.activeBoosts = undefined
	}

	/**
	 * Initialize the client by fetching the topology root space ID.
	 * Best-effort: if topology service is unavailable, logs a warning and continues.
	 * The root will be discovered lazily on the first root-space query via fetchSubspaces().
	 */
	async init(): Promise<void> {
		if (!this.topologyServiceUrl) return

		try {
			const response = await fetch(`${this.topologyServiceUrl}/topology/root`, {
				signal: AbortSignal.timeout(3000),
			})

			if (response.ok) {
				const data = (await response.json()) as {root_id: string}
				this.rootSpaceId = data.root_id
				console.log(`Cached topology root space ID: ${this.rootSpaceId}`)
			} else {
				console.warn(`Topology root endpoint returned ${response.status}, will discover root lazily`)
			}
		} catch (error) {
			console.warn("Failed to fetch topology root on init, will discover root lazily:", error)
		}
	}

	/**
	 * Check if a space ID is the cached root space.
	 *
	 * Both sides are normalized to dashless before comparison so the check
	 * works regardless of which format the caller supplied or which format
	 * the topology service returned. Without this, a caller passing the
	 * dashed root ID would fail to match a dashless cached root (and vice
	 * versa) — silently dropping the canonical-graph rewrite in
	 * `buildAdditionalSpacesFilter` and the SPACE-scope short-circuits.
	 *
	 * Falls back to strict equality if either value isn't a valid UUID
	 * (shouldn't happen in normal flow — route handlers validate UUIDs
	 * upstream — but the cache could be set from an unvalidated source).
	 */
	private isRootSpace(spaceId: string): boolean {
		if (this.rootSpaceId === null) return false
		try {
			return normalizeUuid(spaceId) === normalizeUuid(this.rootSpaceId)
		} catch {
			return spaceId === this.rootSpaceId
		}
	}

	/**
	 * Execute a search query against the index.
	 */
	async search(query: SearchQuery): Promise<SearchResponse> {
		this.activeBoosts = query.boosts
		const searchBody = (await this.buildSearchBody(query)) as Record<string, unknown>

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
			const avatarImageEntityId = relations?.find(
				(rel) => normalizeUuid(rel.relation_type) === AVATAR_RELATION_TYPE_ID,
			)?.to_entity_id
			const coverImageEntityId = relations?.find(
				(rel) => normalizeUuid(rel.relation_type) === COVER_RELATION_TYPE_ID,
			)?.to_entity_id
			const normalizedAvatarId = avatarImageEntityId ? (normalizeUuid(avatarImageEntityId) as string) : undefined
			const normalizedCoverId = coverImageEntityId ? (normalizeUuid(coverImageEntityId) as string) : undefined
			if (normalizedAvatarId) allImageEntityIds.add(normalizedAvatarId)
			if (normalizedCoverId) allImageEntityIds.add(normalizedCoverId)

			const spaceTopicEntityId = hit._source.space_topic_entity_id as string | undefined
			if (spaceTopicEntityId) {
				allSpaceTopicEntityIds.add(normalizeUuid(spaceTopicEntityId) as string)
			}

			return {
				hit,
				typeIds,
				spaceTopicEntityId,
				avatarImageEntityId: normalizedAvatarId,
				coverImageEntityId: normalizedCoverId,
			}
		})

		// Batch-resolve type names, space metadata, and image URLs in parallel
		const [typeNameMap, spaceMetadataMap, imageUrlMap] = await Promise.all([
			this.resolveTypeNames([...allTypeEntityIds]),
			this.resolveSpaceMetadata([...allSpaceTopicEntityIds]),
			this.resolveImageUrls([...allImageEntityIds]),
		])

		// Second pass: build results with enriched data
		const results: SearchResult[] = hitData.map(
			({hit, typeIds, spaceTopicEntityId, avatarImageEntityId, coverImageEntityId}) => {
				// Compute relevanceScore and textMatchScore
				const relevanceScore = hit._score
				const scoreBoost = hit.fields?.score_boost?.[0]
				const textMatchScore =
					scoreBoost !== undefined ? Math.max(0, relevanceScore - scoreBoost) : relevanceScore

				// Build enriched types array
				const types: SearchResultType[] | undefined = typeIds?.length
					? typeIds.map((id) => ({id, name: typeNameMap.get(id)}))
					: undefined

				// Build enriched space object
				const spaceId = normalizeUuid(hit._source.space_id as string) as string
				const normalizedTopicId = spaceTopicEntityId ? (normalizeUuid(spaceTopicEntityId) as string) : undefined
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
					inCanonicalGraph: (hit._source.in_canonical_graph as boolean) ?? false,
				}
			},
		)

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
	 * Avatar/cover are resolved from relations on the topic entity → image entities.
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
				_source: ["entity_id", "name", "description", "relations"],
				size: topicEntityIds.length,
			},
		})

		// Collect image entity IDs from avatar/cover relations on topic entities
		const imageEntityIds = new Set<string>()
		for (const hit of response.body.hits.hits) {
			const source = hit._source as Record<string, unknown>
			const relations = source.relations as Array<{relation_type: string; to_entity_id: string}> | undefined
			if (relations) {
				for (const rel of relations) {
					const relType = normalizeUuid(rel.relation_type) as string
					if (relType === AVATAR_RELATION_TYPE_ID || relType === COVER_RELATION_TYPE_ID) {
						imageEntityIds.add(normalizeUuid(rel.to_entity_id) as string)
					}
				}
			}
		}

		// Resolve image URLs for avatar/cover
		const imageUrlMap =
			imageEntityIds.size > 0 ? await this.resolveImageUrls([...imageEntityIds]) : new Map<string, string>()

		const metadataMap = new Map<string, {name?: string; description?: string; avatar?: string; cover?: string}>()
		for (const hit of response.body.hits.hits) {
			const source = hit._source as Record<string, unknown>
			const entityId = normalizeUuid(source.entity_id as string) as string
			if (!metadataMap.has(entityId)) {
				const relations = source.relations as Array<{relation_type: string; to_entity_id: string}> | undefined

				// Resolve avatar/cover from relations → image entities
				let avatar: string | undefined
				let cover: string | undefined
				if (relations) {
					const avatarRel = relations.find(
						(rel) => normalizeUuid(rel.relation_type) === AVATAR_RELATION_TYPE_ID,
					)
					if (avatarRel) avatar = imageUrlMap.get(normalizeUuid(avatarRel.to_entity_id) as string)
					const coverRel = relations.find(
						(rel) => normalizeUuid(rel.relation_type) === COVER_RELATION_TYPE_ID,
					)
					if (coverRel) cover = imageUrlMap.get(normalizeUuid(coverRel.to_entity_id) as string)
				}

				metadataMap.set(entityId, {
					name: source.name as string | undefined,
					description: source.description as string | undefined,
					avatar,
					cover,
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
	async buildSearchBody(query: SearchQuery): Promise<object> {
		const includeDeleted = query.include_deleted ?? false
		const includeNonCanonical = query.include_non_canonical ?? true

		// Check if the query is empty or whitespace-only
		const trimmedQuery = query.query.trim()
		const excludeTypeIds = query.exclude_type_ids
		const additionalSpaceIds = query.additional_space_ids
		if (trimmedQuery.length === 0) {
			// For empty queries, return top ranked results based on scope
			return await this.buildTopRankedQuery(
				query.scope,
				query.space_id,
				query.type_ids,
				includeDeleted,
				excludeTypeIds,
				includeNonCanonical,
				additionalSpaceIds,
			)
		}

		// Check if the query is a UUID for direct ID lookup (dashed or dashless)
		if (UUID_DASHED_PATTERN.test(trimmedQuery) || UUID_DASHLESS_PATTERN.test(trimmedQuery)) {
			return await this.buildUuidQuery(
				trimmedQuery,
				query.scope,
				query.space_id,
				query.type_ids,
				includeDeleted,
				excludeTypeIds,
				includeNonCanonical,
				additionalSpaceIds,
			)
		}

		// Build base text search query
		const baseTextQuery = this.buildBaseTextQuery(trimmedQuery)

		// Apply scope-specific query building
		switch (query.scope) {
			case "GLOBAL":
				return this.buildGlobalQuery(
					baseTextQuery,
					query.type_ids,
					includeDeleted,
					excludeTypeIds,
					includeNonCanonical,
					additionalSpaceIds,
				)

			case "GLOBAL_BY_SPACE_SCORE":
				return this.buildGlobalBySpaceScoreQuery(
					baseTextQuery,
					query.type_ids,
					includeDeleted,
					excludeTypeIds,
					includeNonCanonical,
					additionalSpaceIds,
				)

			case "GLOBAL_BY_ENTITY_SPACE_SCORE":
				return this.buildGlobalByEntitySpaceScoreQuery(
					baseTextQuery,
					query.type_ids,
					includeDeleted,
					excludeTypeIds,
					includeNonCanonical,
					additionalSpaceIds,
				)

			case "SPACE_SINGLE": {
				if (!query.space_id) {
					throw SearchError.validationError("SPACE_SINGLE scope requires space_id")
				}
				return this.buildSingleSpaceQuery(
					baseTextQuery,
					query.space_id,
					query.type_ids,
					includeDeleted,
					excludeTypeIds,
					includeNonCanonical,
				)
			}

			case "SPACE": {
				if (!query.space_id) {
					throw SearchError.validationError("SPACE scope requires space_id")
				}
				// Short-circuit for cached root space — no subspace fetch needed
				if (this.isRootSpace(query.space_id)) {
					return this.buildMultiSpaceQuery(
						baseTextQuery,
						[],
						query.type_ids,
						includeDeleted,
						true,
						excludeTypeIds,
						includeNonCanonical,
					)
				}
				const {subspaces, isRoot} = await this.fetchSubspaces(query.space_id)
				return this.buildMultiSpaceQuery(
					baseTextQuery,
					subspaces,
					query.type_ids,
					includeDeleted,
					isRoot,
					excludeTypeIds,
					includeNonCanonical,
				)
			}

			default:
				return this.buildGlobalQuery(
					baseTextQuery,
					query.type_ids,
					includeDeleted,
					excludeTypeIds,
					includeNonCanonical,
					additionalSpaceIds,
				)
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
	async buildUuidQuery(
		uuid: string,
		scope: SearchScope,
		space_id?: string,
		typeIds?: string[],
		includeDeleted: boolean = false,
		excludeTypeIds?: string[],
		includeNonCanonical: boolean = false,
		additionalSpaceIds?: string[],
	): Promise<object> {
		// Match both dashed and dashless forms (index may contain either during migration)
		const baseUuidQuery = {
			terms: {entity_id: uuidTermVariants(uuid)},
		}

		const typeFilter = this.buildTypeFilter(typeIds)
		const typeExclusionFilter = this.buildTypeExclusionFilter(excludeTypeIds)
		const additionalSpacesFilter = this.buildAdditionalSpacesFilter(additionalSpaceIds)
		const filters: object[] = []
		const mustNot: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (!includeNonCanonical) filters.push(this.buildCanonicalFilter())
		if (typeFilter) filters.push(typeFilter)
		if (additionalSpacesFilter) filters.push(additionalSpacesFilter)
		if (typeExclusionFilter) mustNot.push(typeExclusionFilter)

		const buildBoolQuery = () => ({
			query: {
				bool: {
					must: [baseUuidQuery],
					filter: filters,
					...(mustNot.length > 0 && {must_not: mustNot}),
				},
			},
		})

		// Apply scope-specific filtering
		switch (scope) {
			case "GLOBAL":
			case "GLOBAL_BY_SPACE_SCORE":
			case "GLOBAL_BY_ENTITY_SPACE_SCORE":
				return buildBoolQuery()

			case "SPACE_SINGLE":
				if (space_id) {
					filters.push({terms: {space_id: uuidTermVariants(space_id)}})
				}
				return buildBoolQuery()

			case "SPACE":
				if (space_id) {
					if (this.isRootSpace(space_id)) {
						filters.push({term: {in_canonical_graph: true}})
					} else {
						const {subspaces, isRoot} = await this.fetchSubspaces(space_id)
						if (isRoot) {
							filters.push({term: {in_canonical_graph: true}})
						} else {
							filters.push({terms: {space_id: subspaces.flatMap(uuidTermVariants)}})
						}
					}
				}
				return buildBoolQuery()

			default:
				return buildBoolQuery()
		}
	}

	/**
	 * Build a query for returning top ranked results without text matching.
	 * Used when no search query is provided - returns results ranked by scope-specific score fields.
	 */
	async buildTopRankedQuery(
		scope: SearchScope,
		space_id?: string,
		typeIds?: string[],
		includeDeleted: boolean = false,
		excludeTypeIds?: string[],
		includeNonCanonical: boolean = false,
		additionalSpaceIds?: string[],
	): Promise<object> {
		const typeFilter = this.buildTypeFilter(typeIds)
		const typeExclusionFilter = this.buildTypeExclusionFilter(excludeTypeIds)
		const additionalSpacesFilter = this.buildAdditionalSpacesFilter(additionalSpaceIds)
		const filters: object[] = []
		const mustNot: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (!includeNonCanonical) filters.push(this.buildCanonicalFilter())
		if (typeFilter) filters.push(typeFilter)
		if (additionalSpacesFilter) filters.push(additionalSpacesFilter)
		if (typeExclusionFilter) mustNot.push(typeExclusionFilter)

		const buildBoolClause = () => ({
			must: [{match_all: {}}],
			filter: filters,
			...(mustNot.length > 0 && {must_not: mustNot}),
		})

		// Apply scope-specific filtering and sorting
		switch (scope) {
			case "GLOBAL":
				return {
					query: {
						function_score: {
							query: {
								bool: buildBoolClause(),
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
								bool: buildBoolClause(),
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
								bool: buildBoolClause(),
							},
							functions: [this.buildGlobalByEntitySpaceBoost()],
							boost_mode: "replace",
							score_mode: "sum",
						},
					},
				}

			case "SPACE_SINGLE":
				if (space_id) {
					filters.push({terms: {space_id: uuidTermVariants(space_id)}})
				}
				return {
					query: {
						function_score: {
							query: {
								bool: buildBoolClause(),
							},
							functions: [this.buildScoreBoostFunction("entity_space_score")],
							boost_mode: "replace",
							score_mode: "sum",
						},
					},
					script_fields: this.buildScoreBoostScriptFields("entity_space_score"),
				}

			case "SPACE":
				if (space_id) {
					if (this.isRootSpace(space_id)) {
						filters.push({term: {in_canonical_graph: true}})
					} else {
						const {subspaces, isRoot} = await this.fetchSubspaces(space_id)
						if (isRoot) {
							filters.push({term: {in_canonical_graph: true}})
						} else {
							filters.push({terms: {space_id: subspaces.flatMap(uuidTermVariants)}})
						}
					}
				}
				return {
					query: {
						function_score: {
							query: {
								bool: buildBoolClause(),
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
								bool: buildBoolClause(),
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
	/**
	 * Get the effective boost value, using the active query override if set.
	 */
	private b(name: keyof BoostOverrides, defaultValue: number): number {
		return this.activeBoosts?.[name] ?? defaultValue
	}

	buildBaseTextQuery(queryText: string): object {
		return {
			bool: {
				should: [
					{
						// Exact raw name match — boosts documents where the query
						// matches the unanalyzed name string exactly. Differentiates
						// "World affairs" from "world-affairs" which the analyzer
						// treats as identical tokens. Uses name_raw (separate keyword
						// field with lowercase normalizer) because search_as_you_type
						// ignores custom subfields (name.raw does not work).
						term: {
							name_raw: {
								value: queryText,
								boost: this.b("name_raw_exact_boost", NAME_RAW_EXACT_BOOST),
							},
						},
					},
					{
						// Case-insensitive raw name match (BM25) — boosts documents where
						// the query matches the full unanalyzed name string ignoring case.
						// "world affairs" matches "World affairs" or "WORLD AFFAIRS"
						// but NOT "world-affairs" (different string structure).
						term: {
							name_raw: {
								value: queryText,
								boost: this.b("name_raw_case_insensitive_boost", NAME_RAW_CASE_INSENSITIVE_BOOST),
								case_insensitive: true,
							},
						},
					},
					{
						// Case-insensitive raw name match (flat bonus) — adds a fixed 50
						// points on top of the BM25 clause above. This ensures exact full
						// name matches get a predictable boost that isn't diluted by IDF
						// when the term appears in many documents.
						constant_score: {
							filter: {
								term: {
									name_raw: {
										value: queryText,
										case_insensitive: true,
									},
								},
							},
							boost: 50,
						},
					},
					{
						// Exact token match on the analyzed name field. The standard
						// analyzer lowercases and splits on special characters (hyphens,
						// underscores, etc.), so "world-affairs" and "World affairs" both
						// match as tokens ["world", "affairs"]. Unlike name.raw above,
						// this does not differentiate based on punctuation or casing.
						// e.g. query "geo" matches name "Geo" (token "geo") but NOT
						// "geojson_preview_tool" (token "geojson" ≠ "geo").
						match: {
							name: {
								query: queryText,
								boost: this.b("name_exact_token_boost", NAME_EXACT_TOKEN_BOOST),
							},
						},
					},
					{
						// Autocomplete-style match over n-grams with higher weight on name
						multi_match: {
							query: queryText,
							type: "bool_prefix",
							fields: [
								`name^${this.b("name_field_boost", NAME_FIELD_BOOST)}`,
								`name._2gram^${this.b("name_field_boost", NAME_FIELD_BOOST)}`,
								`name._3gram^${this.b("name_field_boost", NAME_FIELD_BOOST)}`,
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
							boost: this.b("fuzzy_reduction_boost", FUZZY_REDUCTION_BOOST),
						},
					},
					{
						// Strongly boost documents where the name starts with the query text
						match_phrase_prefix: {
							name: {
								query: queryText,
								boost: this.b("name_prefix_boost", NAME_PREFIX_BOOST),
							},
						},
					},
					{
						// Moderately boost documents where the description starts with the query text
						match_phrase_prefix: {
							description: {
								query: queryText,
								boost: this.b("description_prefix_boost", DESCRIPTION_PREFIX_BOOST),
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
		const scoreBoost = this.b("score_boost", SCORE_BOOST)
		return `
			def scoreValue = doc.containsKey('${scoreField}') && !doc['${scoreField}'].empty
				? doc['${scoreField}'].value
				: ${DEFAULT_AVERAGE_SCORE};
			def clampedScore = Math.max(scoreValue, ${MIN_SCORE_THRESHOLD});
			return (clampedScore + ${SCORE_SHIFT}) * ${scoreBoost};
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
	buildGlobalQuery(
		baseTextQuery: object,
		typeIds?: string[],
		includeDeleted: boolean = false,
		excludeTypeIds?: string[],
		includeNonCanonical: boolean = false,
		additionalSpaceIds?: string[],
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const typeExclusionFilter = this.buildTypeExclusionFilter(excludeTypeIds)
		const additionalSpacesFilter = this.buildAdditionalSpacesFilter(additionalSpaceIds)
		const filters: object[] = []
		const mustNot: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (!includeNonCanonical) filters.push(this.buildCanonicalFilter())
		if (typeFilter) filters.push(typeFilter)
		if (additionalSpacesFilter) filters.push(additionalSpacesFilter)
		if (typeExclusionFilter) mustNot.push(typeExclusionFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
							...(mustNot.length > 0 && {must_not: mustNot}),
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
	buildGlobalBySpaceScoreQuery(
		baseTextQuery: object,
		typeIds?: string[],
		includeDeleted: boolean = false,
		excludeTypeIds?: string[],
		includeNonCanonical: boolean = false,
		additionalSpaceIds?: string[],
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const typeExclusionFilter = this.buildTypeExclusionFilter(excludeTypeIds)
		const additionalSpacesFilter = this.buildAdditionalSpacesFilter(additionalSpaceIds)
		const filters: object[] = []
		const mustNot: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (!includeNonCanonical) filters.push(this.buildCanonicalFilter())
		if (typeFilter) filters.push(typeFilter)
		if (additionalSpacesFilter) filters.push(additionalSpacesFilter)
		if (typeExclusionFilter) mustNot.push(typeExclusionFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
							...(mustNot.length > 0 && {must_not: mustNot}),
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
		excludeTypeIds?: string[],
		includeNonCanonical: boolean = false,
		additionalSpaceIds?: string[],
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const typeExclusionFilter = this.buildTypeExclusionFilter(excludeTypeIds)
		const additionalSpacesFilter = this.buildAdditionalSpacesFilter(additionalSpaceIds)
		const filters: object[] = []
		const mustNot: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (!includeNonCanonical) filters.push(this.buildCanonicalFilter())
		if (typeFilter) filters.push(typeFilter)
		if (additionalSpacesFilter) filters.push(additionalSpacesFilter)
		if (typeExclusionFilter) mustNot.push(typeExclusionFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
							...(mustNot.length > 0 && {must_not: mustNot}),
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
		excludeTypeIds?: string[],
		includeNonCanonical: boolean = false,
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const typeExclusionFilter = this.buildTypeExclusionFilter(excludeTypeIds)
		const filters: object[] = [{terms: {space_id: uuidTermVariants(spaceId)}}]
		const mustNot: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (!includeNonCanonical) filters.push(this.buildCanonicalFilter())
		if (typeFilter) filters.push(typeFilter)
		if (typeExclusionFilter) mustNot.push(typeExclusionFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
							...(mustNot.length > 0 && {must_not: mustNot}),
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
	 * Build a multi-space filtered query.
	 * Filters by multiple space_ids (subspaces) and boosts by entity_space_score.
	 */
	buildMultiSpaceQuery(
		baseTextQuery: object,
		spaceIds: string[],
		typeIds?: string[],
		includeDeleted: boolean = false,
		isRoot: boolean = false,
		excludeTypeIds?: string[],
		includeNonCanonical: boolean = false,
	): object {
		const typeFilter = this.buildTypeFilter(typeIds)
		const typeExclusionFilter = this.buildTypeExclusionFilter(excludeTypeIds)
		const spaceFilter = isRoot
			? {term: {in_canonical_graph: true}}
			: {terms: {space_id: spaceIds.flatMap(uuidTermVariants)}}
		const filters: object[] = [spaceFilter]
		const mustNot: object[] = []
		if (!includeDeleted) filters.push(this.buildNonDeletedFilter())
		if (!includeNonCanonical) filters.push(this.buildCanonicalFilter())
		if (typeFilter) filters.push(typeFilter)
		if (typeExclusionFilter) mustNot.push(typeExclusionFilter)

		return {
			query: {
				function_score: {
					query: {
						bool: {
							must: [baseTextQuery],
							filter: filters,
							...(mustNot.length > 0 && {must_not: mustNot}),
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
	 * Fetch subspace IDs for a given space from the topology service.
	 * Falls back to returning just the original space_id if topology service is not configured or returns 404.
	 * Throws on non-404 HTTP errors and network/timeout failures so callers return 500.
	 * Results are cached for 30 seconds.
	 */
	async fetchSubspaces(spaceId: string): Promise<SubspacesResult> {
		// Check cache first
		const cached = this.subspaceCache.get(spaceId)
		if (cached && cached.expiry > Date.now()) {
			return cached.result
		}

		if (!this.topologyServiceUrl) {
			return {subspaces: [spaceId], isRoot: false}
		}

		try {
			const response = await fetch(`${this.topologyServiceUrl}/topology/subspaces/${spaceId}`, {
				signal: AbortSignal.timeout(3000),
			})

			if (response.status === 404) {
				// Space not in canonical graph — fall back to single space
				const result: SubspacesResult = {subspaces: [spaceId], isRoot: false}
				this.subspaceCache.set(spaceId, {result, expiry: Date.now() + 30_000})
				return result
			}

			if (!response.ok) {
				const body = await response.text().catch(() => "")
				console.error(`Topology service error: status=${response.status} space=${spaceId} body=${body}`)
				throw new Error(`Topology service returned ${response.status} for space ${spaceId}`)
			}

			const data = (await response.json()) as {subspaces: string[]; is_root?: boolean}
			const result: SubspacesResult = {subspaces: data.subspaces, isRoot: data.is_root === true}
			// Lazy backfill: if we discover the root via subspaces response, cache it
			if (result.isRoot && this.rootSpaceId === null) {
				this.rootSpaceId = spaceId
				console.log(`Lazily cached topology root space ID: ${this.rootSpaceId}`)
			}
			this.subspaceCache.set(spaceId, {result, expiry: Date.now() + 30_000})
			return result
		} catch (error) {
			console.error(`Topology service fetch failed: space=${spaceId} error=${error}`)
			throw error instanceof Error ? error : new Error(`Failed to fetch subspaces for space ${spaceId}: ${error}`)
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
	 * Build a type exclusion filter that removes entities with any of the specified types.
	 * Returns null if no excludeTypeIds are provided.
	 */
	buildTypeExclusionFilter(excludeTypeIds?: string[]): object | null {
		if (!excludeTypeIds || excludeTypeIds.length === 0) {
			return null
		}

		const allVariants = excludeTypeIds.flatMap((id) => uuidTermVariants(id))

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
	 * Build a filter to select entities in the canonical graph.
	 * Matches documents where in_canonical_graph is true.
	 */
	buildCanonicalFilter(): object {
		return {term: {in_canonical_graph: true}}
	}

	/**
	 * Build the additional-spaces eligibility filter.
	 *
	 * If the canonical-graph root space ID appears in the list, it is rewritten
	 * to `in_canonical_graph: true` (i.e. "the whole canonical graph"); other
	 * IDs become a `terms: {space_id: ...}` clause. The two are OR'd via
	 * `bool.should` so the caller's full set is treated as a single
	 * eligibility filter.
	 *
	 * Returns `null` when there's nothing to add (no IDs supplied, or all
	 * supplied IDs were the root and thus collapsed to one canonical clause —
	 * caller can pick one term directly when only one clause is produced).
	 */
	buildAdditionalSpacesFilter(additionalSpaceIds?: string[]): object | null {
		if (!additionalSpaceIds || additionalSpaceIds.length === 0) return null

		const rootIncluded = additionalSpaceIds.some((id) => this.isRootSpace(id))
		const nonRootIds = additionalSpaceIds.filter((id) => !this.isRootSpace(id))

		const canonicalClause = rootIncluded ? this.buildCanonicalFilter() : null
		const spaceIdsClause = nonRootIds.length > 0 ? {terms: {space_id: nonRootIds.flatMap(uuidTermVariants)}} : null

		if (canonicalClause && spaceIdsClause) {
			return {bool: {should: [canonicalClause, spaceIdsClause], minimum_should_match: 1}}
		}
		return canonicalClause ?? spaceIdsClause
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
