# GRC-20 v2 Implementation Gaps

Tracking features from the GRC-20 v2 spec that aren't yet implemented in the backend.

## Migration Strategy

The v2 schema changes (new enum values, new columns) require a **wipe and re-index**:
1. Drop existing tables (or wipe the database)
2. Run migrations fresh
3. Re-index from scratch

This is necessary because PostgreSQL enums can't have values removed while existing data references them, and we're replacing v1 types (`String`, `Number`, `Boolean`, `Relation`) with v2 types (`Bool`, `Int64`, `Float64`, etc.).

## Deferred Schema Changes

### Soft Deletes
- `deleted_at` column for `entities` table
- `deleted_at` column for `relations` table

**Why deferred:** Adding these columns would cause existing PostGraphile queries to return deleted records. Need to update the API layer to filter `WHERE deleted_at IS NULL` before adding the columns.

**Unblock by:** Update PostGraphile/GraphQL queries to handle soft deletes, or add database views that filter deleted records.

### Properties Table
- v2 has no global property→type map (types declared per-edit, not globally)
- Current `properties.type` column may become optional or unused

**Why deferred:** Need to understand how this affects existing queries and the indexer before changing.

## Implemented

### Schema (ts-20e6b4)
- `dataTypesEnum` updated to v2 types: `Bool`, `Int64`, `Float64`, `Decimal`, `Text`, `Bytes`, `Date`, `Time`, `Datetime`, `Schedule`, `Point`, `Embedding`
- New value columns in `values` table: `integer`, `float`, `bytes`, `date`, `datetime`, `schedule`, `embedding`
- Indexes for new searchable columns
