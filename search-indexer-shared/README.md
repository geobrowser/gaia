# Search Indexer Shared

Shared types and data structures for the search indexer system.

## Overview

This crate provides the common data types used across the search indexer ecosystem:

- **EntityDocument**: The document structure indexed in the search engine
- **SearchQuery**: Query parameters for search operations
- **SearchResult**: Individual search result items
- **SearchResponse**: Complete search response with metadata

## Usage

```rust
use search_indexer_shared::types::{EntityDocument, SearchQuery, SearchScope};

// Create a document to index
let doc = EntityDocument::new(
    entity_id,
    space_id,
    "Entity Name".to_string(),
    Some("Description text".to_string()),
);

// Build a search query
let query = SearchQuery {
    query: "search term".to_string(),
    scope: SearchScope::Global,
    space_ids: None,
    limit: 20,
    offset: 0,
};
```

## Types

### EntityDocument

Represents an entity document in the search index. Scores are `None` by default
until the scoring service is implemented in a future version.

### SearchScope

Defines the scope of a search query:

- `Global` - Search across all entities
- `GlobalBySpaceScore` - Search globally with space score breakdowns
- `SpaceSingle` - Search within a single space only
- `Space` - Search within a space and its subspaces

