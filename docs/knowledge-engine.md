# Knowledge Engine Architecture

This document describes the Geo Knowledge Graph as a distributed database, mapping traditional database concepts to our architecture.

## Overview

The Geo Knowledge Graph is a **distributed, content-addressed database** with blockchain as the consensus/ordering layer. It implements an **Entity-Attribute-Value (EAV) store** with first-class relations—essentially a **property graph database** with blockchain-ordered writes.

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

### Key Characteristics

| Aspect | Our System | Traditional DB |
|--------|------------|----------------|
| **Ordering** | Block + log index (total order) | Transaction timestamp / MVCC |
| **Conflict resolution** | Last-write-wins (per space) | Locks / optimistic concurrency |
| **Schema** | Emergent (properties are entities) | Declared upfront |
| **Query** | GraphQL over Postgres | SQL / Cypher / SPARQL |
| **Write path** | Blockchain → IPFS → Indexer → Postgres | Direct writes |
| **Consistency** | Eventual (indexer lag) | Strong (ACID) |

## Operations (Ops)

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

## Schema Evolution

The system is **schemaless by default**. Communities create emergent schemas through convention.

### Property Metadata

Properties have:
- **Immutable:** `id`, `data_type` (Number, String, Date, etc.)
- **Mutable:** `name`, `display_hints` (float vs int vs currency), other metadata

Since properties are entities, property metadata is automatically versioned.

### Evolution vs Migration

Rather than traditional migrations, the system supports **schema evolution**:

| Change | Approach |
|--------|----------|
| Rename property | Update property entity's name |
| Change display format | Update property entity's display hints |
| Deprecate property | Mark deprecated in property metadata |
| Replace with new property | Create new property, optionally backfill, mark old as deprecated |

Old and new properties can coexist. Query layer handles heterogeneous data.

**Backfill cost:** Gas costs don't scale with data (Edit contents on IPFS, only hash on chain). Main costs are WAL growth and indexer processing time.

## Database Architecture Mapping

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

## Time-Travel / Historical Queries

### Current State

- Entities track `createdAt`, `createdAtBlock`, `updatedAt`, `updatedAtBlock`
- Properties are entities, so property metadata is versioned
- WAL provides complete history

### Proposed Implementation

Add block ranges to values and relations tables:

```typescript
values: {
  // ... existing fields ...
  validFromBlock: text,      // block this value became active
  validToBlock: text | null, // null = current, set when superseded
}

relations: {
  // ... existing fields ...
  validFromBlock: text,
  validToBlock: text | null,
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
