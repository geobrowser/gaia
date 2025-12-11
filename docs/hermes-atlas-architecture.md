# Hermes Architecture

This document describes the architecture of Hermes, the system that ingests blockchain data, transforms it, and emits normalized events to Kafka.

## Overview

Hermes is an umbrella system composed of independent transformers that process blockchain events. Each transformer:

- Connects to the blockchain data source via a shared library (relay)
- Filters for events it cares about
- Applies its transformation logic
- Emits to its Kafka topic
- Maintains its own cursor for independent restart/replay

The system receives blockchain events which fall into several categories:

1. **Space changes** - creations, topology changes
2. **Edit publishing** - content modifications (contain IPFS hashes pointing to actual content)
3. **Governance** - proposals, voting, membership changes (future)
4. **Curation** - ranking, voting on entities (future)

```
                              ┌─────────────────────────────────────────────────────┐
                              │  Hermes                                             │
                              │                                                     │
                              │  ┌─────────────────────────────────────────────┐    │
Blockchain ───────────────────┼─▶│  relay (lib)                                │    │
Data Source                   │  │  - Connect to data source                   │    │
(globally ordered)            │  │  - Cursor/checkpoint management             │    │
                              │  │  - Decode raw bytes → typed events          │    │
                              │  └───────────────┬─────────────────────────────┘    │
                              │                  │                                  │
                              │    ┌─────────────┼─────────────┬───────────────┐    │
                              │    │             │             │               │    │
                              │    ▼             ▼             ▼               ▼    │
                              │ ┌──────┐    ┌──────┐    ┌──────────┐    ┌─────────┐ │
                              │ │spaces│    │edits │    │ topology │    │ future  │ │
                              │ │(bin) │    │(bin) │    │  (bin)   │    │  ...    │ │
                              │ └──┬───┘    └──┬───┘    └────┬─────┘    └────┬────┘ │
                              │    │           │             │               │      │
                              └────┼───────────┼─────────────┼───────────────┼──────┘
                                   │           │             │               │
                                   ▼           ▼             ▼               ▼
                                Kafka:      Kafka:        Kafka:          Kafka:
                                spaces      edits        topology         ...

                              ┌────────────────────────────────────────┐
Blockchain ──────────────────▶│  IPFS Cache (parallel, ahead-of-time) │
Data Source                   └────────────────────────────────────────┘
```

## Design Principles

### Independent Transformers

Each transformer is a separate binary with its own:
- Connection to the data source (via relay)
- Cursor/checkpoint
- Kafka producer

This enables:
- **Independent deployments** - Fix a bug in edits without restarting spaces
- **Independent replay** - Reprocess edits from block 1M while spaces continues from current
- **Failure isolation** - If edits crashes, spaces keeps running

### Shared Libraries

**`hermes-relay`** provides shared infrastructure for data source access:
- Connection setup to blockchain data source
- Cursor/checkpoint persistence

**`wire`** provides event decoding:
- Protobuf definitions for blockchain events
- Decoding raw bytes → typed events (GeoOutput, EditPublished, etc.)

This separation keeps transformers independent while sharing common logic. If the underlying data source changes (e.g., swap substreams for something else), only relay changes. If event formats change, only wire changes.

## Ordering Guarantees

| Event Type | Ordering Requirement |
|------------|---------------------|
| Edits | Globally ordered within edits (diffs depend on prior state) |
| Space/Governance/Curation | Globally ordered (data source order) |
| Canonical Graph (topology) | Ordered per emission (independent stream) |

Edit events must maintain global ordering because edits are diffs - each edit depends on the state established by prior edits.

## Components

### relay (Library)

Shared infrastructure for connecting to the blockchain data source.

**Provides:**
- Connection setup (currently Substreams, but abstracted)
- Cursor/checkpoint management
- Event decoding (raw bytes → typed events)
- Typed event stream for transformers to consume

**Typed Events:**
```rust
enum GeoEvent {
    SpaceCreated(SpaceCreated),
    TrustExtended(TrustExtended),
    EditPublished(EditPublished),
    // ... governance, curation (future)
}
```

### spaces (Binary)

Transforms space events and emits to Kafka.

**Uses:** `relay`  
**Cursor:** `hermes-spaces`  
**Output:** `spaces` Kafka topic

**Handles:**
- Space creation events
- Trust extension events (Verified, Related, Subtopic)

| Event Type | Protobuf Message |
|------------|------------------|
| SpaceCreated | `HermesCreateSpace` |
| TrustExtended | `HermesSpaceTrustExtension` |

### edits (Binary)

Transforms edit events, resolving IPFS content via cache, and emits to Kafka.

**Uses:** `relay`, IPFS cache  
**Cursor:** `hermes-edits`  
**Output:** `edits` Kafka topic

**How it works:**
1. Receives `EditPublished` event (contains IPFS CID)
2. Reads resolved content from IPFS cache
3. Decodes into `Edit` protobuf (ops, authors, etc.)
4. Enriches with blockchain metadata
5. Emits `HermesEdit` to Kafka

### topology (Binary) [Atlas]

Computes the canonical graph from space topology events.

**Uses:** `relay`  
**Cursor:** `hermes-topology`  
**Output:** `topology` Kafka topic

**Key modules:**
- `GraphState` - Stores all spaces, edges, and topic memberships
- `TransitiveProcessor` - Computes reachable spaces from any root
- `CanonicalProcessor` - Filters to only canonical (trusted) spaces

### IPFS Cache Service

Pre-populates resolved IPFS contents ahead of time so the edits transformer doesn't block on network I/O.

**Location:** `cache/`

**How it works:**
1. Connects to the blockchain data source (parallelized, runs ahead)
2. For each edit event, fetches the IPFS content by CID
3. Stores resolved content in the cache

**Cache miss behavior:** If edits encounters a cache miss, it waits and retries until the content appears. The cache should always be ahead, so misses indicate the cache is catching up.

### hermes-schema (Library)

Protobuf definitions for Kafka output messages.

**Location:** `hermes-schema/`

**Protos:**
- `knowledge.proto` - HermesEdit message
- `space.proto` - HermesCreateSpace, HermesSpaceTrustExtension
- `topology.proto` - CanonicalGraphUpdated, CanonicalTreeNode
- `blockchain_metadata.proto` - Common metadata fields

### mock-substream (Library)

Generates deterministic blockchain events for testing.

**Location:** `mock-substream/`

**Exports:**
- `MockEvent` - Enum of event types (SpaceCreated, TrustExtended, EditPublished)
- `test_topology::generate()` - Generates deterministic test topology
- Well-known IDs for spaces, topics, entities, etc.

## Event Types

### SpaceCreated

A new space is created on-chain.

```
SpaceCreated {
    space_id: [u8; 16],      // Unique space identifier
    topic_id: [u8; 16],      // Topic this space announces
    space_type: SpaceType,   // Personal or DAO
}
```

### TrustExtended

A space extends trust to another space or topic.

```
TrustExtended {
    source_space_id: [u8; 16],
    extension: TrustExtension,
}

TrustExtension:
  - Verified { target_space_id }  // Strong trust
  - Related { target_space_id }   // Weaker association
  - Subtopic { target_topic_id }  // Subscribe to topic
```

### EditPublished

An edit (set of GRC-20 operations) is published to a space.

```
EditPublished {
    edit_id: [u8; 16],
    space_id: [u8; 16],
    authors: Vec<Address>,
    name: String,
    ops: Vec<Op>,           // GRC-20 operations
}
```

## Canonical Graph

The canonical graph represents the "trusted" portion of the knowledge graph.

### Rules

1. The **root space** is always canonical
2. A space is canonical if reachable from root via **explicit edges only** (Verified or Related)
3. **Topic edges** can attach subtrees, but only canonical members are included

### Example Topology

```
CANONICAL (reachable from Root):

  Root
   ├─verified─▶ A ─verified─▶ C ─verified─▶ F
   │             │              └─related─▶ G
   │             └─related─▶ D
   ├─verified─▶ B ─verified─▶ E
   └─related─▶ H ─verified─▶ I
                └─verified─▶ J

NON-CANONICAL (isolated islands):

  Island 1: X ─▶ Y ─▶ Z
             └─▶ W

  Island 2: P ─▶ Q

  Island 3: S (isolated)
```

### Topic Edge Resolution

When a canonical space has a topic edge:

1. Find all spaces that announce that topic
2. Filter to only canonical members
3. Include their subtrees in the canonical graph

```
B ─topic[T_H]─▶ resolves to H (canonical)
                 └─▶ includes H's subtree {I, J}

A ─topic[T_SHARED]─▶ resolves to {C, G} (canonical)
                      └─▶ Y is filtered out (non-canonical)
```

## Kafka Topics

| Topic | Producer | Message Type | Description |
|-------|----------|--------------|-------------|
| `spaces` | spaces | HermesCreateSpace, HermesSpaceTrustExtension | Space creation and trust changes |
| `edits` | edits | HermesEdit | Resolved knowledge graph edits |
| `topology` | topology | CanonicalGraphUpdated | Canonical graph updates |
| `governance` | (future) | (TBD) | Proposals, voting, membership |
| `curation` | (future) | (TBD) | Ranking, voting on entities |

## Crate Structure

```
hermes/
├── relay/           # Shared library: data source connection, event decoding
├── spaces/          # Binary: space event transformer
├── edits/           # Binary: edit event transformer (uses IPFS cache)
├── topology/        # Binary: canonical graph computation (Atlas)
├── schema/          # Library: Kafka output protobuf definitions
└── cache/           # Service: IPFS content pre-fetcher
```

Each binary depends on `relay` for data source access and `schema` for output types.

## Data Flow Example

### Space and Topology Events

1. **Data source** emits: `SpaceCreated(Root)`, `SpaceCreated(A)`, `TrustExtended(Root→A)`

2. **spaces** (via relay):
   - Filters for space events
   - Converts to `HermesCreateSpace` and `HermesSpaceTrustExtension` protos
   - Publishes to `spaces` topic
   - Updates cursor `hermes-spaces`

3. **topology** (via relay, independent):
   - Filters for topology events
   - Updates `GraphState` with new space and edge
   - Recomputes canonical graph (Root + A are now canonical)
   - Publishes updated graph to `topology` topic
   - Updates cursor `hermes-topology`

### Edit Events

1. **Data source** emits: `EditPublished { space_id, ipfs_cid }`

2. **IPFS Cache Service** (running ahead):
   - Already fetched content for `ipfs_cid`
   - Content stored in cache

3. **edits** (via relay):
   - Filters for edit events
   - Reads resolved content from IPFS cache (waits if miss)
   - Decodes into `Edit` protobuf (ops, authors, etc.)
   - Enriches with blockchain metadata
   - Publishes `HermesEdit` to `edits` topic
   - Updates cursor `hermes-edits`

4. **Downstream consumers** read from Kafka topics to:
   - Update search indices
   - Trigger notifications
   - Sync to other databases

## Deployment

Each transformer runs as an independent service:

```bash
# Local development
docker-compose up

# Starts:
# - Kafka broker (localhost:9092)
# - Kafka UI (http://localhost:8080)
# - spaces transformer
# - edits transformer
# - topology transformer
# - IPFS cache service
```

### Independent Operations

```bash
# Restart only edits (e.g., after bug fix)
kubectl rollout restart deployment/hermes-edits

# Replay spaces from specific block
kubectl set env deployment/hermes-spaces START_BLOCK=1000000
kubectl rollout restart deployment/hermes-spaces

# topology continues unaffected
```
