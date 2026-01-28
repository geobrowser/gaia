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

## Pending: Indexer Updates

### indexer (legacy)
- Uses v1 protobuf `wire::pb::grc20::Edit` directly
- Will be superseded by `kg-indexer` for knowledge graph indexing
- No v2 migration planned

### kg-indexer: Op Types Not Yet Handled
The kg-indexer decodes v2 payloads but doesn't yet handle all 9 op types:

**Handled:**
- CreateEntity
- UpdateEntity
- CreateRelation
- UpdateRelation
- DeleteRelation

**Not handled (silently ignored):**
- DeleteEntity - requires soft delete columns (see Deferred Schema Changes)
- RestoreEntity - requires soft delete columns
- RestoreRelation - requires soft delete columns
- CreateValueRef - reified entity value references

**Unblock by:** Task ts-46f76a (Apply v2 ops)

## Implemented

### search-indexer (feat/search-delete-entity)
- Decodes `HermesEdit.payload` with `grc_20::decode_edit()`
- Handles the following operations:
  - `UpdateEntity` - Extracts name, description, avatar; handles unset_values
  - `CreateRelation` - Processes type relations only (TYPE_RELATION_TYPE_ID)
  - `DeleteRelation` - Removes type relations from entities
  - `DeleteEntity` - Soft delete (marks entities as deleted in OpenSearch)
- Operations intentionally skipped (not relevant for search):
  - `CreateEntity`, `RestoreEntity`, `RestoreRelation`, `UpdateRelation`, `CreateValueRef`
- E2E tests added for delete entity and unset properties functionality

### Schema (ts-20e6b4)
- `dataTypesEnum` updated to v2 types: `Bool`, `Int64`, `Float64`, `Decimal`, `Text`, `Bytes`, `Date`, `Time`, `Datetime`, `Schedule`, `Point`, `Embedding`
- New value columns in `values` table: `integer`, `float`, `bytes`, `date`, `datetime`, `schedule`, `embedding`
- Indexes for new searchable columns

### Hermes Pipeline (ts-ad5c1e)
- `HermesEdit` proto updated: `.ops` replaced with `.payload` (raw GRC2/GRC2Z bytes)
- `hermes-ipfs-cache` validates with `grc_20::decode_edit()` before storing
- `hermes-pipeline` passes raw bytes through to Kafka
- Mock infrastructure updated (`IpfsSource::mock_bytes`)

### kg-indexer (ts-d1cfd3, ts-2da76d)
- Decodes `HermesEdit.payload` with `grc_20::decode_edit()`
- Storage layer writes to v2 value columns (integer, float, bytes, date, datetime, schedule, embedding)
- Value types determined from `grc_20::Value` enum variant (no separate property type lookup)
- Handles 5 of 9 op types (see Pending section for gaps)
