# 0002: Add EDITS_PUBLISHED Support to Hermes Spaces

## Status

Proposed

## Context

The original `hermes-pipeline` design (see `0001-complete-action-support.md`) excluded `EDITS_PUBLISHED` events, with the assumption that a dedicated `hermes-edits` transformer would handle them separately. This was based on the principle of specialized transformers.

However, after further consideration, consolidating edit processing into `hermes-pipeline` provides several benefits:
- **Simpler deployment**: One binary handles all space-related events
- **Shared infrastructure**: Reuses existing Kafka producer, cursor management
- **Consistent patterns**: Same conversion/emit patterns as other actions

The key challenge is that `EDITS_PUBLISHED` events contain only an IPFS CID - the actual edit content must be resolved from an IPFS cache. For mock mode, we need to create a mock cache that maps the IPFS hashes emitted by `hermes-relay`'s test topology to `Edit` protos.

### Current Mock Relay Edits

The mock relay (`hermes-relay/src/source/mock_events.rs:309-315`) emits 6 edit events:

```rust
actions.push(edit_published(ROOT_SPACE_ID, "QmRootEdit1CreatePersons"));
actions.push(edit_published(ROOT_SPACE_ID, "QmRootEdit2AddDescriptions"));
actions.push(edit_published(SPACE_A, "QmSpaceAEdit1CreateOrg"));
actions.push(edit_published(SPACE_A, "QmSpaceAEdit2CreateRelations"));
actions.push(edit_published(SPACE_B, "QmSpaceBEdit1CreateDoc"));
actions.push(edit_published(SPACE_C, "QmSpaceCEdit1CreateTopic"));
```

Each IPFS hash needs corresponding mock `Edit` content.

## Decision

Add `EDITS_PUBLISHED` support to `hermes-pipeline` as another data pipeline.

### Data Pipelines

Each pipeline converts Actions into Kafka events:

```
Action ──────────────────────────────────────────────────────▶ Kafka Event

SPACE_REGISTERED ──[ convert ]──────────────────────────────▶ space.creations
SUBSPACE_ADDED   ──[ convert ]──────────────────────────────▶ space.trust.extensions  
SUBSPACE_REMOVED ──[ convert ]──────────────────────────────▶ space.trust.extensions
EDITS_PUBLISHED  ──[ convert + IPFS cache lookup ]──────────▶ knowledge.edits
```

Most pipelines are simple: extract fields from Action, build proto, emit to Kafka.

The **edits pipeline** is the exception - it requires an external lookup to resolve the IPFS hash to actual edit content before emitting.

### Ordering Requirements

- **Space/Trust events**: Must preserve substream order
- **Edit events**: Must be ordered relative to other edits (edits are diffs)

For now, we process all actions sequentially in one binary. The cache lookup for edits is blocking but fast (in-memory for mock, database query for live).

If cache lookups become a bottleneck, we can later:
- **Option A**: Break out edits into a separate `hermes-edits` binary with its own cursor
- **Option B**: Batch cache lookups per block, then emit in order

## Implementation Plan

### Architecture: Pipelines

Organize pipelines into separate modules. Each pipeline is a simple function:

```
hermes-pipeline/src/
├── main.rs                      # Runs all pipelines per action
├── pipelines/
│   ├── mod.rs                   # Re-exports all pipelines
│   ├── spaces.rs                # SPACE_REGISTERED → space.creations
│   ├── trust.rs                 # SUBSPACE_ADDED/REMOVED → space.trust.extensions
│   └── edits.rs                 # EDITS_PUBLISHED → knowledge.edits (+ cache)
├── cache/
│   ├── mod.rs                   # Cache trait
│   └── mock.rs                  # Mock IPFS cache for testing
└── shared.rs                    # Shared utilities (block metadata conversion)
```

### Phase 1: Restructure into Pipelines

Each pipeline is a simple module with a `process` function:

**File:** `hermes-pipeline/src/pipelines/spaces.rs`
```rust
//! Pipeline: SPACE_REGISTERED → space.creations

use anyhow::Result;
use hermes_relay::{actions, Action};
use hermes_kafka::producer::KafkaProducer;
use hermes_schema::pb::space::HermesCreateSpace;
use crate::shared::BlockMetadata;

const TOPIC: &str = "space.creations";

/// Process SPACE_REGISTERED action. Returns Ok(()) if not applicable.
pub fn process(
    action: &Action,
    meta: &BlockMetadata,
    producer: &KafkaProducer,
) -> Result<()> {
    if !actions::matches(&action.action, &actions::SPACE_REGISTERED) {
        return Ok(());
    }

    let event = convert(action, meta)?;
    producer.send(TOPIC, &action.from_id, &event)?;
    Ok(())
}

fn convert(action: &Action, meta: &BlockMetadata) -> Result<HermesCreateSpace> {
    // ... existing conversion logic
}
```

**File:** `hermes-pipeline/src/pipelines/trust.rs`
```rust
//! Pipeline: SUBSPACE_ADDED/REMOVED → space.trust.extensions

use anyhow::Result;
use hermes_relay::{actions, Action};
use hermes_kafka::producer::KafkaProducer;
use hermes_schema::pb::space::HermesSpaceTrustExtension;
use crate::shared::BlockMetadata;

const TOPIC: &str = "space.trust.extensions";

/// Process SUBSPACE_ADDED or SUBSPACE_REMOVED action.
pub fn process(
    action: &Action,
    meta: &BlockMetadata,
    producer: &KafkaProducer,
) -> Result<()> {
    let action_type = &action.action;

    if actions::matches(action_type, &actions::SUBSPACE_ADDED) {
        let event = convert_added(action, meta)?;
        producer.send(TOPIC, &action.from_id, &event)?;
    } else if actions::matches(action_type, &actions::SUBSPACE_REMOVED) {
        let event = convert_removed(action, meta)?;
        producer.send(TOPIC, &action.from_id, &event)?;
    }

    Ok(())
}

fn convert_added(action: &Action, meta: &BlockMetadata) -> Result<HermesSpaceTrustExtension> {
    // ... existing conversion logic
}

fn convert_removed(action: &Action, meta: &BlockMetadata) -> Result<HermesSpaceTrustExtension> {
    // ... existing conversion logic
}
```

### Phase 2: Add Edits Pipeline

**File:** `hermes-pipeline/src/pipelines/edits.rs`
```rust
//! Pipeline: EDITS_PUBLISHED → knowledge.edits
//!
//! Unlike other pipelines, this requires an external cache lookup
//! to resolve IPFS hash → Edit content.

use anyhow::Result;
use hermes_relay::{actions, Action};
use hermes_kafka::producer::KafkaProducer;
use hermes_schema::pb::knowledge::HermesEdit;
use wire::pb::grc20::Edit;
use crate::cache::IpfsCache;
use crate::shared::BlockMetadata;

const TOPIC: &str = "knowledge.edits";

/// Process EDITS_PUBLISHED action with cache lookup.
pub fn process<C: IpfsCache>(
    action: &Action,
    meta: &BlockMetadata,
    producer: &KafkaProducer,
    cache: &C,
) -> Result<()> {
    if !actions::matches(&action.action, &actions::EDITS_PUBLISHED) {
        return Ok(());
    }

    let ipfs_hash = String::from_utf8_lossy(&action.data);

    if let Some(cached_edit) = cache.get(&ipfs_hash) {
        let event = convert(action, cached_edit, meta)?;
        producer.send(TOPIC, &event.space_id, &event)?;
    }

    Ok(())
}

fn convert(action: &Action, edit: &Edit, meta: &BlockMetadata) -> Result<HermesEdit> {
    Ok(HermesEdit {
        id: edit.id.clone(),
        name: edit.name.clone(),
        ops: edit.ops.clone(),
        authors: edit.authors.clone(),
        language: edit.language.clone(),
        space_id: hex::encode(&action.from_id),
        is_canonical: true,
        meta: Some(meta.into()),
    })
}
```

### Phase 3: Add Cache Module

**File:** `hermes-pipeline/src/cache/mod.rs`
```rust
mod mock;
pub use mock::MockCache;

use wire::pb::grc20::Edit;

/// Trait for IPFS cache implementations
pub trait IpfsCache {
    fn get(&self, ipfs_hash: &str) -> Option<&Edit>;
}
```

**File:** `hermes-pipeline/src/cache/mock.rs`
```rust
//! Mock IPFS cache for development/testing

use std::collections::HashMap;
use wire::pb::grc20::Edit;
use super::IpfsCache;

pub struct MockCache {
    edits: HashMap<String, Edit>,
}

impl MockCache {
    pub fn new() -> Self {
        let mut cache = Self { edits: HashMap::new() };
        cache.populate();
        cache
    }

    fn populate(&mut self) {
        self.edits.insert("QmRootEdit1CreatePersons".into(), create_persons_edit());
        self.edits.insert("QmRootEdit2AddDescriptions".into(), create_descriptions_edit());
        self.edits.insert("QmSpaceAEdit1CreateOrg".into(), create_org_edit());
        self.edits.insert("QmSpaceAEdit2CreateRelations".into(), create_relations_edit());
        self.edits.insert("QmSpaceBEdit1CreateDoc".into(), create_doc_edit());
        self.edits.insert("QmSpaceCEdit1CreateTopic".into(), create_topic_edit());
    }
}

impl IpfsCache for MockCache {
    fn get(&self, ipfs_hash: &str) -> Option<&Edit> {
        self.edits.get(ipfs_hash)
    }
}

fn create_persons_edit() -> Edit { /* ... */ }
fn create_descriptions_edit() -> Edit { /* ... */ }
fn create_org_edit() -> Edit { /* ... */ }
fn create_relations_edit() -> Edit { /* ... */ }
fn create_doc_edit() -> Edit { /* ... */ }
fn create_topic_edit() -> Edit { /* ... */ }
```

### Phase 4: Update Main

**File:** `hermes-pipeline/src/main.rs`
```rust
mod cache;
mod pipelines;
mod shared;

use anyhow::Result;
use hermes_kafka::producer::KafkaProducer;
use hermes_relay::{Actions, Sink, StreamSource};
use prost::Message;

use cache::MockCache;

#[tokio::main]
async fn main() -> Result<()> {
    let producer = KafkaProducer::new("localhost:9092")?;
    let cache = MockCache::new();

    let sink = |data| {
        let actions = Actions::decode(data)?;
        let meta = extract_block_metadata(data);

        for action in &actions.actions {
            pipelines::spaces::process(action, &meta, &producer)?;
            pipelines::trust::process(action, &meta, &producer)?;
            pipelines::edits::process(action, &meta, &producer, &cache)?;
        }

        Ok(())
    };

    StreamSource::mock().run(sink).await
}
```

### Phase 6: Add Dependencies

**File:** `hermes-pipeline/Cargo.toml`
```toml
[dependencies]
wire = { path = "../wire" }
# ... existing dependencies
```

## File Changes Summary

| File | Action | Description |
|------|--------|-------------|
| `hermes-pipeline/Cargo.toml` | Modify | Add `wire` dependency |
| `hermes-pipeline/src/main.rs` | Modify | Run all pipelines sequentially |
| `hermes-pipeline/src/pipelines/mod.rs` | Create | Re-export pipelines |
| `hermes-pipeline/src/pipelines/spaces.rs` | Create | SPACE_REGISTERED pipeline |
| `hermes-pipeline/src/pipelines/trust.rs` | Create | SUBSPACE_ADDED/REMOVED pipeline |
| `hermes-pipeline/src/pipelines/edits.rs` | Create | EDITS_PUBLISHED pipeline |
| `hermes-pipeline/src/cache/mod.rs` | Create | IpfsCache trait |
| `hermes-pipeline/src/cache/mock.rs` | Create | Mock cache with test edits |
| `hermes-pipeline/src/shared.rs` | Create | BlockMetadata conversion |
| `hermes-pipeline/src/conversion.rs` | Delete | Moved to pipelines/ |
| `hermes-pipeline/src/kafka.rs` | Delete | Inlined in pipelines/ |
| `hermes-pipeline/src/transformer.rs` | Delete | Replaced by pipelines/ |

## Well-Known IDs for Mock Edits

To maintain consistency with hermes-relay's test topology, use these IDs:

```rust
// Entity IDs (matching hermes-relay test topology pattern)
const ENTITY_PERSON_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xF1];
const ENTITY_PERSON_2: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xF2];
const ENTITY_ORG_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xF3];
const ENTITY_PROJECT_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xF4];
const ENTITY_DOC_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xF5];
const ENTITY_TOPIC_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xF6];

// Property IDs
const PROPERTY_NAME: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xD1];
const PROPERTY_DESCRIPTION: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xD2];
const PROPERTY_URL: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xD3];

// Relation Type IDs
const RELATION_TYPE_BELONGS_TO: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xC2];
const RELATION_TYPE_RELATED_TO: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xC3];

// Edit IDs
const EDIT_ROOT_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xE1];
const EDIT_ROOT_2: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xE2];
const EDIT_A_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xEA];
const EDIT_A_2: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xEB];
const EDIT_B_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xEC];
const EDIT_C_1: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xED];

// Space IDs (for cross-space relations)
const SPACE_A: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0A];
const SPACE_B: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0B];
const SPACE_C: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0C];
```

## Cache Error Handling

When reading from the IPFS cache (especially the live cache), several error conditions can occur. See `indexer/src/preprocess.rs` and `indexer/src/cache/` for reference implementation.

### Error Types

1. **Cache Miss (NotFound)**: The IPFS hash doesn't exist in the cache yet
   - The cache service runs ahead, but timing issues can cause the transformer to catch up
   - **Handling**: Retry with exponential backoff (the indexer uses 10ms base, 2x factor, 5s max)

2. **Errored Entries (`is_errored: true`)**: The cache tried to fetch from IPFS but failed
   - Could be invalid CID, IPFS gateway timeout, content not available
   - **Handling**: Log warning, skip the edit (don't emit to Kafka)

3. **Missing Edit Content (`edit: None`)**: Entry exists but content couldn't be decoded
   - **Handling**: Log warning, skip the edit

4. **Database Errors**: Connection issues, query failures
   - **Handling**: Retry with backoff, eventually fail the block processing

5. **Deserialization Errors**: Cached JSON can't be parsed into `Edit` proto
   - **Handling**: Log error, skip the edit

### Cache Result Type

For live cache integration, use a result type that captures these states:

```rust
pub struct CachedEdit {
    pub cid: String,
    pub edit: Option<Edit>,
    pub is_errored: bool,
    pub space_id: Vec<u8>,
}

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache entry not found")]
    NotFound,
    
    #[error("Database error: {0}")]
    Database(String),
    
    #[error("Deserialization error: {0}")]
    DeserializeError(String),
}
```

### Retry Strategy (for live cache)

```rust
use tokio_retry::{strategy::{jitter, ExponentialBackoff}, Retry};

let retry = ExponentialBackoff::from_millis(10)
    .factor(2)
    .max_delay(std::time::Duration::from_secs(5))
    .map(jitter);

match Retry::spawn(retry, || cache.get(&ipfs_hash)).await {
    Ok(cached) => {
        if cached.is_errored {
            warn!("Cached edit entry is errored: {}", ipfs_hash);
            continue;  // Skip this edit
        }
        if let Some(edit) = cached.edit {
            // Process the edit
        }
    }
    Err(e) => {
        warn!("Failed to fetch edit from cache after retries: {}", e);
        // Either skip or fail depending on policy
    }
}
```

### Mock Cache Simplification

For the mock cache, we can simplify since all entries are pre-populated and never errored:

```rust
impl MockIpfsCache {
    pub fn get(&self, ipfs_hash: &str) -> Option<&Edit> {
        self.edits.get(ipfs_hash)
    }
}
```

The mock cache returns `Option<&Edit>` - `None` means cache miss (shouldn't happen with correct test data), `Some` means success.

## Future Work

### Live IPFS Cache Integration

When ready for production, the `MockIpfsCache` can be replaced with a trait-based abstraction:

```rust
#[async_trait]
pub trait IpfsCache: Send + Sync {
    async fn get(&self, ipfs_hash: &str) -> Result<CachedEdit, CacheError>;
}

// MockIpfsCache implements IpfsCache (sync lookup, wrapped in async)
// LiveIpfsCache implements IpfsCache (from hermes-ipfs-cache PostgreSQL)
```

This will require:
1. Adding `hermes-ipfs-cache` as a dependency
2. Creating a cache source configuration (mock vs live)
3. Making the transformer generic over `IpfsCache`
4. Adding retry logic with exponential backoff
5. Handling errored entries and deserialization failures

### Architecture Doc Update

After implementation, update `docs/hermes-architecture.md` to:
1. Remove the separate `edits` binary from the architecture
2. Show edits processing happening in `hermes-pipeline`
3. Update the diagram to show IPFS cache integration within hermes-pipeline

## Consequences

### Positive

- **Unified transformer**: All space-related events in one binary
- **Simpler operations**: One deployment, one set of cursors
- **Consistent testing**: Mock cache uses same patterns as mock relay

### Negative

- **Larger binary**: More code in hermes-pipeline
- **Cache dependency**: Requires IPFS cache (mock or live) at runtime

### Neutral

- **Same output**: Emits to same `knowledge.edits` topic as planned
- **Same message format**: Uses existing `HermesEdit` proto

## References

- `hermes-relay/src/source/mock_events.rs` - Mock edit events source
- `hermes-processor/src/main.rs` - Reference implementation for edit conversion
- `hermes-ipfs-cache/src/cache.rs` - Live cache implementation (future)
- `docs/hermes-architecture.md` - Overall system architecture
- `0001-complete-action-support.md` - Original action support plan
