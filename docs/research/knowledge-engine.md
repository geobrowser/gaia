# Knowledge Engine

This document describes the Geo Knowledge Engine architecture, including the data model, storage design, and system architecture.

## Table of Contents

- [Overview](#overview)
- [Data Model](#data-model)
- [Operations](#operations)
- [System Architecture](#system-architecture)
- [Storage Design](#storage-design)
- [Durable Storage](#durable-storage)
- [Query Engine](#query-engine)
- [Scaling](#scaling)
- [Open Areas](#open-areas)

## Overview

The Geo Knowledge Graph is a **distributed, content-addressed database** with blockchain as the consensus/ordering layer. It implements an **Entity-Attribute-Value (EAV) store** with first-class relations—essentially a **property graph database** with blockchain-ordered writes.

The system uses a **disaggregated storage architecture** where compute and storage are separated into independent, scalable layers, similar to modern cloud databases that separate compute from object storage.

### Key Characteristics

| Aspect | Our System | Traditional DB |
|--------|------------|----------------|
| **Ordering** | Block + log index (total order) | Transaction timestamp / MVCC |
| **Conflict resolution** | Last-write-wins (per space) | Locks / optimistic concurrency |
| **Schema** | Emergent (properties are entities) | Declared upfront |
| **Query** | GraphQL over Postgres | SQL / Cypher / SPARQL |
| **Write path** | Blockchain → IPFS → Indexer → Postgres | Direct writes |
| **Consistency** | Eventual (indexer lag) | Strong (ACID) |

### Database Architecture Mapping

| Database Component | Purpose | Our System | Status |
|-------------------|---------|------------|--------|
| **WAL** | Ordered, durable record of changes | Blockchain + IPFS | ✅ |
| **Storage Engine** | Persistent storage | IPFS (raw), Postgres (materialized) | ✅ |
| **Buffer Pool / Cache** | Fast access to hot data | Postgres | ✅ |
| **Query Processor** | Parse and execute queries | GraphQL API | ✅ |
| **Query Optimizer** | Efficient execution plans | Postgres planner (implicit) | ⚠️ Partial |
| **Catalog / System Tables** | Schema metadata | Properties as entities | ✅ |
| **Index Manager** | Secondary indexes | Postgres indexes | ✅ |
| **Transaction Manager** | Ordering, concurrency | Block ordering + LWW | ✅ |
| **Lock Manager** | Concurrency control | N/A (append-only) | ✅ N/A |
| **Recovery Manager** | Restore from WAL | Re-index from blockchain | ✅ |
| **Checkpoint Manager** | Snapshots for faster recovery | — | ❌ Missing |
| **Replication Manager** | Multi-node consistency | IPFS pinning + indexers | ⚠️ Informal |

## Data Model

```
┌─────────────────────────────────────────────────────────────┐
│                        SPACE (namespace)                    │
│  - Access control boundary (smart contract enforced)        │
│  - Emergent schema context                                  │
├─────────────────────────────────────────────────────────────┤
│  ENTITY (node)                                              │
│  ├── VALUES (attributes)                                    │
│  │   └── propertyId → typed value (string|number|bool|...)  │
│  └── RELATIONS (edges)                                      │
│      └── typeId → (fromEntity, toEntity, position)          │
├─────────────────────────────────────────────────────────────┤
│  PROPERTY (attribute/relation type definition)              │
│  - Also an entity (self-describing)                         │
│  - Defines the "schema" implicitly                          │
└─────────────────────────────────────────────────────────────┘
```

### Data Characteristics

```
500k entities × ~12 values avg × ~50 bytes/value = ~300 MB values
500k entities × ~10 relations avg × ~100 bytes/relation = ~500 MB relations
Property metadata: negligible

Current state total: ~1 GB (fits comfortably in RAM)
At 10x scale (5M entities): ~10 GB (still feasible for single node)
```

### Entity Shape

- Entities are narrow: 5-20 values, 0-20 relations each
- Values and relations per entity are stable
- Most queries want full entity with all values/relations

## Operations

Defined in `wire/proto/grc20.proto`. Each Edit contains a batch of Ops that apply diffs to the graph.

| Op | Payload | Effect |
|----|---------|--------|
| `update_entity` | `Entity{id, values[]}` | Upsert entity with values (merge semantics) |
| `create_relation` | `Relation{...}` | Create a new relation |
| `update_relation` | `RelationUpdate{id, ...}` | Modify existing relation fields |
| `delete_relation` | `bytes` (id) | Remove a relation |
| `create_property` | `Property{id, data_type}` | Define a new property type |
| `unset_entity_values` | `{id, properties[]}` | Remove specific values from entity |
| `unset_relation_fields` | `{id, field_flags...}` | Clear optional fields on relation |

### Op Semantics

**Entities:**
- Created implicitly via `update_entity`
- No hard delete (append-only for indexer parallelization)
- "Deletion" = unset all values + remove all relations

**Properties:**
- Immutable `id` and `data_type` once created
- Mutable metadata (name, display hints) since properties are entities
- No data type migrations—create new property instead

**Values:**
- Merge semantics: `update_entity` upserts per (entity_id, property_id)
- Explicit `unset_entity_values` required to remove

**Relations:**
- Multi-edge: `(from, type, to)` is NOT unique—relation `id` is the key
- Supports multiple relations of same type between same entities
- Position field enables ordering

**Cross-Space References:**
- Global entity namespace (UUIDs)
- Entities can reference entities in other spaces
- `to_space` is a hint/constraint, not enforced referential integrity
- Relation rot possible (referenced entity may change or be "deleted")

### Schema Evolution

The system is **schemaless by default**. Communities create emergent schemas through convention.

Properties have:
- **Immutable:** `id`, `data_type` (Number, String, Date, etc.)
- **Mutable:** `name`, `display_hints` (float vs int vs currency), other metadata

Rather than traditional migrations, the system supports **schema evolution**:

| Change | Approach |
|--------|----------|
| Rename property | Update property entity's name |
| Change display format | Update property entity's display hints |
| Deprecate property | Mark deprecated in property metadata |
| Replace with new property | Create new property, optionally backfill, mark old as deprecated |

Old and new properties can coexist. Query layer handles heterogeneous data.

**Backfill cost:** Gas costs don't scale with data (Edit contents on IPFS, only hash on chain). Main costs are WAL growth and indexer processing time.

## System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLIENTS                                 │
│                      (GraphQL queries)                          │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      QUERY PROCESSOR                            │
│                   (GraphQL resolvers)                           │
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
│  │   Parser    │  │  Executor   │  │  "Optimizer" (Postgres) │ │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     MATERIALIZED STATE                          │
│                       (Postgres)                                │
│                                                                 │
│  ┌──────────┐ ┌──────────┐ ┌───────────┐ ┌──────────────────┐  │
│  │ entities │ │  values  │ │ relations │ │ properties/spaces│  │
│  └──────────┘ └──────────┘ └───────────┘ └──────────────────┘  │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                      INDEXES                             │   │
│  │   B-tree, GIN (trigram), composite indexes               │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              ▲
                              │
┌─────────────────────────────────────────────────────────────────┐
│                         INDEXER                                 │
│              (WAL consumer / state builder)                     │
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
│  │ Block cursor│  │ Op applier  │  │  IPFS fetcher           │ │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
                              ▲
                              │
┌─────────────────────────────────────────────────────────────────┐
│                      WAL (Source of Truth)                      │
│                                                                 │
│  ┌─────────────────────┐    ┌────────────────────────────────┐ │
│  │     Blockchain      │    │             IPFS               │ │
│  │  (ordered hashes)   │───▶│      (Edit contents)           │ │
│  └─────────────────────┘    └────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Disaggregated Architecture

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

### Storage Temperature Hierarchy

| Layer | Location | Access Time | Contents | Persistence |
|-------|----------|-------------|----------|-------------|
| **Hot** | RAM | <1μs | Current state, indexes, query cache | Lost on restart |
| **Warm** | Local SSD | ~1ms | Checkpoints, history segments | Survives restart |
| **Cold** | Remote service | ~10ms | Cached IPFS edits | Shared across nodes |
| **Frozen** | IPFS | 100ms-10s | Raw edit data (origin) | Permanent, immutable |

## Storage Design

### Why Entity-Centric Storage

Traditional EAV (like Postgres):
```sql
SELECT * FROM values WHERE entity_id = ?;
SELECT * FROM relations WHERE entity_id = ?;
-- Multiple joins to reconstruct entity
```

Entity-centric storage stores entities with values and relations inline:
```rust
struct Entity {
    id: Uuid,
    values: Vec<Value>,
    relations_out: Vec<Relation>,
    created_at_block: u64,
    updated_at_block: u64,
}
```

**Benefit:** One hashmap lookup returns complete entity. Zero joins.

### In-Memory Primary Store

```
┌─────────────────────────────────────────────────────────────────┐
│                     PRIMARY STORE (In-Memory)                   │
│                        Current State Only                       │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ entities: HashMap<EntityId, Entity>                      │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ INDEXES                                                  │   │
│  │                                                          │   │
│  │ by_space: HashMap<SpaceId, HashSet<EntityId>>            │   │
│  │ by_name: HashMap<(SpaceId, String), HashSet<EntityId>>   │   │
│  │ by_prop_value: HashMap<(PropId, Value), HashSet<EntityId>>│  │
│  │ by_relation: HashMap<(RelTypeId, ToEntityId), HashSet<EntityId>>│
│  │ relations_in: HashMap<EntityId, Vec<Relation>>           │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ METADATA                                                 │   │
│  │ properties: HashMap<PropertyId, PropertyMeta>            │   │
│  │ spaces: HashMap<SpaceId, SpaceMeta>                      │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### History Store (On-Disk)

```
┌─────────────────────────────────────────────────────────────────┐
│                   HISTORY STORE (On-Disk)                       │
│                     Append-Only Segments                        │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ Segment files (one per block range):                     │   │
│  │   blocks_0_10000.segment                                 │   │
│  │   blocks_10001_20000.segment                             │   │
│  │   ...                                                    │   │
│  │                                                          │   │
│  │ Format: sorted by (entity_id, block)                     │   │
│  │ [entity_id][block][entity snapshot or delta]             │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ INDEXES                                                  │   │
│  │ block_index: block_number → segment file + offset        │   │
│  │ entity_block_index: (entity_id, block) → segment + offset│   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Performance Characteristics

| Operation | Complexity | Notes |
|-----------|------------|-------|
| Get entity by ID | O(1) | HashMap lookup |
| Get entity values | O(1) | Inline in entity |
| Get entity relations | O(1) | Inline in entity |
| Search by name in space | O(1) | Index lookup |
| Filter by property=value | O(1) | Index lookup |
| Filter by relation | O(1) | Index lookup |
| Filter + order (same prop) | O(n log n) | n = filtered candidates |
| Filter + order (traversed) | O(n × m log n) | m = relations per entity |
| Historical query | O(log n) | Binary search in segment |
| Apply op | O(k) | k = number of index updates |
| Snapshot save | O(n) | n = total entities |
| Snapshot load | O(n) | Plus index rebuild |

## Durable Storage

### Content Addressing

The key abstraction is the **CID (Content Identifier)**—a hash of the content itself.

```
Location-addressed:  s3://my-bucket/edits/abc123
                     https://api.geo.io/edits/abc123
                     ↑ Tied to provider, can break

Content-addressed:   bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oc...
                     ↑ Hash of content, works anywhere
```

Benefits:

| Benefit | How CID Enables It |
|---------|-------------------|
| **Provider independence** | Switch S3 → R2 → GCS without changing identifiers |
| **Verification** | Any provider's data can be verified against CID |
| **Redundancy** | Same CID stored in multiple places |
| **Migration** | Move data between providers, CIDs stay same |
| **Caching** | Cache anywhere, CID guarantees correctness |
| **Decentralization ready** | Add IPFS/Arweave anytime, same CIDs work |

### Multi-Provider Architecture

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

### Why Not Pure IPFS?

IPFS provides content addressing, but has operational challenges:

| Requirement | IPFS Provides | What We Need |
|-------------|---------------|--------------|
| Write latency | Seconds to minutes | <100ms |
| Read latency | Unpredictable | <10ms |
| Availability | Best effort | 99.9%+ |
| Durability | Only if pinned | Guaranteed |

**Solution:** Use CIDs as the identifier format, but store in multiple providers with different characteristics.

### Durability Strategy

1. **Primary (S3):** Immediate storage, 11 nines durability, fast reads
2. **Secondary (IPFS):** Async backup, decentralized, content-addressed network
3. **Tertiary (Arweave):** Optional, pay-once permanent storage

### Recovery Scenarios

| Scenario | Recovery Path |
|----------|---------------|
| S3 bucket deleted | Restore from IPFS/Arweave |
| IPFS data unpinned | Still in S3, re-pin |
| Provider goes down | Failover to other providers |
| All providers fail | Rebuild from blockchain (CIDs) + any surviving provider |

## Query Engine

The query engine supports arbitrary filtering and ordering without requiring pre-defined indexes for every possible query pattern.

### Design Philosophy

Rather than pre-indexing all possible orderings (which would be combinatorially explosive), we:

1. **Index for filtering** - reduce candidate set using available indexes
2. **Sort in memory** - fast for reduced candidate sets
3. **Cache results** - avoid re-sorting for pagination and repeated queries

### Query Patterns

| Query | Frequency | Access Pattern |
|-------|-----------|----------------|
| Entity + all values + property names | Hot | Point lookup → inline values → property metadata |
| Entity + relations + to_entity names | Hot | Point lookup → inline relations → point lookups |
| Search by name in space | Hot | Index scan: `(space_id, name) → entity_ids` |
| Filter by property=value | Hot | Index scan: `(property_id, value) → entity_ids` |
| Filter by relation type + to_entity | Hot | Index scan: `(relation_type, to_entity) → entity_ids` |
| Historical state at block X | Cold | Segment lookup, binary search |

### Ordering Complexity

| Ordering Type | Complexity | Notes |
|---------------|------------|-------|
| By direct property | O(n log n) | n = filtered candidates |
| By traversed property | O(n × m log n) | m = avg relations per entity |
| By count | O(n × m log n) | m = avg relations per entity |

**Key insight:** We cannot use indexes to avoid sorting for arbitrary orderings. Instead, we rely on:

1. Filters being selective (reducing n significantly)
2. Caching sorted results (amortizing sort cost across pagination)
3. Failing gracefully when result sets are too large

## Scaling

### Read Scaling

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

### Scaling Characteristics

| Nodes | Read Throughput | Notes |
|-------|-----------------|-------|
| 1 | Baseline | Single point of failure |
| 3 | ~3x | Good for HA |
| 10 | ~10x | Linear scaling |
| N | Diminishes | Cold storage becomes bottleneck |

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

### Benefits of Disaggregation

| Concern | Coupled Architecture | Disaggregated |
|---------|---------------------|---------------|
| **IPFS latency** | Blocks indexing | Hidden behind cache |
| **IPFS failures** | Indexer fails | Cache retries, compute unaffected |
| **New node startup** | Fetch all from IPFS (slow) | Fetch from cache (fast) |
| **Multiple consumers** | Each fetches from IPFS | Share single cache |
| **IPFS rate limits** | Each node hits limits | Single service manages rate |
| **Compute scaling** | Limited by IPFS | Independent scaling |

### Future: Sharding

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

**Cross-space references:** Entities can reference entities in other spaces. Strategies:

| Strategy | Description |
|----------|-------------|
| **Global replication** | Core types/properties replicated to all shards |
| **Reference cache** | LRU cache of popular cross-shard entities |
| **Scatter-gather** | Router fetches from multiple shards, assembles result |
| **Client hints** | Let queries specify: resolve fully, cached-only, or IDs-only |

## Open Areas

### Checkpointing

Currently, recovery requires re-indexing from block 0. Checkpoints would enable:
- Faster recovery (start from checkpoint, not genesis)
- Snapshot verification (merkle root)
- New indexer bootstrapping

### Query Optimization

GraphQL resolvers may have:
- N+1 queries on relation traversals
- Inefficient cross-space joins
- No cost-based query planning

### Replication Consistency

Multiple indexers can exist, but:
- No formal consistency verification
- Clients may hit indexers at different block heights
- No protocol for indexer coordination

### Garbage Collection

Append-only WAL grows forever. Options:
- Accept unbounded growth (history is valuable)
- Purge old `validToBlock IS NOT NULL` rows (if history window is bounded)
- Checkpoint + prune (keep snapshots, discard intermediate states)

### Time-Travel / Historical Queries

Add block ranges to values and relations tables:

```typescript
values: {
  // ... existing fields ...
  validFromBlock: text,      // block this value became active
  validToBlock: text | null, // null = current, set when superseded
}
```

**Query current state:**
```sql
SELECT * FROM values WHERE entityId = ? AND validToBlock IS NULL
```

**Query at block X:**
```sql
SELECT * FROM values
WHERE entityId = ?
  AND validFromBlock <= X
  AND (validToBlock IS NULL OR validToBlock > X)
```

## Related Documents

- [Hermes Architecture](./hermes-architecture.md) - Event streaming from blockchain
- [Atlas Documentation](../atlas/docs/) - Canonical graph computation
