# Mock IPFS Client for Testing and Local Development

## Status

Proposed

## Problem

The `hermes-ipfs-cache` service fetches edit content from IPFS. For testing and local development, we need to mock these IPFS network calls while keeping the rest of the system (cache storage, cursor persistence) real.

## Goals

1. Mock IPFS network fetches without mocking the cache itself
2. Return real `grc20.Edit` protos that match mock `EditsPublished` events
3. Support failure injection for resilience testing
4. Enable local development without IPFS infrastructure

## Design

### IpfsFetcher Trait

Abstract the IPFS client behind a trait so we can swap implementations:

```rust
// ipfs/src/lib.rs

/// Trait for fetching content from IPFS.
#[async_trait]
pub trait IpfsFetcher: Send + Sync {
    /// Fetch and decode a GRC-20 Edit from IPFS by URI.
    async fn get(&self, uri: &str) -> Result<Edit>;
    
    /// Fetch raw bytes from IPFS by CID.
    async fn get_bytes(&self, cid: &str) -> Result<Vec<u8>>;
}

/// Production IPFS client that fetches from a gateway.
pub struct IpfsClient {
    url: String,
    client: ReqwestClient,
}

#[async_trait]
impl IpfsFetcher for IpfsClient {
    async fn get(&self, uri: &str) -> Result<Edit> {
        let cid = uri.split_once("://").map(|(_, c)| c).unwrap_or("");
        let bytes = self.get_bytes(cid).await?;
        let data = deserialize(&bytes)?;
        Ok(data)
    }
    
    async fn get_bytes(&self, cid: &str) -> Result<Vec<u8>> {
        let url = format!("{}{}", self.url, cid);
        let res = self.client.get(&url).send().await?;
        let bytes = res.bytes().await?;
        Ok(bytes.to_vec())
    }
}
```

### MockIpfsClient

The mock client returns pre-configured `grc20.Edit` data based on the CID:

```rust
// mock-substream/src/ipfs.rs

use std::collections::HashMap;
use std::sync::RwLock;
use wire::pb::grc20;

/// Mock IPFS client that returns pre-configured edit data.
///
/// Instead of fetching from IPFS, returns `grc20.Edit` protos that
/// correspond to mock `EditsPublished` events.
pub struct MockIpfsClient {
    /// Map of CID -> serialized Edit bytes
    edits: RwLock<HashMap<String, Vec<u8>>>,
}

impl MockIpfsClient {
    pub fn new() -> Self {
        Self {
            edits: RwLock::new(HashMap::new()),
        }
    }
    
    /// Create a mock client pre-populated with edits from the test topology.
    pub fn from_topology(blocks: &[MockBlock]) -> Self {
        let client = Self::new();
        
        for block in blocks {
            for event in &block.events {
                if let MockEvent::EditPublished(edit) = event {
                    let cid = Self::deterministic_cid(&edit.edit_id);
                    let wire_edit = Self::to_grc20_edit(edit);
                    
                    // Serialize the Edit proto to bytes (as it would be stored in IPFS)
                    let bytes = wire::serialize::serialize(&wire_edit);
                    client.edits.write().unwrap().insert(cid, bytes);
                }
            }
        }
        
        client
    }
    
    /// Register an edit to be returned for a given CID.
    pub fn register_edit(&self, cid: &str, edit: grc20::Edit) {
        let bytes = wire::serialize::serialize(&edit);
        self.edits.write().unwrap().insert(cid.to_string(), bytes);
    }
    
    /// Generate a deterministic CID from an edit ID.
    ///
    /// Both `MockSource` and `MockIpfsClient` use this to ensure CIDs match.
    pub fn deterministic_cid(edit_id: &EditId) -> String {
        format!("Qm{}", hex::encode(edit_id))
    }
    
    /// Convert mock edit to real `grc20.Edit` proto.
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

#[async_trait]
impl IpfsFetcher for MockIpfsClient {
    async fn get(&self, uri: &str) -> Result<Edit> {
        let cid = uri.split_once("://").map(|(_, c)| c).unwrap_or(uri);
        let bytes = self.get_bytes(cid).await?;
        let edit = wire::deserialize::deserialize(&bytes)?;
        Ok(edit)
    }
    
    async fn get_bytes(&self, cid: &str) -> Result<Vec<u8>> {
        self.edits
            .read()
            .unwrap()
            .get(cid)
            .cloned()
            .ok_or_else(|| IpfsError::CidError(format!("CID not found in mock: {}", cid)))
    }
}
```

### Updated IpfsCacheSink

Update `hermes-ipfs-cache` to accept any `IpfsFetcher` implementation:

```rust
// hermes-ipfs-cache/src/lib.rs

use ipfs::IpfsFetcher;

/// IPFS cache sink that implements the hermes-relay Sink trait.
///
/// Generic over the IPFS fetcher, allowing injection of mock implementations.
pub struct IpfsCacheSink<F: IpfsFetcher> {
    cache: Arc<Mutex<Cache>>,
    ipfs: Arc<F>,
    semaphore: Arc<Semaphore>,
    pending: Arc<Mutex<PendingFetches>>,
}

impl<F: IpfsFetcher + 'static> IpfsCacheSink<F> {
    /// Create a new IPFS cache sink with the given fetcher.
    pub fn new(cache: Cache, ipfs: F) -> Self {
        Self {
            cache: Arc::new(Mutex::new(cache)),
            ipfs: Arc::new(ipfs),
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
            pending: Arc::new(Mutex::new(PendingFetches::default())),
        }
    }
}

impl<F: IpfsFetcher + 'static> Sink for IpfsCacheSink<F> {
    type Error = IpfsCacheError;
    
    // ... implementation unchanged, just uses self.ipfs which is now generic ...
}
```

#### Type Aliases

```rust
// hermes-ipfs-cache/src/lib.rs

/// Production cache sink using real IPFS client.
pub type ProductionCacheSink = IpfsCacheSink<ipfs::IpfsClient>;

/// Mock cache sink for testing.
#[cfg(feature = "mock")]
pub type MockCacheSink = IpfsCacheSink<mock_substream::MockIpfsClient>;
```

## Failure Injection

The `MockIpfsClient` supports simulating various failure modes for resilience testing:

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

## Testing Failure States

### Cache Failure Scenarios

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
    
    sink.run_with_source(source).await.unwrap();
}

#[tokio::test]
async fn test_cache_marks_corrupt_data_as_errored() {
    let blocks = test_topology::generate();
    let mock_ipfs = MockIpfsClient::from_topology(&blocks);
    
    mock_ipfs.set_failure_mode(IpfsFailureMode::CorruptData);
    
    let cache = Cache::new(Storage::new().await.unwrap());
    let sink = IpfsCacheSink::new(cache, mock_ipfs);
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    sink.run_with_source(source).await.unwrap();
    
    let item = cache.get("ipfs://Qm...").await.unwrap().unwrap();
    assert!(item.is_errored);
    assert!(item.json.is_none());
}

#[tokio::test]
async fn test_cache_handles_partial_failures() {
    let blocks = test_topology::generate();
    let mock_ipfs = MockIpfsClient::from_topology(&blocks);
    
    let failing_cids: HashSet<_> = vec![
        "Qm00000000000000000000000000000000e1".to_string(),
    ].into_iter().collect();
    mock_ipfs.set_failure_mode(IpfsFailureMode::FailCids(failing_cids));
    
    let cache = Cache::new(Storage::new().await.unwrap());
    let sink = IpfsCacheSink::new(cache, mock_ipfs);
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    sink.run_with_source(source).await.unwrap();
    
    // Successful CIDs cached normally, failed CIDs marked as errored
}
```

### Downstream Consumer Resilience

```rust
#[tokio::test]
async fn test_edits_transformer_waits_for_cache() {
    let cache = Cache::new(Storage::new().await.unwrap());
    let transformer = EditsTransformer::new(cache, kafka_producer);
    
    let blocks = test_topology::generate();
    let source = MockSource::new(blocks, HermesModule::EditsPublished);
    
    let transformer_handle = tokio::spawn(async move {
        transformer.run_with_source(source).await
    });
    
    // Simulate cache being populated after a delay
    tokio::time::sleep(Duration::from_millis(500)).await;
    populate_cache_from_topology(&cache, &blocks).await;
    
    transformer_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_edits_transformer_skips_errored_cache_entries() {
    let blocks = test_topology::generate();
    let cache = Cache::new(Storage::new().await.unwrap());
    
    // Pre-populate cache with some errored entries
    for block in &blocks {
        for event in &block.events {
            if let MockEvent::EditPublished(edit) = event {
                let cid = MockIpfsClient::deterministic_cid(&edit.edit_id);
                let is_errored = edit.edit_id == EDIT_ROOT_1;
                
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
    
    // Simulate network partition
    mock_ipfs.simulate_network_partition();
    
    let second_half: Vec<_> = blocks.iter().skip(blocks.len() / 2).cloned().collect();
    let source = MockSource::new(second_half.clone(), HermesModule::EditsPublished);
    
    let result = sink.run_with_source(source).await;
    assert!(result.is_err());
    
    // Cursor should not have advanced
    let cursor_after = cache.load_cursor("hermes_ipfs_cache").await.unwrap();
    assert_eq!(cursor_before, cursor_after);
    
    // Recover and retry
    mock_ipfs.simulate_network_recovery();
    let source = MockSource::new(second_half, HermesModule::EditsPublished);
    sink.run_with_source(source).await.unwrap();
    
    let cursor_final = cache.load_cursor("hermes_ipfs_cache").await.unwrap();
    assert!(cursor_final.unwrap().block_number > cursor_before.unwrap().block_number);
}
```

## Usage

### Production

```rust
use ipfs::IpfsClient;

let cache = Cache::new(Storage::new().await?);
let ipfs = IpfsClient::new(&env::var("IPFS_GATEWAY_URL")?);
let sink = IpfsCacheSink::new(cache, ipfs);
```

### Tests / Local Dev

```rust
use mock_substream::{test_topology, MockIpfsClient};

let blocks = test_topology::generate();
let mock_ipfs = MockIpfsClient::from_topology(&blocks);
let cache = Cache::new(Storage::new().await?);
let sink = IpfsCacheSink::new(cache, mock_ipfs);
```

## Data Flow

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   MockSource    │────▶│ IpfsCacheSink   │────▶│  Cache (real)   │
│ (mock blocks)   │     │                 │     │  (PostgreSQL)   │
└─────────────────┘     └────────┬────────┘     └─────────────────┘
                                 │
                        ┌────────▼────────┐
                        │ MockIpfsClient  │
                        │ (returns real   │
                        │  grc20.Edit)    │
                        └─────────────────┘
```

## References

- [0001-mock-substream-integration.md](./0001-mock-substream-integration.md) - Block source mocking
- [wire/proto/grc20.proto](../../../wire/proto/grc20.proto) - GRC-20 Edit proto schema
- [hermes-ipfs-cache](../../../hermes-ipfs-cache/) - IPFS cache service
