# Context-Aware Versioned Diffs Implementation Plan

## Summary

Consume GRC-20 context metadata in our versioned diffs to enable context-aware change grouping. The grc-20 0.3.0 crate already has full context support (`Context`, `ContextEdge` types on every op) - we just need to extract, store, and use it.

## GRC-20 Context Structure (Already in Crate)

```rust
pub struct Context {
    pub root_id: Id,              // Parent entity (e.g., Byron)
    pub edges: Vec<ContextEdge>,  // Path to changed entity
}

pub struct ContextEdge {
    pub type_id: Id,              // Relation type (e.g., BLOCKS_ID)
    pub to_entity_id: Id,         // Target entity (e.g., TextBlock_9)
}
```

Every op has `context: Option<Context>`. Currently ignored in kg-indexer.

## Implementation Phases

### Phase 1: Database Schema Migration

**New migration in `api/drizzle/`:**

```sql
-- Add context columns to value_versions
ALTER TABLE value_versions ADD COLUMN context_root_id uuid;
ALTER TABLE value_versions ADD COLUMN context_edge_type_id uuid;

-- Add context columns to relation_versions
ALTER TABLE relation_versions ADD COLUMN context_root_id uuid;
ALTER TABLE relation_versions ADD COLUMN context_edge_type_id uuid;

-- Indexes for context-based lookups
CREATE INDEX value_versions_context_idx
  ON value_versions (context_root_id, context_edge_type_id)
  WHERE context_root_id IS NOT NULL;

CREATE INDEX relation_versions_context_idx
  ON relation_versions (context_root_id, context_edge_type_id)
  WHERE context_root_id IS NOT NULL;
```

### Phase 2: kg-indexer Updates

**Files to modify:**

1. **`kg-indexer/src/handlers/edits.rs`** - Extract context from ops:
   ```rust
   use grc_20::Context;

   fn extract_context(ctx: &Option<Context>) -> (Option<Uuid>, Option<Uuid>) {
       match ctx {
           Some(c) => {
               let root_id = id_to_uuid(&c.root_id);
               let edge_type_id = c.edges.first().map(|e| id_to_uuid(&e.type_id));
               (Some(root_id), edge_type_id)
           }
           None => (None, None),
       }
   }
   ```

   Use in `extract_values()` and `extract_relations()` to populate context fields.

2. **`kg-indexer/src/models/values.rs`** - Add context fields to `ValueOp`:
   ```rust
   pub struct ValueOp {
       // ... existing fields
       pub context_root_id: Option<Uuid>,
       pub context_edge_type_id: Option<Uuid>,
   }
   ```

3. **`kg-indexer/src/models/relations.rs`** - Add context fields to `SetRelationItem`.

4. **`kg-indexer/src/storage.rs`** - Persist context columns in `insert_value_versions()` and `insert_relation_versions()`.

### Phase 3: API Versioned Diff Updates

**Files to modify:**

1. **`api/src/versioned/queries.ts`** - Include context in version queries:
   ```typescript
   // Query values with context for block attribution
   SELECT *, context_root_id, context_edge_type_id
   FROM value_versions
   WHERE entity_id = $1
     AND valid_from_key <= $2
     AND (valid_to_key IS NULL OR valid_to_key > $2)
   ```

2. **`api/src/versioned/diff.ts`** - Use context for grouping:
   - Changes with `context_edge_type_id = BLOCKS_ID` and `context_root_id = entityId` belong in `blocks[]`
   - More reliable than current approach of looking up BLOCKS relations

## Critical Files

| File | Change |
|------|--------|
| `api/drizzle/00XX_context_columns.sql` | New migration |
| `kg-indexer/src/handlers/edits.rs` | Extract context from ops |
| `kg-indexer/src/models/values.rs` | Add context fields to ValueOp |
| `kg-indexer/src/models/relations.rs` | Add context fields to SetRelationItem |
| `kg-indexer/src/storage.rs` | Persist context to DB |
| `api/src/versioned/queries.ts` | Query context for diffs |
| `api/src/versioned/diff.ts` | Use context for block grouping |

## Backward Compatibility

- Existing data has NULL context columns - handled gracefully
- Current diff behavior (BLOCKS relation lookup) continues to work
- Context-based grouping enhances accuracy when available
- No API response shape changes - `blocks[]` already exists

## Testing Strategy

1. Unit tests in kg-indexer for context extraction
2. Integration tests verifying context persists through indexing
3. API tests for diff grouping with/without context data
