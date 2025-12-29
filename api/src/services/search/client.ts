/**
 * Search client interface.
 *
 * This module defines the abstract interface for search operations,
 * allowing for dependency injection of different implementations.
 * Note: This TypeScript API is read-only - it only queries the search index.
 * All indexing/updating/deleting is done by the Rust search-indexer service.
 */

import type {SearchQuery, SearchResponse} from "./types"

/**
 * Abstract search client interface for dependency injection.
 *
 * Implementations can be swapped for testing or alternative search backends.
 *
 * @example
 * ```typescript
 * // Production
 * const client = new OpenSearchClient("http://localhost:9200");
 *
 * // Testing
 * const client = new MockSearchClient();
 *
 * // Use the same interface
 * const results = await client.search({ query: "test", scope: "GLOBAL" });
 * ```
 */
export interface SearchClient {
	/**
	 * Execute a search query against the index.
	 *
	 * @param query - The search query parameters
	 * @returns Promise resolving to search results
	 * @throws Error if the search fails
	 */
	search(query: SearchQuery): Promise<SearchResponse>

	/**
	 * Check if the search engine is healthy.
	 *
	 * @returns Promise resolving to true if healthy
	 */
	healthCheck(): Promise<boolean>
}
