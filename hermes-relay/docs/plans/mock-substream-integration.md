# Mock Substream Integration for hermes-relay

## Status

Proposed

## Problem

We need to develop and test hermes transformers and downstream architecture (Kafka consumers, indexers) without waiting for upstream smart contract dependencies. The current `hermes-relay` implementation is tightly coupled to a real substream connection, making it impossible to run the full pipeline with mock data.

## Goals

1. Enable running hermes transformers with mock blockchain events
2. Support deterministic integration testing across the entire pipeline
3. Allow parallel development while waiting for smart contract deployment
4. Maintain compatibility with the production substream path

## Architecture Overview

```
Production Path:
┌──────────────┐     ┌─────────────────┐     ┌────────────────┐     ┌───────┐
│  Blockchain  │────▶│ hermes-substream│────▶│  hermes-relay  │────▶│ Kafka │
└──────────────┘     └─────────────────┘     └────────────────┘     └───────┘
                                                     │
                                              ┌──────▼──────┐
                                              │  IPFS Cache │
                                              │ (PostgreSQL)│
                                              └─────────────┘

Mock Path:
┌──────────────┐     ┌────────────────┐     ┌───────┐
│mock-substream│────▶│  hermes-relay  │────▶│ Kafka │
└──────────────┘     └────────────────┘     └───────┘
                             │
                      ┌──────▼──────┐
                      │  Mock IPFS  │
                      │   Cache     │
                      └─────────────┘
```

## Design

### Option 1: BlockSource Trait Abstraction (Recommended)

Introduce a `BlockSource` trait that abstracts over the event source, allowing both real substreams and mock data to be consumed through the same interface.

#### New Types

```rust
// hermes-relay/src/source.rs

/// A block of data from any source (real or mock).
pub struct BlockData {
    pub block_number: u64,
    pub timestamp: u64,
    pub cursor: String,
    /// Module output data (protobuf-encoded).
    pub output: Vec<u8>,
    /// Module name that produced this output.
    pub module_name: String,
}

/// Signal to undo blocks due to chain reorganization.
pub struct UndoSignal {
    pub last_valid_block: u64,
    pub last_valid_cursor: String,
}

/// Response from a block source.
pub enum BlockResponse {
    New(BlockData),
    Undo(UndoSignal),
}

/// Trait for consuming blocks from any source.
#[async_trait]
pub trait BlockSource: Send {
    /// Get the next block response, or None if the stream is exhausted.
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>>;
    
    /// Get the current cursor position.
    fn cursor(&self) -> Option<&str>;
}
```

#### Production Implementation

Wraps the existing `SubstreamsStream`:

```rust
// hermes-relay/src/source/substream.rs

pub struct SubstreamSource {
    stream: SubstreamsStream,
    current_cursor: Option<String>,
}

impl SubstreamSource {
    pub async fn connect(
        endpoint_url: &str,
        module: HermesModule,
        cursor: Option<String>,
        start_block: i64,
        end_block: u64,
    ) -> Result<Self, anyhow::Error> {
        let token = env::var("SUBSTREAMS_API_TOKEN").ok();
        let package = stream::read_package(HERMES_SPKG).await?;
        let endpoint = Arc::new(SubstreamsEndpoint::new(endpoint_url, token).await?);
        
        let stream = SubstreamsStream::new(
            endpoint,
            cursor.clone(),
            package.modules.clone(),
            module.to_string(),
            start_block,
            end_block,
        );
        
        Ok(Self {
            stream,
            current_cursor: cursor,
        })
    }
}

#[async_trait]
impl BlockSource for SubstreamSource {
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>> {
        match self.stream.next().await {
            None => None,
            Some(Ok(stream::BlockResponse::New(data))) => {
                self.current_cursor = Some(data.cursor.clone());
                Some(Ok(BlockResponse::New(BlockData {
                    block_number: data.clock.as_ref().unwrap().number,
                    timestamp: data.clock.as_ref().unwrap().timestamp.as_ref().unwrap().seconds as u64,
                    cursor: data.cursor,
                    output: data.output.first().map(|o| o.map_output.clone()).unwrap_or_default(),
                    module_name: data.output.first().map(|o| o.name.clone()).unwrap_or_default(),
                })))
            }
            Some(Ok(stream::BlockResponse::Undo(signal))) => {
                self.current_cursor = Some(signal.last_valid_cursor.clone());
                Some(Ok(BlockResponse::Undo(UndoSignal {
                    last_valid_block: signal.last_valid_block.as_ref().unwrap().number,
                    last_valid_cursor: signal.last_valid_cursor,
                })))
            }
            Some(Err(e)) => Some(Err(e)),
        }
    }
    
    fn cursor(&self) -> Option<&str> {
        self.current_cursor.as_deref()
    }
}
```

#### Mock Implementation

Converts `mock_substream::MockBlock` events to the relay's expected format:

```rust
// hermes-relay/src/source/mock.rs

use mock_substream::{MockBlock, MockEvent};
use prost::Message;

pub struct MockSource {
    blocks: VecDeque<MockBlock>,
    current_cursor: Option<String>,
    module: HermesModule,
}

impl MockSource {
    /// Create a mock source from pre-generated blocks.
    pub fn new(blocks: Vec<MockBlock>, module: HermesModule) -> Self {
        Self {
            blocks: blocks.into(),
            current_cursor: None,
            module,
        }
    }
    
    /// Create a mock source with the deterministic test topology.
    pub fn deterministic(module: HermesModule) -> Self {
        Self::new(mock_substream::test_topology::generate(), module)
    }
    
    /// Convert mock events to protobuf-encoded output based on the module.
    fn encode_events(&self, events: &[MockEvent]) -> Vec<u8> {
        match self.module {
            HermesModule::Actions => {
                // Encode all events as Actions
                let actions = events.iter().filter_map(|e| self.event_to_action(e)).collect();
                let msg = hermes_substream::pb::hermes::Actions { actions };
                msg.encode_to_vec()
            }
            HermesModule::SpacesRegistered => {
                // Filter and encode only space registration events
                let spaces: Vec<_> = events.iter().filter_map(|e| match e {
                    MockEvent::SpaceCreated(s) => Some(self.space_created_to_proto(s)),
                    _ => None,
                }).collect();
                // Encode as SpacesRegistered proto...
                vec![]
            }
            HermesModule::EditsPublished => {
                // Filter and encode only edit events
                let edits: Vec<_> = events.iter().filter_map(|e| match e {
                    MockEvent::EditPublished(e) => Some(self.edit_to_proto(e)),
                    _ => None,
                }).collect();
                // Encode as EditsPublished proto...
                vec![]
            }
            // ... other modules
            _ => vec![],
        }
    }
    
    fn event_to_action(&self, event: &MockEvent) -> Option<hermes_substream::pb::hermes::Action> {
        // Convert MockEvent to raw Action based on event type
        match event {
            MockEvent::SpaceCreated(s) => Some(hermes_substream::pb::hermes::Action {
                from_id: s.space_id.to_vec(),
                to_id: s.space_id.to_vec(),
                action: crate::actions::SPACE_REGISTERED.to_vec(),
                topic: s.topic_id.to_vec(),
                data: self.encode_space_data(s),
            }),
            MockEvent::TrustExtended(t) => {
                let (action_type, topic, data) = match &t.extension {
                    TrustExtension::Verified { target_space_id } => (
                        crate::actions::SUBSPACE_ADDED.to_vec(),
                        target_space_id.to_vec(),
                        vec![],
                    ),
                    // ... handle other extension types
                    _ => return None,
                };
                Some(hermes_substream::pb::hermes::Action {
                    from_id: t.source_space_id.to_vec(),
                    to_id: vec![], // depends on extension type
                    action: action_type,
                    topic,
                    data,
                })
            }
            MockEvent::EditPublished(e) => Some(hermes_substream::pb::hermes::Action {
                from_id: e.space_id.to_vec(),
                to_id: e.space_id.to_vec(),
                action: crate::actions::EDITS_PUBLISHED.to_vec(),
                topic: vec![],
                data: self.encode_edit_data(e),
            }),
        }
    }
}

#[async_trait]
impl BlockSource for MockSource {
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>> {
        let block = self.blocks.pop_front()?;
        self.current_cursor = Some(block.cursor.clone());
        
        Some(Ok(BlockResponse::New(BlockData {
            block_number: block.number,
            timestamp: block.timestamp,
            cursor: block.cursor,
            output: self.encode_events(&block.events),
            module_name: self.module.to_string(),
        })))
    }
    
    fn cursor(&self) -> Option<&str> {
        self.current_cursor.as_deref()
    }
}
```

#### Updated Sink Trait

Add a method to run with any `BlockSource`:

```rust
// hermes-relay/src/sink.rs

pub trait Sink: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    // ... existing methods ...

    /// Run the sink with a custom block source.
    fn run_with_source<S: BlockSource>(
        &self,
        source: S,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send {
        async move {
            let mut source = source;
            
            loop {
                match source.next().await {
                    None => {
                        println!("Stream consumed");
                        break;
                    }
                    Some(Ok(BlockResponse::New(data))) => {
                        // Convert BlockData to BlockScopedData for compatibility
                        let block_scoped_data = self.block_data_to_scoped(&data)?;
                        self.process_block_scoped_data(&block_scoped_data).await?;
                        self.persist_cursor(data.cursor, data.block_number).await?;
                    }
                    Some(Ok(BlockResponse::Undo(signal))) => {
                        // Handle undo...
                    }
                    Some(Err(err)) => {
                        println!("Stream terminated with error: {:?}", err);
                        std::process::exit(1);
                    }
                }
            }
            
            Ok(())
        }
    }
    
    /// Convenience method to run with mock data.
    #[cfg(feature = "mock")]
    fn run_mock(
        &self,
        blocks: Vec<mock_substream::MockBlock>,
        module: HermesModule,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send {
        self.run_with_source(MockSource::new(blocks, module))
    }
}
```

### Option 2: Feature-Flagged Mock Mode

Instead of abstracting the source, use feature flags to swap implementations at compile time.

```rust
// hermes-relay/src/sink.rs

impl Sink {
    #[cfg(not(feature = "mock"))]
    fn run(...) {
        // Production implementation using SubstreamsStream
    }
    
    #[cfg(feature = "mock")]
    fn run(...) {
        // Mock implementation using MockSource
    }
}
```

**Pros:** Simpler, no runtime overhead
**Cons:** Can't switch at runtime, harder to test both paths

### Option 3: Environment-Based Switching

Use environment variables to switch between real and mock sources.

```rust
if env::var("HERMES_MOCK_MODE").is_ok() {
    // Use MockSource
} else {
    // Use SubstreamSource
}
```

**Pros:** Easy runtime switching
**Cons:** Magic environment variables, harder to configure in tests

## Recommended Approach

**Option 1 (BlockSource Trait)** is recommended because:

1. **Type-safe:** The trait ensures both implementations conform to the same contract
2. **Testable:** Easy to inject mock sources in tests
3. **Flexible:** Can add new sources (file-based replay, network replay) without changing consumers
4. **Explicit:** No magic environment variables or compile-time flags

## Implementation Plan

### Phase 1: Core Abstraction

1. Add `hermes-relay/src/source.rs` with `BlockSource` trait and types
2. Add `hermes-relay/src/source/substream.rs` wrapping existing `SubstreamsStream`
3. Add `Sink::run_with_source()` method
4. Keep existing `Sink::run()` as convenience for production use

### Phase 2: Mock Source

1. Add `mock` feature to `hermes-relay/Cargo.toml`
2. Add `hermes-relay/src/source/mock.rs` with `MockSource`
3. Implement event-to-protobuf conversion for each `HermesModule`
4. Add `Sink::run_mock()` convenience method

### Phase 3: Integration

1. Update `hermes-spaces` to support mock mode
2. Update `atlas` to support mock mode  
3. Add integration tests using deterministic topology
4. Document usage in transformer READMEs

## Event Conversion Details

The key challenge is converting `mock_substream::MockEvent` to the protobuf format expected by transformers. This requires:

### Space Events

```rust
MockEvent::SpaceCreated(space) -> hermes_substream::pb::hermes::SpaceRegistered {
    space_id: space.space_id,
    space_address: derive_address(space.space_id),
    // data field contains governance DAO info or personal space owner
}
```

### Trust Events

```rust
MockEvent::TrustExtended(trust) -> hermes_substream::pb::hermes::Action {
    // Maps to SUBSPACE_ADDED, SUBSPACE_REMOVED based on extension type
    // For topic edges, maps to appropriate topic action
}
```

### Edit Events

```rust
MockEvent::EditPublished(edit) -> hermes_substream::pb::hermes::EditsPublished {
    space_id: edit.space_id,
    // data field contains IPFS CID pointing to Edit proto
}
```

## IPFS Content Generation

The edits transformer relies on the IPFS cache to resolve edit content. In production:

1. `hermes-ipfs-cache` subscribes to `EditsPublished` events
2. For each event, it fetches content from IPFS by CID
3. The IPFS content is a `grc20.Edit` proto (defined in `wire/proto/grc20.proto`):
   ```protobuf
   message Edit {
     bytes id = 1;
     string name = 2;
     repeated Op ops = 3;
     repeated bytes authors = 4;
     optional bytes language = 5;
   }
   ```
4. Stores the decoded `Edit` proto in PostgreSQL
5. Edits transformer reads from cache, enriches with blockchain/topology metadata, and emits `HermesEdit` to Kafka

For mock mode, we don't want to mock the cache itself - the real cache infrastructure should work as normal. Instead, we need to **pre-populate the cache with real `grc20.Edit` data** that corresponds to our mock `EditsPublished` events.

### Approach

1. Generate mock `EditsPublished` events with deterministic CIDs
2. Convert `mock_substream::EditPublished` to real `grc20.Edit` protos
3. Pre-populate the IPFS cache (PostgreSQL) with these edits before running tests
4. The edits transformer reads from the cache as normal

This tests the full production path - the only thing mocked is the source of the blockchain events and the IPFS content (which is real GRC-20 data, just not fetched from IPFS).

### Cache Seeding

Add a utility to seed the cache with mock edit data:

```rust
// mock-substream/src/cache_seeder.rs

use hermes_ipfs_cache::cache::{Cache, CacheItem};
use wire::pb::grc20;

/// Seeds the IPFS cache with real GRC-20 edit data from mock events.
pub struct CacheSeeder {
    cache: Cache,
}

impl CacheSeeder {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
    
    /// Seed the cache with all edit events from the mock topology.
    ///
    /// For each `EditPublished` event, converts it to a real `grc20.Edit`
    /// proto and stores it in the cache with a deterministic CID.
    pub async fn seed_from_topology(&self, blocks: &[MockBlock]) -> Result<(), CacheError> {
        for block in blocks {
            for event in &block.events {
                if let MockEvent::EditPublished(edit) = event {
                    let cid = Self::deterministic_cid(&edit.edit_id);
                    let wire_edit = Self::to_grc20_edit(edit);
                    
                    let item = CacheItem {
                        uri: format!("ipfs://{}", cid),
                        json: Some(wire_edit),
                        block: block.timestamp.to_string(),
                        space_id: hex::encode(&edit.space_id),
                        is_errored: false,
                    };
                    
                    self.cache.put(&item).await?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Generate a deterministic CID from an edit ID.
    ///
    /// In production, CIDs are content-addressed hashes. For testing,
    /// we use a simple encoding of the edit ID that both the mock source
    /// and cache seeder agree on.
    pub fn deterministic_cid(edit_id: &EditId) -> String {
        // Use a fake but valid-looking CID format
        format!("Qm{}", hex::encode(edit_id))
    }
    
    /// Convert mock edit to real `grc20.Edit` proto.
    ///
    /// This produces the exact same structure that would be stored in IPFS
    /// and fetched by the production IPFS cache service.
    fn to_grc20_edit(edit: &EditPublished) -> grc20::Edit {
        grc20::Edit {
            id: edit.edit_id.to_vec(),
            name: edit.name.clone(),
            ops: edit.ops.iter().map(Self::to_grc20_op).collect(),
            authors: edit.authors.iter().map(|a| a.to_vec()).collect(),
            language: None,
        }
    }
    
    /// Convert mock op to `grc20.Op` proto.
    fn to_grc20_op(op: &mock_substream::Op) -> grc20::Op {
        use grc20::op::Payload;
        
        let payload = match op {
            mock_substream::Op::UpdateEntity(u) => {
                Payload::UpdateEntity(grc20::Entity {
                    id: u.id.to_vec(),
                    values: u.values.iter().map(|v| grc20::Value {
                        property: v.property.to_vec(),
                        value: v.value.clone(),
                        options: None,
                    }).collect(),
                })
            }
            mock_substream::Op::CreateRelation(r) => {
                Payload::CreateRelation(grc20::Relation {
                    id: r.id.to_vec(),
                    r#type: r.relation_type.to_vec(),
                    from_entity: r.from_entity.to_vec(),
                    from_space: r.from_space.map(|s| s.to_vec()),
                    from_version: None,
                    to_entity: r.to_entity.to_vec(),
                    to_space: r.to_space.map(|s| s.to_vec()),
                    to_version: None,
                    entity: r.entity.to_vec(),
                    position: r.position.clone(),
                    verified: r.verified,
                })
            }
            mock_substream::Op::UpdateRelation(r) => {
                Payload::UpdateRelation(grc20::RelationUpdate {
                    id: r.id.to_vec(),
                    from_space: r.from_space.map(|s| s.to_vec()),
                    from_version: None,
                    to_space: r.to_space.map(|s| s.to_vec()),
                    to_version: None,
                    position: r.position.clone(),
                    verified: r.verified,
                })
            }
            mock_substream::Op::DeleteRelation(id) => {
                Payload::DeleteRelation(id.to_vec())
            }
            mock_substream::Op::CreateProperty(p) => {
                Payload::CreateProperty(grc20::Property {
                    id: p.id.to_vec(),
                    data_type: p.data_type as i32,
                })
            }
            mock_substream::Op::UnsetEntityValues(u) => {
                Payload::UnsetEntityValues(grc20::UnsetEntityValues {
                    id: u.id.to_vec(),
                    properties: u.properties.iter().map(|p| p.to_vec()).collect(),
                })
            }
            mock_substream::Op::UnsetRelationFields(u) => {
                Payload::UnsetRelationFields(grc20::UnsetRelationFields {
                    id: u.id.to_vec(),
                    from_space: u.from_space,
                    from_version: None,
                    to_space: u.to_space,
                    to_version: None,
                    position: u.position,
                    verified: u.verified,
                })
            }
        };
        
        grc20::Op { payload: Some(payload) }
    }
}
```

### MockSource CID Generation

The `MockSource` must generate the same CIDs that the cache seeder uses:

```rust
// hermes-relay/src/source/mock.rs

impl MockSource {
    /// Encode edit event with CID matching what's in the seeded cache.
    fn encode_edit_data(&self, edit: &EditPublished) -> Vec<u8> {
        // Must match CacheSeeder::deterministic_cid()
        let cid = format!("Qm{}", hex::encode(&edit.edit_id));
        format!("ipfs://{}", cid).into_bytes()
    }
}
```

### Test Setup

```rust
#[tokio::test]
async fn test_edits_transformer_with_seeded_cache() {
    // Setup: real PostgreSQL cache (can use testcontainers)
    let cache = Cache::new(Storage::new().await.unwrap());
    
    // Generate deterministic topology
    let blocks = mock_substream::test_topology::generate();
    
    // Seed the cache with real GRC-20 edit data
    let seeder = CacheSeeder::new(cache.clone());
    seeder.seed_from_topology(&blocks).await.unwrap();
    
    // Create mock source (generates matching CIDs)
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    // Create transformer with real cache
    let transformer = EditsTransformer::new(cache, kafka_producer);
    
    // Run the pipeline - cache lookups work normally
    transformer.run_with_source(source).await.unwrap();
    
    // Verify edits were processed
    assert_eq!(kafka_mock.messages.len(), 6); // 6 edits in test topology
}
```

### E2E Test Flow

```bash
# 1. Start infrastructure
docker-compose up -d postgres kafka

# 2. Run migrations
sqlx migrate run

# 3. Seed the cache with mock GRC-20 data
cargo run --bin cache-seeder -- --topology deterministic

# 4. Run transformers with mock block source
MOCK_MODE=1 cargo run --bin hermes-edits

# 5. Verify Kafka output
kafka-console-consumer --topic edits --from-beginning
```

### Why This Approach

1. **Tests real code paths** - The cache, database, and transformer logic are all production code
2. **Real GRC-20 data** - The edit content is valid `grc20.Edit` protos, not fake data
3. **Deterministic** - Same topology always produces same CIDs and cache entries
4. **Debuggable** - Can inspect the cache contents in PostgreSQL during test failures

## Testing Strategy

### Unit Tests

Test `MockSource` produces correctly encoded protobufs:

```rust
#[test]
fn test_mock_source_encodes_space_events() {
    let blocks = vec![/* single space creation */];
    let mut source = MockSource::new(blocks, HermesModule::SpacesRegistered);
    
    let response = source.next().await.unwrap().unwrap();
    let data = match response {
        BlockResponse::New(d) => d,
        _ => panic!("expected new block"),
    };
    
    let decoded: SpacesRegistered = prost::Message::decode(&data.output[..]).unwrap();
    assert_eq!(decoded.spaces.len(), 1);
}
```

### Integration Tests

Test full transformer pipeline with mock data:

```rust
#[tokio::test]
async fn test_spaces_transformer_with_mock() {
    let blocks = mock_substream::test_topology::generate();
    let transformer = SpacesTransformer::new(/* kafka producer mock */);
    
    transformer.run_mock(blocks, HermesModule::Actions).await.unwrap();
    
    // Verify Kafka messages were produced
    assert_eq!(kafka_mock.messages.len(), 18); // 18 spaces
}
```

### End-to-End Tests

Run full hermes stack with mock data:

```bash
# Start Kafka
docker-compose up -d kafka

# Run transformers with mock mode
HERMES_MOCK_MODE=1 cargo run --bin hermes-spaces --features mock
HERMES_MOCK_MODE=1 cargo run --bin hermes-edits --features mock

# Verify Kafka topics have expected messages
kafka-console-consumer --topic spaces --from-beginning
```

## Cursor Persistence

Cursor persistence is critical for testing restart and replay scenarios. The mock infrastructure must support persisting and loading cursors to verify that transformers correctly resume from their last processed block.

### CursorStore Trait

Abstract cursor persistence to support both real (PostgreSQL) and mock (in-memory) implementations:

```rust
// hermes-relay/src/cursor.rs

/// Trait for cursor persistence implementations.
#[async_trait]
pub trait CursorStore: Send + Sync {
    /// Load the cursor for a given indexer ID.
    async fn load(&self, indexer_id: &str) -> Result<Option<CursorPosition>, anyhow::Error>;
    
    /// Persist the cursor for a given indexer ID.
    async fn persist(&self, indexer_id: &str, position: &CursorPosition) -> Result<(), anyhow::Error>;
}

/// Cursor position in the block stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorPosition {
    /// Opaque cursor string for resuming the stream.
    pub cursor: String,
    /// Block number for debugging/logging.
    pub block_number: u64,
    /// Timestamp when this cursor was persisted.
    pub persisted_at: u64,
}
```

### Mock Cursor Store

In-memory implementation for testing:

```rust
// mock-substream/src/cursor.rs (or hermes-relay/src/cursor/mock.rs)

use std::collections::HashMap;
use std::sync::RwLock;

/// In-memory cursor store for testing.
pub struct MockCursorStore {
    cursors: RwLock<HashMap<String, CursorPosition>>,
    /// History of all persist calls for verification.
    history: RwLock<Vec<(String, CursorPosition)>>,
}

impl MockCursorStore {
    pub fn new() -> Self {
        Self {
            cursors: RwLock::new(HashMap::new()),
            history: RwLock::new(Vec::new()),
        }
    }
    
    /// Create a cursor store pre-populated with a starting position.
    ///
    /// Useful for testing resume-from-cursor scenarios.
    pub fn with_cursor(indexer_id: &str, position: CursorPosition) -> Self {
        let store = Self::new();
        store.cursors.write().unwrap().insert(indexer_id.to_string(), position);
        store
    }
    
    /// Get the history of all persist calls.
    ///
    /// Useful for verifying cursor progression in tests.
    pub fn persist_history(&self) -> Vec<(String, CursorPosition)> {
        self.history.read().unwrap().clone()
    }
    
    /// Get the number of times persist was called for an indexer.
    pub fn persist_count(&self, indexer_id: &str) -> usize {
        self.history.read().unwrap()
            .iter()
            .filter(|(id, _)| id == indexer_id)
            .count()
    }
    
    /// Clear all cursors and history. Useful between test runs.
    pub fn clear(&self) {
        self.cursors.write().unwrap().clear();
        self.history.write().unwrap().clear();
    }
}

#[async_trait]
impl CursorStore for MockCursorStore {
    async fn load(&self, indexer_id: &str) -> Result<Option<CursorPosition>, anyhow::Error> {
        Ok(self.cursors.read().unwrap().get(indexer_id).cloned())
    }
    
    async fn persist(&self, indexer_id: &str, position: &CursorPosition) -> Result<(), anyhow::Error> {
        self.cursors.write().unwrap().insert(indexer_id.to_string(), position.clone());
        self.history.write().unwrap().push((indexer_id.to_string(), position.clone()));
        Ok(())
    }
}
```

### Updated Sink Trait

Modify the `Sink` trait to accept a `CursorStore`:

```rust
// hermes-relay/src/sink.rs

pub trait Sink: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// The indexer ID used for cursor persistence.
    fn indexer_id(&self) -> &str;

    /// Process a new block of data.
    fn process_block_scoped_data(
        &self,
        data: &BlockScopedData,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    /// Handle a block undo signal (chain reorganization).
    fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), Self::Error> {
        unimplemented!("you must implement block undo handling, or request only final blocks")
    }

    /// Run the sink with a custom block source and cursor store.
    fn run_with_source<S, C>(
        &self,
        source: S,
        cursor_store: C,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send
    where
        S: BlockSource,
        C: CursorStore,
    {
        async move {
            let mut source = source;
            
            loop {
                match source.next().await {
                    None => {
                        println!("Stream consumed");
                        break;
                    }
                    Some(Ok(BlockResponse::New(data))) => {
                        let block_scoped_data = self.block_data_to_scoped(&data)?;
                        self.process_block_scoped_data(&block_scoped_data).await?;
                        
                        // Persist cursor after successful processing
                        cursor_store.persist(self.indexer_id(), &CursorPosition {
                            cursor: data.cursor,
                            block_number: data.block_number,
                            persisted_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs(),
                        }).await?;
                    }
                    Some(Ok(BlockResponse::Undo(signal))) => {
                        self.process_block_undo_signal(&signal.into())?;
                        
                        cursor_store.persist(self.indexer_id(), &CursorPosition {
                            cursor: signal.last_valid_cursor,
                            block_number: signal.last_valid_block,
                            persisted_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs(),
                        }).await?;
                    }
                    Some(Err(err)) => {
                        println!("Stream terminated with error: {:?}", err);
                        std::process::exit(1);
                    }
                }
            }
            
            Ok(())
        }
    }
}
```

### MockSource with Cursor Resume

The `MockSource` should support starting from a cursor position:

```rust
// hermes-relay/src/source/mock.rs

impl MockSource {
    /// Create a mock source that resumes from a cursor position.
    ///
    /// Skips blocks until the cursor is found, then begins emitting.
    pub fn resume_from(
        blocks: Vec<MockBlock>,
        module: HermesModule,
        cursor: &str,
    ) -> Self {
        let mut source = Self::new(blocks, module);
        
        // Skip blocks until we find the cursor
        while let Some(block) = source.blocks.front() {
            if block.cursor == cursor {
                // Found the cursor - remove this block (already processed)
                // and start from the next one
                source.blocks.pop_front();
                source.current_cursor = Some(cursor.to_string());
                break;
            }
            source.blocks.pop_front();
        }
        
        source
    }
    
    /// Create a mock source with cursor store integration.
    ///
    /// Automatically loads the cursor and resumes from the correct position.
    pub async fn with_cursor_store<C: CursorStore>(
        blocks: Vec<MockBlock>,
        module: HermesModule,
        cursor_store: &C,
        indexer_id: &str,
    ) -> Result<Self, anyhow::Error> {
        let cursor = cursor_store.load(indexer_id).await?;
        
        Ok(match cursor {
            Some(pos) => Self::resume_from(blocks, module, &pos.cursor),
            None => Self::new(blocks, module),
        })
    }
}
```

### Testing Cursor Persistence

```rust
#[tokio::test]
async fn test_cursor_persisted_after_each_block() {
    let blocks = mock_substream::test_topology::generate();
    let block_count = blocks.len();
    
    let source = MockSource::new(blocks, HermesModule::Actions);
    let cursor_store = MockCursorStore::new();
    let transformer = SpacesTransformer::new(kafka_mock);
    
    transformer.run_with_source(source, &cursor_store).await.unwrap();
    
    // Verify cursor was persisted after each block
    assert_eq!(cursor_store.persist_count("hermes_spaces"), block_count);
    
    // Verify final cursor position
    let final_cursor = cursor_store.load("hermes_spaces").await.unwrap().unwrap();
    assert_eq!(final_cursor.block_number, 1_000_000 + block_count as u64 - 1);
}

#[tokio::test]
async fn test_resume_from_cursor() {
    let blocks = mock_substream::test_topology::generate();
    let total_blocks = blocks.len();
    
    // Process first half
    let cursor_store = MockCursorStore::new();
    let first_half: Vec<_> = blocks.iter().take(total_blocks / 2).cloned().collect();
    let source = MockSource::new(first_half, HermesModule::Actions);
    let transformer = SpacesTransformer::new(kafka_mock.clone());
    
    transformer.run_with_source(source, &cursor_store).await.unwrap();
    
    let halfway_cursor = cursor_store.load("hermes_spaces").await.unwrap().unwrap();
    let first_half_messages = kafka_mock.messages.len();
    
    // Resume from cursor with all blocks
    let source = MockSource::resume_from(blocks, HermesModule::Actions, &halfway_cursor.cursor);
    
    transformer.run_with_source(source, &cursor_store).await.unwrap();
    
    // Should have processed remaining blocks
    let total_messages = kafka_mock.messages.len();
    assert!(total_messages > first_half_messages);
}

#[tokio::test]
async fn test_restart_from_persisted_cursor() {
    let blocks = mock_substream::test_topology::generate();
    
    // First run - process some blocks then "crash"
    let cursor_store = Arc::new(MockCursorStore::new());
    let partial_blocks: Vec<_> = blocks.iter().take(5).cloned().collect();
    let source = MockSource::new(partial_blocks, HermesModule::Actions);
    let transformer = SpacesTransformer::new(kafka_mock.clone());
    
    transformer.run_with_source(source, cursor_store.as_ref()).await.unwrap();
    
    // Simulate restart - create new source from cursor store
    let source = MockSource::with_cursor_store(
        blocks.clone(),
        HermesModule::Actions,
        cursor_store.as_ref(),
        "hermes_spaces",
    ).await.unwrap();
    
    // Should skip already-processed blocks
    let remaining_blocks: Vec<_> = source.blocks.iter().collect();
    assert_eq!(remaining_blocks.len(), blocks.len() - 5);
}
```

### PostgreSQL Cursor Store (Production)

For completeness, here's the production implementation:

```rust
// hermes-relay/src/cursor/postgres.rs

pub struct PostgresCursorStore {
    pool: sqlx::PgPool,
}

impl PostgresCursorStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CursorStore for PostgresCursorStore {
    async fn load(&self, indexer_id: &str) -> Result<Option<CursorPosition>, anyhow::Error> {
        let row = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT cursor, block_number, persisted_at FROM cursors WHERE indexer_id = $1"
        )
        .bind(indexer_id)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.map(|(cursor, block_number, persisted_at)| CursorPosition {
            cursor,
            block_number: block_number as u64,
            persisted_at: persisted_at as u64,
        }))
    }
    
    async fn persist(&self, indexer_id: &str, position: &CursorPosition) -> Result<(), anyhow::Error> {
        sqlx::query(
            "INSERT INTO cursors (indexer_id, cursor, block_number, persisted_at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (indexer_id) DO UPDATE SET \
                cursor = EXCLUDED.cursor, \
                block_number = EXCLUDED.block_number, \
                persisted_at = EXCLUDED.persisted_at"
        )
        .bind(indexer_id)
        .bind(&position.cursor)
        .bind(position.block_number as i64)
        .bind(position.persisted_at as i64)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
}
```

## Open Questions

1. **Undo Signal Testing:** Should `MockSource` support generating undo signals for reorg testing? This would require extending `mock_substream::MockBlock`.

## References

- [hermes-architecture.md](../../docs/hermes-architecture.md) - System architecture
- [0001-multiple-substreams-modules-consumers.md](./decisions/0001-multiple-substreams-modules-consumers.md) - Module selection rationale
- [mock-substream](../../mock-substream/) - Mock event generator
