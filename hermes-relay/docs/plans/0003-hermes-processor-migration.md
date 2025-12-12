# Migrate hermes-processor and hermes-spaces to hermes-relay

## Status

Proposed

## Problem

Several services still use outdated patterns for consuming blockchain events:

1. **hermes-processor** - Uses `mock-substream` directly instead of `hermes-relay`
2. **hermes-spaces** - Uses old `hermes-relay` API (`run(&endpoint, module, start, end)`)
3. **hermes-ipfs-cache** - Uses old `hermes-relay` API and needs `MockIpfsClient` for testing

These services need to be updated to use the new `StreamSource` config-based API introduced in hermes-relay.

## Goals

1. Migrate `hermes-processor` from `mock-substream` to `hermes-relay`
2. Update `hermes-spaces` to use `StreamSource` API
3. Update `hermes-ipfs-cache` to use `StreamSource` API
4. Add `MockIpfsClient` for testing IPFS-dependent services
5. Update GitHub workflows and Dockerfiles

## Design

### StreamSource API (Already Implemented)

The `StreamSource` enum provides explicit configuration:

```rust
pub enum StreamSource {
    /// Mock data - all test topology events in a single block
    Mock,
    
    /// Live substream connection
    Live {
        endpoint_url: String,
        module: HermesModule,
        start_block: i64,
        end_block: u64,
    },
}

// Usage
sink.run(StreamSource::mock()).await?;
sink.run(StreamSource::live(endpoint, module, start, end)).await?;
```

### IpfsClient Trait Abstraction

Abstract the IPFS client behind a trait for dependency injection:

```rust
// ipfs/src/lib.rs

use async_trait::async_trait;

/// Trait for fetching content from IPFS.
#[async_trait]
pub trait IpfsFetcher: Send + Sync {
    /// Fetch and decode a GRC-20 Edit from IPFS by URI.
    async fn get(&self, uri: &str) -> Result<Edit>;
}

/// Production IPFS client that fetches from a gateway.
pub struct IpfsClient {
    url: String,
    client: reqwest::Client,
}

#[async_trait]
impl IpfsFetcher for IpfsClient {
    async fn get(&self, uri: &str) -> Result<Edit> {
        // Existing implementation
    }
}
```

### MockIpfsClient

Add mock IPFS client in `hermes-relay` for testing:

```rust
// hermes-relay/src/source/mock_ipfs.rs

use std::collections::HashMap;
use std::sync::RwLock;
use ipfs::IpfsFetcher;

/// Mock IPFS client that returns pre-configured edit data.
pub struct MockIpfsClient {
    /// Map of URI -> Edit
    edits: RwLock<HashMap<String, Edit>>,
}

impl MockIpfsClient {
    pub fn new() -> Self {
        Self {
            edits: RwLock::new(HashMap::new()),
        }
    }
    
    /// Create from test topology - generates matching edits for mock events.
    pub fn from_test_topology() -> Self {
        let client = Self::new();
        
        // Pre-populate with edits matching test_topology::generate()
        for action in mock_events::test_topology::generate() {
            if is_edit_published(&action) {
                let uri = extract_ipfs_uri(&action);
                let edit = generate_mock_edit(&action);
                client.edits.write().unwrap().insert(uri, edit);
            }
        }
        
        client
    }
    
    /// Register a custom edit for testing.
    pub fn register_edit(&self, uri: &str, edit: Edit) {
        self.edits.write().unwrap().insert(uri.to_string(), edit);
    }
}

#[async_trait]
impl IpfsFetcher for MockIpfsClient {
    async fn get(&self, uri: &str) -> Result<Edit> {
        self.edits
            .read()
            .unwrap()
            .get(uri)
            .cloned()
            .ok_or_else(|| IpfsError::NotFound(uri.to_string()))
    }
}
```

### IpfsSource Config

Similar to `StreamSource`, add an `IpfsSource` enum:

```rust
// hermes-relay/src/ipfs.rs

pub enum IpfsSource {
    /// Mock IPFS client with test topology data
    Mock,
    
    /// Live IPFS gateway
    Live { gateway_url: String },
}

impl IpfsSource {
    pub fn mock() -> Self {
        Self::Mock
    }
    
    pub fn live(gateway_url: impl Into<String>) -> Self {
        Self::Live {
            gateway_url: gateway_url.into(),
        }
    }
    
    /// Create the appropriate IpfsFetcher implementation.
    pub fn create_fetcher(&self) -> Box<dyn IpfsFetcher> {
        match self {
            Self::Mock => Box::new(MockIpfsClient::from_test_topology()),
            Self::Live { gateway_url } => Box::new(IpfsClient::new(gateway_url)),
        }
    }
}
```

## Migration Steps

### 1. hermes-processor

**Current state:** Uses `mock-substream` types directly.

**Changes needed:**

1. Update `Cargo.toml`:
   ```toml
   # Remove
   mock-substream = { path = "../mock-substream" }
   
   # Add
   hermes-relay = { path = "../hermes-relay" }
   tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
   anyhow = "1"
   ```

2. Implement `Sink` trait:
   ```rust
   struct HermesProcessorSink {
       producer: BaseProducer,
   }
   
   impl Sink for HermesProcessorSink {
       type Error = ProcessorError;
       
       async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
           let actions = Actions::decode(extract_output(data))?;
           
           for action in &actions.actions {
               match classify_action(&action.action) {
                   ActionType::SpaceRegistered => self.handle_space_created(action)?,
                   ActionType::SubspaceAdded => self.handle_trust_extended(action)?,
                   ActionType::EditsPublished => self.handle_edit_published(action)?,
                   _ => {} // Ignore other action types
               }
           }
           
           Ok(())
       }
   }
   ```

3. Update `main.rs`:
   ```rust
   #[tokio::main]
   async fn main() -> Result<()> {
       let producer = create_producer(&broker)?;
       let sink = HermesProcessorSink::new(producer);
       
       // Use mock for development
       sink.run(StreamSource::mock()).await?;
       
       Ok(())
   }
   ```

4. Update `Dockerfile`:
   ```dockerfile
   COPY hermes-relay ./hermes-relay
   COPY hermes-substream ./hermes-substream
   COPY stream ./stream
   # Remove: COPY mock-substream ./mock-substream
   ```

5. Update `.github/workflows/hermes-deploy.yml`:
   ```yaml
   paths:
     - 'hermes-processor/**'
     - 'hermes-relay/**'
     - 'hermes-substream/**'
     - 'stream/**'
     # Remove: - 'mock-substream/**'
   ```

### 2. hermes-spaces

**Current state:** Uses old `run(&endpoint, module, start, end)` API.

**Changes needed:**

Update `main.rs` to use `StreamSource`:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let transformer = SpacesTransformer::new(producer);
    
    let source = StreamSource::live(
        &endpoint,
        HermesModule::Actions,
        start_block,
        end_block,
    );
    
    transformer.run(source).await?;
    
    Ok(())
}
```

### 3. hermes-ipfs-cache

**Current state:** Uses old API and real `IpfsClient` only.

**Changes needed:**

1. Add `IpfsFetcher` trait to `ipfs` crate
2. Make `IpfsCacheSink` generic over `IpfsFetcher`:
   ```rust
   pub struct IpfsCacheSink<F: IpfsFetcher> {
       cache: Cache,
       ipfs: Arc<F>,
   }
   ```

3. Update `main.rs`:
   ```rust
   let source = StreamSource::live(...);
   let ipfs = IpfsSource::live(&gateway_url).create_fetcher();
   let sink = IpfsCacheSink::new(cache, ipfs);
   sink.run(source).await?;
   ```

4. Add tests with mock:
   ```rust
   #[tokio::test]
   async fn test_ipfs_cache_with_mock() {
       let cache = Cache::new(...);
       let ipfs = MockIpfsClient::from_test_topology();
       let sink = IpfsCacheSink::new(cache, ipfs);
       
       sink.run(StreamSource::mock()).await.unwrap();
       
       // Verify cache contents
   }
   ```

## File Changes Summary

### New Files

- `hermes-relay/src/source/mock_ipfs.rs` - MockIpfsClient implementation
- `hermes-relay/src/ipfs.rs` - IpfsSource enum

### Modified Files

| File | Changes |
|------|---------|
| `ipfs/src/lib.rs` | Add `IpfsFetcher` trait |
| `hermes-processor/Cargo.toml` | Replace mock-substream with hermes-relay |
| `hermes-processor/src/main.rs` | Implement Sink trait, use StreamSource |
| `hermes-processor/Dockerfile` | Update COPY statements |
| `hermes-spaces/src/main.rs` | Use StreamSource API |
| `hermes-ipfs-cache/src/lib.rs` | Make generic over IpfsFetcher |
| `hermes-ipfs-cache/src/main.rs` | Use StreamSource and IpfsSource |
| `.github/workflows/hermes-deploy.yml` | Update paths |

## Testing Strategy

1. **Unit tests**: Each service should have tests using `StreamSource::mock()` and `MockIpfsClient`
2. **Integration tests**: Test with real Kafka using docker-compose
3. **Manual verification**: Run services locally with mock data, verify Kafka messages

## Rollout Plan

1. Implement `IpfsFetcher` trait and `MockIpfsClient`
2. Migrate `hermes-spaces` (smallest change, already uses hermes-relay)
3. Migrate `hermes-processor` (larger change, needs full rewrite)
4. Migrate `hermes-ipfs-cache` (needs generic IpfsFetcher)
5. Update all workflows and Dockerfiles
6. Remove `mock-substream` crate once no longer used

## References

- [0001-mock-substream-integration.md](./0001-mock-substream-integration.md) - StreamSource design
- [0002-mock-ipfs-client.md](./0002-mock-ipfs-client.md) - MockIpfsClient design (proposed)
- [atlas migration](../../../atlas/) - Reference implementation using StreamSource
