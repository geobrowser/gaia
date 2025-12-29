/**
 * Search service module.
 *
 * Re-exports all search service components.
 */

export type {SearchClient} from "./client"
export {OpenSearchClient} from "./opensearch"
export type {
	SearchErrorType,
	SearchQuery,
	SearchResponse,
	SearchResult,
	SearchScope,
} from "./types"
export {isSearchError, SearchError} from "./types"
