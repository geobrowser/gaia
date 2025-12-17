# Search Indexer Shared

Shared types and data structures for the search indexer system.

## Overview

This crate provides the common data types used across the search indexer ecosystem:

- **EntityDocument**: The document structure indexed in the search engine

## Usage

```rust
use search_indexer_shared::EntityDocument;
use uuid::Uuid;

// Create a document to index
let doc = EntityDocument::new(
    entity_id,
    space_id,
    "Entity Name".to_string(),
    Some("Description text".to_string()),
);
```

## Types

### EntityDocument

Represents an entity document in the search index. Scores are `None` by default
until the scoring service is implemented in a future version.
