# Search Implementation

## Overview

This implementation provides fuzzy search capabilities for entities using PostgreSQL's `pg_trgm` extension for trigram-based similarity matching. The search operates on all entity values (properties) and returns ranked results based on text similarity.

## Features

- **Trigram Similarity**: Uses PostgreSQL's `pg_trgm` extension for fuzzy text matching
- **Configurable Threshold**: Adjustable similarity threshold (default: 0.3)
- **Space Filtering**: Optional filtering by space ID
- **Ranked Results**: Results ordered by similarity score (highest first)
- **Performance Optimized**: GIN indexes for fast search operations
- **SystemIds Integration**: Uses standard property IDs from `@graphprotocol/grc-20`

## GraphQL Schema

```graphql
search(
  query: String!
  spaceId: String
  limit: Int = 10
  offset: Int = 0
  threshold: Float = 0.3
): [Entity]!
```

### Parameters

- `query`: The search term (required)
- `spaceId`: Optional space filter - only search within specific space
- `limit`: Maximum number of results (default: 10)
- `offset`: Pagination offset (default: 0)
- `threshold`: Minimum similarity score (0.0-1.0, default: 0.3)

## Usage Examples

### Basic Search
```graphql
query {
  search(query: "artificial intelligence") {
    id
    name
    description
  }
}
```

### Space-Filtered Search
```graphql
query {
  search(
    query: "machine learning"
    spaceId: "space-uuid-here"
    limit: 5
  ) {
    id
    name
    description
  }
}
```

### Custom Threshold
```graphql
query {
  search(
    query: "neural networks"
    threshold: 0.5  # Higher threshold = more strict matching
  ) {
    id
    name
    description
  }
}
```

## Implementation Details

### Two Search Functions

1. **`search()`**: Searches across all entity values
   - Broadest search coverage
   - May include matches from any property type
   - Good for comprehensive discovery

2. **`searchNameDescription()`**: Searches only name and description properties
   - Uses `SystemIds.NAME_PROPERTY` and `SystemIds.DESCRIPTION_PROPERTY` from `@graphprotocol/grc-20`
   - More targeted and relevant results
   - Better performance due to reduced search scope

### Database Schema

The fuzzy search leverages the existing `values` table structure:
- Searches across entity `values` where similarity exceeds threshold
- Uses `pg_trgm`'s `similarity()` function for scoring
- Groups results by entity to avoid duplicates
- Orders by maximum similarity score per entity

### SystemIds Integration

The implementation uses standard property identifiers:
```typescript
import {SystemIds} from "@graphprotocol/grc-20"

// Name and description searches use:
SystemIds.NAME_PROPERTY
SystemIds.DESCRIPTION_PROPERTY
```

This ensures consistency across the knowledge graph ecosystem.

### Indexes

Three indexes support fuzzy search performance:

1. **Trigram Index**: `values_text_trgm_idx`
   - Type: GIN with `gin_trgm_ops`
   - Purpose: Fast trigram similarity searches

2. **Full-Text Index**: `values_fts_idx`
   - Type: GIN with `to_tsvector`
   - Purpose: Alternative full-text search capability

3. **Composite Index**: `values_space_text_idx`
   - Type: B-tree on `(space_id, value)`
   - Purpose: Optimized space-filtered searches

### Query Pattern

```sql
SELECT entities.*, MAX(similarity(values.value, $query)) as similarity
FROM entities
INNER JOIN values ON entities.id = values.entity_id
WHERE similarity(values.value, $query) > $threshold
  AND ($spaceId IS NULL OR values.space_id = $spaceId)
  AND values.value IS NOT NULL
  AND length(trim(values.value)) > 0
GROUP BY entities.id, entities.created_at, entities.created_at_block, 
         entities.updated_at, entities.updated_at_block
ORDER BY MAX(similarity(values.value, $query)) DESC
LIMIT $limit OFFSET $offset;
```

For name/description-only searches, an additional filter is added:
```sql
AND (values.property_id = $NAME_PROPERTY_ID OR values.property_id = $DESCRIPTION_PROPERTY_ID)
```

## Migration

This implementation uses Drizzle's built-in migration system. To enable fuzzy search:

### 1. Generate Schema Migration
The basic indexes are defined in the schema and generated automatically:

```bash
npx drizzle-kit generate
```

### 2. Generate Custom Migration for pg_trgm
Since Drizzle doesn't automatically create PostgreSQL extensions, create a custom migration:

```bash
npx drizzle-kit generate --custom
```

Add the following to the generated custom migration file:

```sql
-- Enable pg_trgm extension for fuzzy search
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Drop the basic text index created by the schema
DROP INDEX IF EXISTS "values_text_idx";

-- Create GIN index for trigram fuzzy search
CREATE INDEX "values_text_trgm_idx" ON "values" USING gin ("value" gin_trgm_ops);

-- Create GIN index for full-text search
CREATE INDEX "values_fts_idx" ON "values" USING gin (to_tsvector('english', "value"));

-- Add comment for documentation
COMMENT ON INDEX "values_text_trgm_idx" IS 'GIN index for trigram-based fuzzy search on entity values';
COMMENT ON INDEX "values_fts_idx" IS 'GIN index for full-text search on entity values';
```

### 3. Run Migrations
Apply all pending migrations:

```bash
npx drizzle-kit migrate
```

## Performance Considerations

- **Index Size**: GIN indexes can be large but provide fast search
- **Similarity Threshold**: Lower thresholds return more results but may be less relevant
- **Query Length**: Very short queries (1-2 characters) may not work well with trigrams
- **Space Filtering**: Including `spaceId` significantly improves performance for large datasets
- **Property Filtering**: Using `fuzzySearchNameDescription()` is faster than searching all properties

## Limitations

- **Trigram Limitation**: Works best with words of 3+ characters
- **Language Support**: Currently optimized for English text
- **Case Sensitivity**: Searches are case-insensitive by default
- **Special Characters**: Punctuation and special characters may affect matching

## Choosing Between Search Functions

**Use `search()`** when:
- You want comprehensive coverage across all entity properties
- You're unsure what type of content you're looking for
- You need to find entities based on any textual property

**Use `searchNameDescription()`** when:
- You specifically want to match against entity names and descriptions
- You need better performance and more relevant results
- You're building a primary search interface for users