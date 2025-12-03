# Knowledge Engine Durable Storage

This document describes the durable storage layer for the Geo Knowledge Graph, focusing on content addressing, provider independence, and durability guarantees.

## Design Principles

1. **Durability first** - Data must exist forever
2. **Content addressing** - CIDs as universal identifiers, not location URLs
3. **Provider independence** - No lock-in to any single storage provider
4. **Performance** - Fast reads for real-time indexing

## Content Addressing

The key abstraction is the **CID (Content Identifier)**—a hash of the content itself.

```
Location-addressed:  s3://my-bucket/edits/abc123
                     https://api.geo.io/edits/abc123
                     ↑ Tied to provider, can break

Content-addressed:   bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oc...
                     ↑ Hash of content, works anywhere
```

**The CID says "this exact data" regardless of where it lives.**

Benefits:

| Benefit | How CID Enables It |
|---------|-------------------|
| **Provider independence** | Switch S3 → R2 → GCS without changing identifiers |
| **Verification** | Any provider's data can be verified against CID |
| **Redundancy** | Same CID stored in multiple places |
| **Migration** | Move data between providers, CIDs stay same |
| **Caching** | Cache anywhere, CID guarantees correctness |
| **Decentralization ready** | Add IPFS/Arweave anytime, same CIDs work |

## Why Not Pure IPFS?

IPFS provides content addressing, but has operational challenges:

| Dimension | IPFS Promise | Reality |
|-----------|--------------|---------|
| **Reliability** | Battle-tested network | No SLA for your data |
| **Durability** | Persists if pinned | If no one pins it, it's gone |
| **Performance** | Content retrieval | Unpredictable latency, DHT sync issues |
| **Availability** | Distributed = resilient | Centralized gateway = single point of failure |

**Our requirements:**

| Requirement | IPFS Provides | What We Need |
|-------------|---------------|--------------|
| Write latency | Seconds to minutes | <100ms |
| Read latency | Unpredictable | <10ms |
| Availability | Best effort | 99.9%+ |
| Durability | Only if pinned | Guaranteed |

**Solution:** Use CIDs as the identifier format, but store in multiple providers with different characteristics.

## Multi-Provider Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    STORAGE ABSTRACTION                      │
│                                                             │
│                         CID                                 │
│                          │                                  │
│          ┌───────────────┼───────────────┐                  │
│          ▼               ▼               ▼                  │
│    ┌──────────┐    ┌──────────┐    ┌──────────┐            │
│    │    S3    │    │   IPFS   │    │ Arweave  │            │
│    │ (fast)   │    │ (decent.)│    │ (perma)  │            │
│    └──────────┘    └──────────┘    └──────────┘            │
│                                                             │
│    Any provider can be added/removed without changing       │
│    the identifier. Data is the same if CID matches.         │
└─────────────────────────────────────────────────────────────┘
```

### Provider Characteristics

| Provider | Durability | Latency | Cost | Decentralized |
|----------|------------|---------|------|---------------|
| **S3/R2/GCS** | 99.999999999% | ~10ms | ~$0.02/GB/mo | No |
| **IPFS + Pinning** | Depends on pinning | 100ms-10s | ~$0.10/GB/mo | Yes |
| **Arweave** | Permanent | ~1s | ~$5/GB once | Yes |
| **Filecoin** | Incentivized | Minutes | Variable | Yes |

### Write Path

```
Client → API → S3 (immediate) → IPFS/Arweave (async backup)
                    │
                    └── CID computed from content
```

### Read Path

```
Indexer → S3 (fast) → IPFS (fallback) → Arweave (last resort)
```

## Implementation

### Core Abstraction

```rust
struct ContentAddressedStorage {
    providers: Vec<Box<dyn StorageProvider>>,
}

trait StorageProvider: Send + Sync {
    /// Retrieve content by CID
    async fn get(&self, cid: &Cid) -> Result<Vec<u8>>;

    /// Store content, returns CID
    async fn put(&self, content: &[u8]) -> Result<Cid>;

    /// Check if content exists
    async fn exists(&self, cid: &Cid) -> Result<bool>;

    /// Priority for read ordering (lower = try first)
    fn priority(&self) -> u32;

    /// Provider name for logging/metrics
    fn name(&self) -> &str;
}
```

### Read with Fallback

```rust
impl ContentAddressedStorage {
    async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        let providers = self.providers
            .iter()
            .sorted_by_key(|p| p.priority());

        for provider in providers {
            match provider.get(cid).await {
                Ok(content) => {
                    // Verify content matches CID
                    let actual_cid = compute_cid(&content);
                    if actual_cid == *cid {
                        return Ok(content);
                    }
                    // CID mismatch = corrupted/malicious data
                    warn!(
                        "CID mismatch from {}: expected {}, got {}",
                        provider.name(), cid, actual_cid
                    );
                }
                Err(e) => {
                    debug!("Provider {} failed: {}", provider.name(), e);
                }
            }
        }

        Err(Error::NotFound(cid.clone()))
    }
}
```

### Write with Redundancy

```rust
impl ContentAddressedStorage {
    async fn put(&self, content: &[u8]) -> Result<Cid> {
        let cid = compute_cid(content);

        // Store in primary provider synchronously
        let primary = self.providers
            .iter()
            .min_by_key(|p| p.priority())
            .ok_or(Error::NoProviders)?;

        primary.put(content).await?;

        // Store in backup providers asynchronously
        for provider in self.providers.iter().skip(1) {
            let provider = provider.clone();
            let content = content.to_vec();
            tokio::spawn(async move {
                if let Err(e) = provider.put(&content).await {
                    warn!("Backup to {} failed: {}", provider.name(), e);
                }
            });
        }

        Ok(cid)
    }
}
```

### Provider Implementations

```rust
// S3-compatible storage (S3, R2, GCS, MinIO)
struct S3Provider {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl StorageProvider for S3Provider {
    async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        let resp = self.client
            .get_object()
            .bucket(&self.bucket)
            .key(cid.to_string())
            .send()
            .await?;

        let bytes = resp.body.collect().await?.into_bytes();
        Ok(bytes.to_vec())
    }

    async fn put(&self, content: &[u8]) -> Result<Cid> {
        let cid = compute_cid(content);

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(cid.to_string())
            .body(content.to_vec().into())
            .send()
            .await?;

        Ok(cid)
    }

    async fn exists(&self, cid: &Cid) -> Result<bool> {
        match self.client
            .head_object()
            .bucket(&self.bucket)
            .key(cid.to_string())
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(e)) if e.err().is_not_found() => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn priority(&self) -> u32 { 0 }  // fastest
    fn name(&self) -> &str { "s3" }
}

// IPFS via HTTP gateway
struct IpfsProvider {
    gateway_url: String,
    pin_url: Option<String>,  // for pinning service
    client: reqwest::Client,
}

impl StorageProvider for IpfsProvider {
    async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        let url = format!("{}/ipfs/{}", self.gateway_url, cid);
        let resp = self.client
            .get(&url)
            .timeout(Duration::from_secs(30))
            .send()
            .await?;

        Ok(resp.bytes().await?.to_vec())
    }

    async fn put(&self, content: &[u8]) -> Result<Cid> {
        let Some(pin_url) = &self.pin_url else {
            return Err(Error::ReadOnly);
        };

        // Pin via pinning service API
        let resp = self.client
            .post(pin_url)
            .body(content.to_vec())
            .send()
            .await?;

        let cid = resp.json::<PinResponse>().await?.cid;
        Ok(cid.parse()?)
    }

    fn priority(&self) -> u32 { 10 }  // slower, fallback
    fn name(&self) -> &str { "ipfs" }
}

// Arweave permanent storage
struct ArweaveProvider {
    gateway_url: String,
    wallet: Option<ArweaveWallet>,
    client: reqwest::Client,
}

impl StorageProvider for ArweaveProvider {
    async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        // Arweave uses its own tx IDs, need mapping
        let tx_id = self.lookup_tx_id(cid).await?;
        let url = format!("{}/{}", self.gateway_url, tx_id);

        Ok(self.client.get(&url).send().await?.bytes().await?.to_vec())
    }

    async fn put(&self, content: &[u8]) -> Result<Cid> {
        let Some(wallet) = &self.wallet else {
            return Err(Error::ReadOnly);
        };

        // Create and submit Arweave transaction
        // Tag with CID for lookup
        let cid = compute_cid(content);
        let tx = self.create_transaction(content, &cid, wallet).await?;
        self.submit_transaction(tx).await?;

        Ok(cid)
    }

    fn priority(&self) -> u32 { 20 }  // last resort
    fn name(&self) -> &str { "arweave" }
}

// Local filesystem (for development)
struct LocalProvider {
    base_path: PathBuf,
}

impl StorageProvider for LocalProvider {
    async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        let path = self.base_path.join(cid.to_string());
        Ok(tokio::fs::read(&path).await?)
    }

    async fn put(&self, content: &[u8]) -> Result<Cid> {
        let cid = compute_cid(content);
        let path = self.base_path.join(cid.to_string());
        tokio::fs::write(&path, content).await?;
        Ok(cid)
    }

    fn priority(&self) -> u32 { 0 }
    fn name(&self) -> &str { "local" }
}
```

## CID Computation

Use the same algorithm as IPFS for compatibility:

```rust
use cid::Cid;
use multihash::{Code, MultihashDigest};

fn compute_cid(content: &[u8]) -> Cid {
    // Use SHA2-256, same as IPFS default
    let hash = Code::Sha2_256.digest(content);

    // CIDv1 with raw codec (0x55)
    Cid::new_v1(0x55, hash)
}
```

This ensures:
- CIDs match what IPFS would generate
- Data can be verified against CID from any source
- Compatible with IPFS ecosystem tooling

## On-Chain References

The blockchain stores only CIDs:

```
Block 1000:
  Event: EditPublished
  Space: 0x123...
  CID: bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oc...
```

The CID is permanent. Where it resolves to can change over time.

## Integration with Cold Storage Service

The Cold Storage Service (see [architecture doc](./knowledge-engine-architecture.md)) uses this abstraction:

```rust
struct ColdStorageService {
    storage: ContentAddressedStorage,
    cache: RocksDB,  // local cache of deserialized edits
    chain_cursor: BlockNumber,
}

impl ColdStorageService {
    async fn prefetch_and_cache(&self, block: BlockNumber, cid: &Cid) {
        if self.cache.contains(cid) {
            return;
        }

        // Fetch via multi-provider abstraction
        let bytes = self.storage.get(cid).await?;
        let edit = Edit::deserialize(&bytes)?;

        self.cache.put(cid, CachedEdit {
            block,
            edit,
            fetched_at: Instant::now(),
        });
    }
}
```

## Durability Strategy

### Current Approach

1. **Primary (S3):** Immediate storage, 11 nines durability, fast reads
2. **Secondary (IPFS):** Async backup, decentralized, content-addressed network
3. **Tertiary (Arweave):** Optional, pay-once permanent storage

### Cost Estimate

At current scale (~500 MB):

| Provider | Cost |
|----------|------|
| S3 Standard | ~$0.01/month |
| IPFS Pinning | ~$0.05/month |
| Arweave | ~$2.50 one-time |

Cost is negligible. Even at 100x scale, <$5/month total.

### Recovery Scenarios

| Scenario | Recovery Path |
|----------|---------------|
| S3 bucket deleted | Restore from IPFS/Arweave |
| IPFS data unpinned | Still in S3, re-pin |
| Provider goes down | Failover to other providers |
| All providers fail | Rebuild from blockchain (CIDs) + any surviving provider |

## Future Considerations

### Provider Health Monitoring

```rust
impl ContentAddressedStorage {
    async fn health_check(&self) -> Vec<ProviderHealth> {
        futures::future::join_all(
            self.providers.iter().map(|p| async {
                let start = Instant::now();
                let healthy = p.exists(&self.test_cid).await.is_ok();
                ProviderHealth {
                    name: p.name().to_string(),
                    healthy,
                    latency: start.elapsed(),
                }
            })
        ).await
    }
}
```

### Automatic Replication

Ensure data exists in minimum number of providers:

```rust
async fn ensure_replicated(&self, cid: &Cid, min_replicas: usize) {
    let exists: Vec<_> = futures::future::join_all(
        self.providers.iter().map(|p| p.exists(cid))
    ).await;

    let replica_count = exists.iter().filter(|r| r.is_ok()).count();

    if replica_count < min_replicas {
        // Fetch from existing provider, replicate to others
        let content = self.get(cid).await?;
        for (provider, exists) in self.providers.iter().zip(exists) {
            if exists.is_err() {
                provider.put(&content).await?;
            }
        }
    }
}
```

### Content Garbage Collection

For providers with storage costs, track which CIDs are still referenced:

```rust
async fn gc(&self, referenced_cids: &HashSet<Cid>) {
    for provider in &self.providers {
        if provider.supports_delete() {
            let stored = provider.list_all().await?;
            for cid in stored {
                if !referenced_cids.contains(&cid) {
                    provider.delete(&cid).await?;
                }
            }
        }
    }
}
```
