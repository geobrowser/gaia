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
	/** Maximum number of results to return (default: 20, max: 100). */
	limit?: number
	/** Offset for pagination (default: 0). */
	offset?: number
	/** Include soft-deleted entities in results (default: false). */
	include_deleted?: boolean
}

/**
 * A single search result item.
 */
export interface SearchResult {
	/** The entity's unique identifier. */
	entityId: string
	/** The space this entity belongs to. */
	spaceId: string
	/** Optional entity display name. */
	name?: string
	/** Optional description text. */
	description?: string
	/** Optional avatar image URL. */
	avatar?: string
	/** Optional cover image URL. */
	cover?: string
	/** Type IDs associated with this entity (extracted from type_relations). */
	typeIds?: string[]
	/** Global entity score. */
	entityGlobalScore?: number
	/** Space score. */
	spaceScore?: number
	/** Entity-space score. */
	entitySpaceScore?: number
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
