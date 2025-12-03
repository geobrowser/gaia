# Knowledge Engine Architecture

This document describes the overall system architecture for the Geo Knowledge Engine, including disaggregated storage, compute scaling, and high availability.

## Overview

The Knowledge Engine uses a **disaggregated storage architecture** where compute and storage are separated into independent, scalable layers. This is similar to modern cloud databases that separate compute from object storage.

```
                         Clients
                            │
                            ▼
                    ┌───────────────┐
                    │ Load Balancer │
                    └───────────────┘
                      /     |     \
                     ▼      ▼      ▼
        ┌──────────────────────────────────────┐
        │         COMPUTE LAYER                │
        │      (Knowledge Engine Nodes)        │
        │                                      │
        │  ┌────────┐ ┌────────┐ ┌────────┐   │
        │  │ Node 1 │ │ Node 2 │ │ Node 3 │   │
        │  │ (hot)  │ │ (hot)  │ │ (hot)  │   │
        │  │ (warm) │ │ (warm) │ │ (warm) │   │
        │  └────────┘ └────────┘ └────────┘   │
        └──────────────────────────────────────┘
                         │
                         │ get_edits()
                         ▼
        ┌──────────────────────────────────────┐
        │       COLD STORAGE SERVICE           │
        │         (IPFS Cache Layer)           │
        │                                      │
        │  ┌────────────────────────────────┐  │
        │  │     Edit Cache (RocksDB/S3)    │  │
        │  │  - indexed by block, CID       │  │
        │  │  - pre-fetched ahead of chain  │  │
        │  └────────────────────────────────┘  │
        │                 │                    │
        │                 │ cache miss         │
        │                 ▼                    │
        │  ┌────────────────────────────────┐  │
        │  │        IPFS Gateway            │  │
        │  └────────────────────────────────┘  │
        └──────────────────────────────────────┘
                         │
                         │ watch events
                         ▼
        ┌──────────────────────────────────────┐
        │            BLOCKCHAIN                │
        │     (WAL - ordered CID references)   │
        └──────────────────────────────────────┘
```

## Storage Temperature Hierarchy

| Layer | Location | Access Time | Contents | Persistence |
|-------|----------|-------------|----------|-------------|
| **Hot** | RAM | <1μs | Current state, indexes, query cache | Lost on restart |
| **Warm** | Local SSD | ~1ms | Checkpoints, history segments | Survives restart |
| **Cold** | Remote service | ~10ms | Cached IPFS edits | Shared across nodes |
| **Frozen** | IPFS | 100ms-10s | Raw edit data (origin) | Permanent, immutable |

## Layer Details

### Compute Layer (Knowledge Engine Nodes)

Stateless compute nodes that serve queries. Each node independently indexes from the same source of truth.

**Hot Storage (In-Memory)**
- Current entity state in HashMaps
- Indexes for filtering and querying
- Query result cache
- ~1 GB at current scale

**Warm Storage (Local Disk)**
- Checkpoints for fast restart
- Historical data segments (optional)
- ~10 GB capacity

```rust
struct KnowledgeEngine {
    // Hot storage
    store: PrimaryStore,
    query_cache: LruCache<QueryHash, CachedResult>,

    // Warm storage
    checkpoints: CheckpointManager,
    history: HistoryStore,

    // Cold storage client
    cold_storage: ColdStorageClient,
}
```

See [knowledge-engine-storage.md](./knowledge-engine-storage.md) for detailed storage implementation.

### Cold Storage Service (IPFS Cache)

A dedicated service that pre-fetches IPFS content ahead of consumers. This isolates the slow, unreliable IPFS fetches from the query path.

**Responsibilities:**
- Watch blockchain for new events
- Pre-fetch edit content from IPFS
- Cache deserialized edits
- Serve edits to compute nodes

```rust
struct ColdStorageService {
    // Persistent cache
    db: RocksDB,

    // IPFS gateway for cache misses
    ipfs_client: IpfsClient,

    // How far ahead we've cached
    cache_head: BlockNumber,
}

impl ColdStorageService {
    /// Continuously fetch and cache new edits
    async fn run(&mut self) {
        loop {
            let latest_block = self.chain.get_latest_block().await;

            for block in self.cache_head..=latest_block {
                let events = self.chain.get_events(block).await;

                for event in events {
                    if let Some(cid) = event.ipfs_cid {
                        self.prefetch_and_cache(block, &cid).await;
                    }
                }

                self.cache_head = block;
            }

            sleep(Duration::from_secs(1)).await;
        }
    }

    async fn prefetch_and_cache(&self, block: BlockNumber, cid: &str) {
        if self.db.contains(cid) {
            return;
        }

        // Fetch from IPFS (slow, may fail)
        let bytes = self.ipfs_client.get(cid).await?;
        let edit = Edit::deserialize(&bytes)?;

        self.db.put(cid, CachedEdit {
            block,
            space_id: edit.space_id,
            raw_bytes: bytes,
            deserialized: edit,
            fetched_at: Instant::now(),
        });
    }
}
```

**API for Consumers:**

```rust
impl ColdStorageService {
    /// Get edit by CID (fast, from cache)
    async fn get_edit(&self, cid: &str) -> Result<Edit>;

    /// Get all edits for a block range (for replay)
    async fn get_edits_range(&self, start: BlockNumber, end: BlockNumber) -> Vec<Edit>;

    /// Current cache head (how far ahead we've fetched)
    fn cache_head(&self) -> BlockNumber;
}
```

### Blockchain (WAL)

The blockchain serves as an external, authoritative write-ahead log:

- Total ordering of all edits
- CID references to IPFS content
- Consensus already handled
- Immutable history

**Key insight:** Because the WAL is external, compute nodes don't need to coordinate with each other. Any node can independently rebuild state by replaying from the blockchain.

## Compute Node Lifecycle

### Startup

```rust
impl KnowledgeEngine {
    async fn startup(&mut self) -> Result<()> {
        // 1. Load from warm storage (local checkpoint)
        if let Some(checkpoint) = self.checkpoints.load_latest()? {
            self.store = checkpoint;
            println!("Loaded checkpoint at block {}", self.store.current_block);
        }

        // 2. Replay from cold storage (cached edits)
        let start = self.store.current_block + 1;
        let cache_head = self.cold_storage.cache_head().await;

        println!("Replaying blocks {} to {}", start, cache_head);

        let edits = self.cold_storage.get_edits_range(start, cache_head).await;
        for edit in edits {
            self.apply_edit(edit)?;
        }

        println!("Ready at block {}", self.store.current_block);
        Ok(())
    }
}
```

### Sync Loop

```rust
impl KnowledgeEngine {
    async fn sync_loop(&mut self) {
        loop {
            let cache_head = self.cold_storage.cache_head().await;

            if cache_head > self.store.current_block {
                let edits = self.cold_storage
                    .get_edits_range(self.store.current_block + 1, cache_head)
                    .await;

                for edit in edits {
                    self.apply_edit(edit)?;
                }
            }

            // Periodic checkpoint
            self.checkpoints.maybe_checkpoint(&self.store)?;

            sleep(Duration::from_millis(100)).await;
        }
    }
}
```

## Read Scaling

Since all nodes index the same WAL, horizontal scaling for reads is straightforward:

```
                    ┌─────────────────┐
                    │  Load Balancer  │
                    └─────────────────┘
                      /      |      \
                     ▼       ▼       ▼
              ┌──────────┐ ┌──────────┐ ┌──────────┐
              │  Node 1  │ │  Node 2  │ │  Node 3  │
              │ block: N │ │ block: N │ │block: N-1│
              └──────────┘ └──────────┘ └──────────┘
                     \       |       /
                      \      |      /
                       ▼     ▼     ▼
                 Cold Storage Service
```

**Properties:**
- All nodes are identical (no primary/replica distinction)
- Each node indexes independently
- Nodes may be at slightly different block heights
- Add nodes to increase read throughput

### Node Selection

```rust
struct NodePool {
    nodes: Vec<NodeInfo>,
}

struct NodeInfo {
    id: String,
    endpoint: String,
    current_block: BlockNumber,
    healthy: bool,
}

impl NodePool {
    fn select_node(&self, consistency: ReadConsistency) -> Option<&NodeInfo> {
        self.nodes
            .iter()
            .filter(|n| n.healthy)
            .filter(|n| self.meets_consistency(n, &consistency))
            .choose(&mut rand::thread_rng())
    }
}
```

### Consistency Options

```rust
enum ReadConsistency {
    /// Any healthy node, fastest response
    Any,

    /// Node must be at least this block
    AtLeast(BlockNumber),

    /// Node must be at chain head (±1 block)
    Latest,
}
```

**Usage:**
```rust
// Fast read, may be slightly stale
client.query(query, ReadConsistency::Any)

// After submitting an edit, ensure you see it
client.query(query, ReadConsistency::AtLeast(submitted_block))
```

### Scaling Characteristics

| Nodes | Read Throughput | Notes |
|-------|-----------------|-------|
| 1 | Baseline | Single point of failure |
| 3 | ~3x | Good for HA |
| 10 | ~10x | Linear scaling |
| N | Diminishes | Cold storage becomes bottleneck |

## Benefits of Disaggregation

| Concern | Coupled Architecture | Disaggregated |
|---------|---------------------|---------------|
| **IPFS latency** | Blocks indexing | Hidden behind cache |
| **IPFS failures** | Indexer fails | Cache retries, compute unaffected |
| **New node startup** | Fetch all from IPFS (slow) | Fetch from cache (fast) |
| **Multiple consumers** | Each fetches from IPFS | Share single cache |
| **IPFS rate limits** | Each node hits limits | Single service manages rate |
| **Compute scaling** | Limited by IPFS | Independent scaling |

## High Availability

### Node Failure

- Load balancer detects unhealthy node
- Routes traffic to remaining nodes
- Failed node restarts, loads checkpoint, replays from cold storage
- Rejoins pool when caught up

### Cold Storage Failure

- Compute nodes continue serving from hot storage
- New writes queue until cold storage recovers
- Cold storage is simpler service, easier to make redundant

### Deployment Strategy

**Rolling updates:**
1. Remove node from load balancer
2. Update node
3. Node replays from checkpoint
4. Add back to load balancer
5. Repeat for next node

**Blue-green:**
1. Spin up new node pool
2. Wait for all nodes to sync
3. Switch load balancer to new pool
4. Tear down old pool

## Future Considerations

### Sharding

If data exceeds single-node RAM, shard by space:

```
         Router (by space_id)
              /         \
             ▼           ▼
        Shard A       Shard B
       (Spaces       (Spaces
        1-1000)      1001-2000)
```

Each shard:
- Filters WAL for relevant spaces
- Holds subset of data in memory
- Scales independently

**Cross-space references:** Entities can reference entities in other spaces, which complicates sharding. Strategies to handle this:

| Strategy | Description |
|----------|-------------|
| **Global replication** | Core types/properties replicated to all shards |
| **Reference cache** | LRU cache of popular cross-shard entities |
| **Scatter-gather** | Router fetches from multiple shards, assembles result |
| **Client hints** | Let queries specify: resolve fully, cached-only, or IDs-only |

Most cross-space references hit a small set of popular entities (types, properties, well-known entities). Cache those locally, scatter-gather for the long tail.

### Real-Time Subscriptions

For collaborative/multiplayer use cases:

```rust
struct SubscriptionManager {
    subscriptions: HashMap<QueryHash, Vec<SubscriberId>>,
    query_results: HashMap<QueryHash, HashSet<EntityId>>,
}

impl SubscriptionManager {
    fn on_block_applied(&mut self, changed_entities: &[EntityId]) {
        for (query_hash, subscribers) in &self.subscriptions {
            let old_results = self.query_results.get(query_hash);
            let new_results = self.engine.execute(query);

            let delta = compute_delta(old_results, &new_results);
            if !delta.is_empty() {
                for subscriber in subscribers {
                    self.send_delta(subscriber, &delta);
                }
            }
        }
    }
}
```

### Geographic Distribution

For global latency optimization:

```
                US Users              EU Users
                    │                     │
                    ▼                     ▼
            ┌─────────────┐       ┌─────────────┐
            │ US Nodes    │       │ EU Nodes    │
            └─────────────┘       └─────────────┘
                    \                 /
                     \               /
                      ▼             ▼
               ┌─────────────────────────┐
               │   Cold Storage Service   │
               │   (globally replicated)  │
               └─────────────────────────┘
                          │
                          ▼
                    Blockchain WAL
```

Each region:
- Has local compute nodes
- Reads from nearest cold storage replica
- Same consistency guarantees (blockchain is global)
