# 0005: Persistence Implementation Plan

## Overview

Atlas needs to persist state to survive restarts without reprocessing the entire event stream. This document outlines the implementation plan for cursor, block number, and graph state persistence.

## Goals

1. **Resumability** - Atlas resumes from where it left off after restart
2. **Consistency** - Persisted state matches the last processed block
3. **Simplicity** - Use existing infrastructure patterns where possible

## What to Persist

| Data | Purpose | Size |
|------|---------|------|
| Cursor | Substream/Kafka position for resumption | ~100 bytes |
| Block Number | Last processed block for debugging/monitoring | 8 bytes |
| GraphState | Topology state for computing canonical graphs | ~1-2 MB for 10K spaces |

## Storage Options

### Option A: PostgreSQL (Recommended)

Use the existing `meta` table pattern from `api/src/services/storage/schema.ts`:

```sql
-- Existing schema
CREATE TABLE meta (
    id TEXT PRIMARY KEY,        -- e.g., "atlas"
    cursor TEXT NOT NULL,       -- Kafka consumer offset or substream cursor
    block_number TEXT NOT NULL  -- Last processed block
);
```

For GraphState, add a new table:

```sql
CREATE TABLE atlas_graph_state (
    id TEXT PRIMARY KEY,              -- e.g., "atlas" (single row)
    state BYTEA NOT NULL,             -- Serialized GraphState (bincode)
    updated_at TIMESTAMPTZ NOT NULL   -- For debugging
);
```

**Pros:**
- Consistent with existing indexers (`indexer`, `cache`, `actions-indexer-pipeline`)
- ACID transactions ensure cursor + state are updated atomically
- Existing infrastructure, no new dependencies

**Cons:**
- Requires PostgreSQL connection
- Slightly more latency than embedded storage

### Option B: Embedded Database (redb)

Single-file embedded key-value store:

```rust
let db = Database::create("atlas-state.redb")?;
let tx = db.begin_write()?;
{
    let mut table = tx.open_table(STATE_TABLE)?;
    table.insert("cursor", cursor.as_bytes())?;
    table.insert("block", block_number.to_le_bytes())?;
    table.insert("state", bincode::serialize(&state)?)?;
}
tx.commit()?;
```

**Pros:**
- No external dependencies
- Fast (no network round-trip)
- Self-contained deployment

**Cons:**
- New pattern for the codebase
- Additional dependency to maintain

### Recommendation

**Use PostgreSQL** for consistency with the rest of the codebase. Atlas can share the same database as other services or use its own instance.

## Implementation Steps

### Phase 1: Database Schema

| Step | Description |
|------|-------------|
| 1.1 | Add `atlas_graph_state` table migration to `api/drizzle/` |
| 1.2 | Update schema.ts with new table definition |
| 1.3 | Add sqlx queries for Rust (or use raw SQL) |

### Phase 2: Persistence Layer in Atlas

| Step | Description |
|------|-------------|
| 2.1 | Add `sqlx` dependency to Atlas |
| 2.2 | Create `Storage` trait for persistence operations |
| 2.3 | Implement `PostgresStorage` using sqlx |
| 2.4 | Add serialization for GraphState (serde + bincode) |

### Phase 3: Integration

| Step | Description |
|------|-------------|
| 3.1 | Load cursor + state on startup |
| 3.2 | Persist after each canonical graph emission |
| 3.3 | Handle startup with empty state (first run) |

## Persistence Strategy

### When to Persist

Persist **after every canonical graph change**, before emitting to Kafka:

```rust
// Pseudocode
for event in events {
    state.apply_event(&event);
    transitive.handle_event(&event, &state);
    
    if let Some(graph) = canonical.compute(&state, &mut transitive) {
        // Persist BEFORE emitting (crash-safe)
        storage.save(&cursor, block_number, &state).await?;
        
        // Now emit
        emitter.emit(&graph, &event.meta)?;
    }
}
```

This ensures:
- If crash before persist: Re-process block, re-emit (acceptable, same result)
- If crash after persist, before emit: Resume from cursor, no re-emit of this block
- If crash after emit: Resume from cursor, skip already-processed block

### Transaction Semantics

Cursor + block + state should be updated atomically:

```rust
impl PostgresStorage {
    async fn save(&self, cursor: &str, block: u64, state: &GraphState) -> Result<()> {
        let state_bytes = bincode::serialize(state)?;
        
        sqlx::query!(
            r#"
            INSERT INTO meta (id, cursor, block_number) 
            VALUES ($1, $2, $3)
            ON CONFLICT (id) DO UPDATE SET cursor = $2, block_number = $3;
            
            INSERT INTO atlas_graph_state (id, state, updated_at)
            VALUES ($1, $4, NOW())
            ON CONFLICT (id) DO UPDATE SET state = $4, updated_at = NOW();
            "#,
            "atlas",
            cursor,
            block.to_string(),
            state_bytes
        )
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
}
```

## GraphState Serialization

Add serde derives to GraphState and related types:

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GraphState {
    pub spaces: HashSet<SpaceId>,
    pub space_topics: HashMap<SpaceId, TopicId>,
    pub topic_spaces: HashMap<TopicId, HashSet<SpaceId>>,
    pub explicit_edges: HashMap<SpaceId, Vec<(SpaceId, EdgeType)>>,
    pub topic_edges: HashMap<SpaceId, HashSet<TopicId>>,
    pub topic_edge_sources: HashMap<TopicId, HashSet<SpaceId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeType {
    Root,
    Verified,
    Related,
    Topic,
}
```

Use bincode for compact binary serialization:

```rust
// Serialize
let bytes = bincode::serialize(&state)?;

// Deserialize
let state: GraphState = bincode::deserialize(&bytes)?;
```

## Startup Flow

```rust
async fn main() {
    let storage = PostgresStorage::connect(&database_url).await?;
    
    // Load persisted state (if any)
    let (cursor, state) = match storage.load().await? {
        Some((cursor, block, state)) => {
            println!("Resuming from block {}", block);
            (Some(cursor), state)
        }
        None => {
            println!("Starting fresh");
            (None, GraphState::new())
        }
    };
    
    // Initialize processors with loaded state
    let mut transitive = TransitiveProcessor::new();
    let mut canonical = CanonicalProcessor::new(root_space_id);
    
    // Start consuming from cursor position
    let consumer = KafkaConsumer::new(&broker, cursor)?;
    
    // Process events...
}
```

## Dependencies

Add to `atlas/Cargo.toml`:

```toml
[dependencies]
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres"] }
serde = { version = "1", features = ["derive"] }
bincode = "1"
```

## Testing

| Test | Description |
|------|-------------|
| Unit | Serialization round-trip for GraphState |
| Unit | Storage save/load operations |
| Integration | Restart recovery with persisted state |
| Integration | First-run with empty database |

## Future Considerations

- **Snapshots**: For very large graphs, consider periodic full snapshots vs incremental updates
- **Compression**: Compress serialized state if size becomes an issue
- **Multiple roots**: If Atlas tracks multiple root spaces, partition state by root

## Related Documents

- [0000: Atlas Implementation Roadmap](./0000-atlas-implementation-roadmap.md)
- [Known Issues](../known-issues.md) - Event ordering on reprocess
