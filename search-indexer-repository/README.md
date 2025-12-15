# Search Indexer Repository

Repository interfaces and implementations for the search indexer system.

## Overview

This crate provides:

- **SearchIndexProvider trait**: Abstract interface for search index operations
- **OpenSearchProvider**: Concrete implementation using OpenSearch

## Architecture

The crate uses a trait-based design for dependency injection, allowing:

- Easy testing with mock implementations
- Swappable search backends
- Clean separation of concerns

```
┌─────────────────────────────────────┐
│   SearchIndexProvider               │  (trait)
│  - update_document()                │  (upsert: create or update)
│  - delete_document()                │
│  - bulk_update_documents()          │
│  - bulk_delete_documents()          │
└─────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────┐
│        OpenSearchProvider           │  (implementation)
│  - Uses opensearch crate            │
│  - Configurable index settings      │
└─────────────────────────────────────┘
```

## Note on Document Creation

There is no separate `create` or `index_document` function because creates in grc-20 are just updates.
The `update_document` function performs an upsert operation: it will create the document if it doesn't exist,
or update it if it does exist.

## Usage

```rust
use search_indexer_repository::{OpenSearchProvider, SearchIndexProvider};
use search_indexer_repository::types::UpdateEntityRequest;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create provider
    use search_indexer_repository::opensearch::IndexConfig;
    let config = IndexConfig::new("entities", 0);
    let provider = OpenSearchProvider::new("http://localhost:9200", config).await?;
    
    // Update a document (Creates it if it doesn't exist, updates it if it does)
    let request = UpdateEntityRequest {
        entity_id: uuid::Uuid::new_v4().to_string(),
        space_id: uuid::Uuid::new_v4().to_string(),
        name: Some("My Entity".to_string()),
        description: Some("Description".to_string()),
        ..Default::default()
    };
    // This will create the document if it doesn't exist, or update it if it does
    provider.update_document(&request).await?;
    
    Ok(())
}
```

## Index Configuration

The OpenSearch index is configured with:

- **search_as_you_type fields**: Built-in field type for autocomplete on name and description (uses n-grams internally)
- **rank_feature fields**: Score fields (entity_global_score, space_score, entity_space_score) optimized for relevance boosting

## Error Handling

All operations return `Result<T, SearchIndexError>` with specific error types:

- `ValidationError`: Input validation failed (e.g., invalid UUIDs)
- `ConnectionError`: Failed to connect to OpenSearch
- `UpdateError`: Document update/creation failed
- `DeleteError`: Document deletion failed
- `IndexCreationError`: Failed to create the search index
- `ParseError`: Failed to parse response from search index backend
- `SerializationError`: Failed to serialize data for the search index backend
- `DocumentNotFound`: Document not found (note: `update_document` performs upsert, so this won't occur for updates)
- `BatchSizeExceeded`: Batch size exceeds configured maximum
- `Unknown`: Unknown error

