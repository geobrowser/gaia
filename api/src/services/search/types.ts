/**
 * Search service types.
 *
 * These types define the API contract for the search service.
 * Note: This TypeScript API is read-only - it only queries the search index.
 * All indexing/updating/deleting is done by the Rust search-indexer service.
 */

/**
 * Defines the scope of a search query.
 *
 * - GLOBAL: Search across all spaces, boosted by entity_global_score
 * - GLOBAL_BY_SPACE_SCORE: Search across all spaces, boosted by space_score
 * - GLOBAL_BY_ENTITY_SPACE_SCORE: Search across all spaces, boosted by entity_space_score * space_score
 * - SPACE_SINGLE: Search within a single specific space, boosted by entity_space_score
 * - SPACE: Search within a space and its subspaces (currently implemented as single space)
 */
export type SearchScope = "GLOBAL" | "GLOBAL_BY_SPACE_SCORE" | "GLOBAL_BY_ENTITY_SPACE_SCORE" | "SPACE_SINGLE" | "SPACE"

/**
 * Search query parameters.
 */
export interface SearchQuery {
	/** The search query string. */
	query: string
	/** The scope of the search. */
	scope: SearchScope
	/**
	 * Space ID for space-scoped searches.
	 * Required for SPACE_SINGLE and SPACE scopes.
	 */
	space_id?: string
	/** Set of type IDs to filter results by. Results must have at least one of the specified type IDs. */
	type_ids?: string[]
	/** Set of type IDs to exclude from results. Entities with any of these types will be filtered out. */
	exclude_type_ids?: string[]
	/**
	 * Additional space IDs to widen the eligibility set on `GLOBAL`-family scopes.
	 *
	 * Caller passes the spaces they want results from. If the canonical-graph
	 * root space ID is included in the list, it is rewritten into the
	 * `in_canonical_graph: true` filter (i.e. "all canonical-graph spaces");
	 * remaining IDs become a `space_id IN (...)` clause. The two are OR'd to
	 * form one eligibility filter. Empty/absent → behavior unchanged.
	 *
	 * Not valid with `SPACE` or `SPACE_SINGLE` scopes — those scopes already
	 * define their own space set.
	 */
	additional_space_ids?: string[]
	/** Maximum number of results to return (default: 20, max: 100). */
	limit?: number
	/** Offset for pagination (default: 0). */
	offset?: number
	/** Include soft-deleted entities in results (default: false). */
	include_deleted?: boolean
	/** Include entities from non-canonical spaces (default: false). */
	include_non_canonical?: boolean
	/** Optional boost overrides for tuning search relevance. */
	boosts?: BoostOverrides
}

/**
 * Optional overrides for search boost values.
 * When provided, these override the corresponding constants in opensearch.ts.
 */
export interface BoostOverrides {
	score_boost?: number
	name_prefix_boost?: number
	description_prefix_boost?: number
	name_field_boost?: number
	name_exact_token_boost?: number
	name_raw_exact_boost?: number
	name_raw_case_insensitive_boost?: number
	fuzzy_reduction_boost?: number
}

/**
 * A type associated with a search result entity.
 */
export interface SearchResultType {
	/** The type entity's unique identifier. */
	id: string
	/** The type entity's display name, if available. */
	name?: string
}

/**
 * Space metadata for a search result entity.
 */
export interface SearchResultSpace {
	/** The space's unique identifier. */
	id: string
	/** The space's display name, if available. */
	name?: string
	/** The space's description, if available. */
	description?: string
	/** The space's avatar image URL, if available. */
	avatar?: string
	/** The space's cover image URL, if available. */
	cover?: string
}

/**
 * A single search result item.
 */
export interface SearchResult {
	/** The entity's unique identifier. */
	entityId: string
	/** The space this entity belongs to, with optional metadata. */
	space: SearchResultSpace
	/** Optional entity display name. */
	name?: string
	/** Optional description text. */
	description?: string
	/** Optional avatar image URL. */
	avatar?: string
	/** Optional cover image URL. */
	cover?: string
	/** Types associated with this entity, with optional names. */
	types?: SearchResultType[]
	/** Global entity score. */
	entityGlobalScore?: number
	/** Space score. */
	spaceScore?: number
	/** Entity-space score. */
	entitySpaceScore?: number
	/** Final relevance score after all boosts (OpenSearch _score). */
	relevanceScore?: number
	/** Text matching score without score field boosts. */
	textMatchScore?: number
	/** Whether this entity's space is in the canonical graph. */
	inCanonicalGraph: boolean
}

/**
 * Complete search response with results and metadata.
 */
export interface SearchResponse {
	/** The list of search results, ordered by relevance. */
	results: SearchResult[]
	/** Total number of matching documents. */
	total: number
	/** Time taken to execute the search in milliseconds. */
	tookMs: number
}

/**
 * Search error types.
 */
export enum SearchErrorType {
	ValidationError = "ValidationError",
}

/**
 * Search error class.
 */
export class SearchError extends Error {
	constructor(
		public readonly type: SearchErrorType,
		message: string,
		public readonly details?: unknown,
	) {
		super(message)
		this.name = "SearchError"
		Object.setPrototypeOf(this, SearchError.prototype)
	}

	static validationError(message: string, details?: unknown): SearchError {
		return new SearchError(SearchErrorType.ValidationError, message, details)
	}
}

/**
 * Type guard to check if a value is a SearchError.
 */
export function isSearchError(value: unknown): value is SearchError {
	return value instanceof SearchError
}
