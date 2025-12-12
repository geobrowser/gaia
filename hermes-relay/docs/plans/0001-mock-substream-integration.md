# Mock Substream Integration for hermes-relay

## Status

Proposed

## Problem

We need to develop and test hermes transformers and downstream architecture (Kafka consumers, indexers) without relying on upstream smart contract dependencies. The current `hermes-relay` implementation is tightly coupled to a real substream connection, making it impossible to run the full pipeline with mock data.

## Goals

1. Enable running hermes transformers with mock blockchain events
2. Support deterministic integration testing across the entire pipeline
3. Allow parallel development while not being coupled to smart contract deployment
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

For edit events, the `MockSource` generates deterministic CIDs that correspond to mock edit data. See [0002-mock-ipfs-client.md](./0002-mock-ipfs-client.md) for details on mocking the IPFS fetcher.

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

Cursor persistence is critical for testing restart and replay scenarios. We use the real PostgreSQL cursor storage in tests to verify that transformers correctly resume from their last processed block.

### MockSource with Cursor Resume

The `MockSource` should support starting from a cursor position stored in the database:

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
}
```

### Testing Cursor Persistence

Tests use real PostgreSQL to verify cursor behavior:

```rust
#[tokio::test]
async fn test_cursor_persisted_after_each_block() {
    let storage = Storage::new().await.unwrap();
    let blocks = mock_substream::test_topology::generate();
    let block_count = blocks.len();
    
    let source = MockSource::new(blocks, HermesModule::Actions);
    let transformer = SpacesTransformer::new(storage.clone(), kafka_mock);
    
    transformer.run_with_source(source).await.unwrap();
    
    // Verify final cursor position in real database
    let final_cursor = storage.load_cursor("hermes_spaces").await.unwrap().unwrap();
    assert_eq!(final_cursor.block_number, 1_000_000 + block_count as u64 - 1);
}

#[tokio::test]
async fn test_resume_from_cursor() {
    let storage = Storage::new().await.unwrap();
    let blocks = mock_substream::test_topology::generate();
    let total_blocks = blocks.len();
    
    // Process first half
    let first_half: Vec<_> = blocks.iter().take(total_blocks / 2).cloned().collect();
    let source = MockSource::new(first_half, HermesModule::Actions);
    let transformer = SpacesTransformer::new(storage.clone(), kafka_mock.clone());
    
    transformer.run_with_source(source).await.unwrap();
    
    let halfway_cursor = storage.load_cursor("hermes_spaces").await.unwrap().unwrap();
    let first_half_messages = kafka_mock.messages.len();
    
    // Resume from cursor with all blocks
    let source = MockSource::resume_from(blocks, HermesModule::Actions, &halfway_cursor.cursor);
    
    transformer.run_with_source(source).await.unwrap();
    
    // Should have processed remaining blocks
    let total_messages = kafka_mock.messages.len();
    assert!(total_messages > first_half_messages);
}

#[tokio::test]
async fn test_restart_from_persisted_cursor() {
    let storage = Storage::new().await.unwrap();
    let blocks = mock_substream::test_topology::generate();
    
    // First run - process some blocks then "crash"
    let partial_blocks: Vec<_> = blocks.iter().take(5).cloned().collect();
    let source = MockSource::new(partial_blocks, HermesModule::Actions);
    let transformer = SpacesTransformer::new(storage.clone(), kafka_mock.clone());
    
    transformer.run_with_source(source).await.unwrap();
    
    // Simulate restart - load cursor from database and resume
    let cursor = storage.load_cursor("hermes_spaces").await.unwrap().unwrap();
    let source = MockSource::resume_from(blocks.clone(), HermesModule::Actions, &cursor.cursor);
    
    // Should skip already-processed blocks
    let remaining_blocks: Vec<_> = source.blocks.iter().collect();
    assert_eq!(remaining_blocks.len(), blocks.len() - 5);
}
```

## Testing Failure States

Distributed systems fail in various ways. The mock infrastructure should support simulating failure modes to test resilience, especially around the IPFS cache which is a critical coordination point between services.

### IPFS Fetch Failures

The `MockIpfsClient` can be extended to simulate various failure modes:

```rust
// mock-substream/src/ipfs.rs

/// Failure modes that can be injected into the mock IPFS client.
#[derive(Debug, Clone)]
pub enum IpfsFailureMode {
    /// All fetches succeed (default)
    None,
    /// Specific CIDs fail with an error
    FailCids(HashSet<String>),
    /// Random failures with given probability (0.0 - 1.0)
    RandomFailure(f64),
    /// All fetches timeout (hang forever)
    Timeout,
    /// Fetches succeed but return malformed/corrupt data
    CorruptData,
    /// Fetches fail for the first N attempts, then succeed
    FailThenSucceed { attempts: usize },
    /// Fetches are delayed by a fixed duration
    Delay(Duration),
}

pub struct MockIpfsClient {
    edits: RwLock<HashMap<String, Vec<u8>>>,
    failure_mode: RwLock<IpfsFailureMode>,
    attempt_counts: RwLock<HashMap<String, usize>>,
}

impl MockIpfsClient {
    /// Set the failure mode for this client.
    pub fn set_failure_mode(&self, mode: IpfsFailureMode) {
        *self.failure_mode.write().unwrap() = mode;
    }
    
    /// Simulate a network partition - all fetches fail.
    pub fn simulate_network_partition(&self) {
        self.set_failure_mode(IpfsFailureMode::RandomFailure(1.0));
    }
    
    /// Simulate network recovery - fetches succeed again.
    pub fn simulate_network_recovery(&self) {
        self.set_failure_mode(IpfsFailureMode::None);
    }
    
    /// Simulate specific CIDs being unavailable (e.g., not yet pinned).
    pub fn simulate_missing_cids(&self, cids: HashSet<String>) {
        self.set_failure_mode(IpfsFailureMode::FailCids(cids));
    }
}

#[async_trait]
impl IpfsFetcher for MockIpfsClient {
    async fn get(&self, uri: &str) -> Result<Edit> {
        let cid = uri.split_once("://").map(|(_, c)| c).unwrap_or(uri);
        
        // Check failure mode
        match &*self.failure_mode.read().unwrap() {
            IpfsFailureMode::None => {}
            
            IpfsFailureMode::FailCids(cids) => {
                if cids.contains(cid) {
                    return Err(IpfsError::CidError(format!("CID unavailable: {}", cid)));
                }
            }
            
            IpfsFailureMode::RandomFailure(probability) => {
                if rand::random::<f64>() < *probability {
                    return Err(IpfsError::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "Simulated network failure",
                    )));
                }
            }
            
            IpfsFailureMode::Timeout => {
                // Hang forever (or until test timeout)
                futures::future::pending::<()>().await;
                unreachable!()
            }
            
            IpfsFailureMode::CorruptData => {
                return Err(IpfsError::DeserializeError(
                    wire::deserialize::DeserializeError::InvalidData("Corrupt data".into())
                ));
            }
            
            IpfsFailureMode::FailThenSucceed { attempts } => {
                let mut counts = self.attempt_counts.write().unwrap();
                let count = counts.entry(cid.to_string()).or_insert(0);
                *count += 1;
                if *count <= *attempts {
                    return Err(IpfsError::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        format!("Simulated failure, attempt {}/{}", count, attempts),
                    )));
                }
            }
            
            IpfsFailureMode::Delay(duration) => {
                tokio::time::sleep(*duration).await;
            }
        }
        
        // Normal fetch
        let bytes = self.get_bytes(cid).await?;
        let edit = wire::deserialize::deserialize(&bytes)?;
        Ok(edit)
    }
}
```

### Cache Failure Scenarios

Test scenarios for IPFS cache resilience:

```rust
#[tokio::test]
async fn test_cache_handles_ipfs_timeout() {
    let blocks = test_topology::generate();
    let mock_ipfs = MockIpfsClient::from_topology(&blocks);
    
    // Simulate IPFS gateway being slow/unavailable
    mock_ipfs.set_failure_mode(IpfsFailureMode::Timeout);
    
    let cache = Cache::new(Storage::new().await.unwrap());
    let sink = IpfsCacheSink::new(cache, mock_ipfs);
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    // Should timeout or handle gracefully (depending on implementation)
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        sink.run_with_source(source),
    ).await;
    
    assert!(result.is_err(), "Should timeout when IPFS is unavailable");
}

#[tokio::test]
async fn test_cache_retries_failed_fetches() {
    let blocks = test_topology::generate();
    let mock_ipfs = MockIpfsClient::from_topology(&blocks);
    
    // Fail first 2 attempts, then succeed
    mock_ipfs.set_failure_mode(IpfsFailureMode::FailThenSucceed { attempts: 2 });
    
    let cache = Cache::new(Storage::new().await.unwrap());
    let sink = IpfsCacheSink::new(cache, mock_ipfs);
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    // Should eventually succeed after retries
    sink.run_with_source(source).await.unwrap();
    
    // Verify all edits were cached
    // ...
}

#[tokio::test]
async fn test_cache_marks_corrupt_data_as_errored() {
    let blocks = test_topology::generate();
    let mock_ipfs = MockIpfsClient::from_topology(&blocks);
    
    // Return corrupt data for all fetches
    mock_ipfs.set_failure_mode(IpfsFailureMode::CorruptData);
    
    let cache = Cache::new(Storage::new().await.unwrap());
    let sink = IpfsCacheSink::new(cache, mock_ipfs);
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    sink.run_with_source(source).await.unwrap();
    
    // Verify edits were cached but marked as errored
    let item = cache.get("ipfs://Qm...").await.unwrap().unwrap();
    assert!(item.is_errored);
    assert!(item.json.is_none());
}

#[tokio::test]
async fn test_cache_handles_partial_failures() {
    let blocks = test_topology::generate();
    let mock_ipfs = MockIpfsClient::from_topology(&blocks);
    
    // Only some CIDs fail
    let failing_cids: HashSet<_> = vec![
        "Qm00000000000000000000000000000000e1".to_string(),
        "Qm00000000000000000000000000000000ea".to_string(),
    ].into_iter().collect();
    mock_ipfs.set_failure_mode(IpfsFailureMode::FailCids(failing_cids));
    
    let cache = Cache::new(Storage::new().await.unwrap());
    let sink = IpfsCacheSink::new(cache, mock_ipfs);
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    sink.run_with_source(source).await.unwrap();
    
    // Successful CIDs should be cached normally
    // Failed CIDs should be marked as errored
}
```

### Downstream Consumer Resilience

Test that downstream consumers (edits transformer) handle cache states correctly:

```rust
#[tokio::test]
async fn test_edits_transformer_waits_for_cache() {
    // Cache is empty - edits transformer should wait/retry
    let cache = Cache::new(Storage::new().await.unwrap());
    let transformer = EditsTransformer::new(cache, kafka_producer);
    
    let blocks = test_topology::generate();
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    // Start transformer (will wait for cache)
    let transformer_handle = tokio::spawn(async move {
        transformer.run_with_source(source).await
    });
    
    // Simulate cache being populated after a delay
    tokio::time::sleep(Duration::from_millis(500)).await;
    populate_cache_from_topology(&cache, &blocks).await;
    
    // Transformer should complete successfully
    transformer_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_edits_transformer_skips_errored_cache_entries() {
    let blocks = test_topology::generate();
    
    // Pre-populate cache with some errored entries
    let cache = Cache::new(Storage::new().await.unwrap());
    for block in &blocks {
        for event in &block.events {
            if let MockEvent::EditPublished(edit) = event {
                let cid = MockIpfsClient::deterministic_cid(&edit.edit_id);
                let is_errored = edit.edit_id == EDIT_ROOT_1; // Mark one as errored
                
                cache.put(&CacheItem {
                    uri: format!("ipfs://{}", cid),
                    json: if is_errored { None } else { Some(to_grc20_edit(edit)) },
                    block: block.timestamp.to_string(),
                    space_id: hex::encode(&edit.space_id),
                    is_errored,
                }).await.unwrap();
            }
        }
    }
    
    let transformer = EditsTransformer::new(cache, kafka_producer);
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    transformer.run_with_source(source).await.unwrap();
    
    // Should have produced 5 edits (skipped 1 errored)
    assert_eq!(kafka_mock.messages.len(), 5);
}
```

### Network Partition Simulation

Test behavior during network partitions between services:

```rust
#[tokio::test]
async fn test_cache_cursor_consistency_during_partition() {
    let blocks = test_topology::generate();
    let mock_ipfs = MockIpfsClient::from_topology(&blocks);
    let cache = Cache::new(Storage::new().await.unwrap());
    let sink = IpfsCacheSink::new(cache.clone(), mock_ipfs.clone());
    
    // Process first half of blocks
    let first_half: Vec<_> = blocks.iter().take(blocks.len() / 2).cloned().collect();
    let source = MockSource::new(first_half, HermesModule::EditsPublished);
    sink.run_with_source(source).await.unwrap();
    
    let cursor_before = cache.load_cursor("hermes_ipfs_cache").await.unwrap();
    
    // Simulate network partition during second half
    mock_ipfs.simulate_network_partition();
    
    let second_half: Vec<_> = blocks.iter().skip(blocks.len() / 2).cloned().collect();
    let source = MockSource::new(second_half.clone(), HermesModule::EditsPublished);
    
    // This should fail or timeout
    let result = sink.run_with_source(source).await;
    assert!(result.is_err());
    
    // Cursor should not have advanced past the partition
    let cursor_after = cache.load_cursor("hermes_ipfs_cache").await.unwrap();
    assert_eq!(cursor_before, cursor_after, "Cursor should not advance during failed processing");
    
    // Recover and retry
    mock_ipfs.simulate_network_recovery();
    let source = MockSource::new(second_half, HermesModule::EditsPublished);
    sink.run_with_source(source).await.unwrap();
    
    // Now cursor should have advanced
    let cursor_final = cache.load_cursor("hermes_ipfs_cache").await.unwrap();
    assert!(cursor_final.unwrap().block_number > cursor_before.unwrap().block_number);
}
```

## Open Questions

1. **Undo Signal Testing:** Should `MockSource` support generating undo signals for reorg testing? This would require extending `mock_substream::MockBlock`.

## References

- [hermes-architecture.md](../../docs/hermes-architecture.md) - System architecture
- [0001-multiple-substreams-modules-consumers.md](./decisions/0001-multiple-substreams-modules-consumers.md) - Module selection rationale
- [mock-substream](../../mock-substream/) - Mock event generator
