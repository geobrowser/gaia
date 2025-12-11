# Hermes & Atlas Architecture

This document describes the architecture of the Hermes and Atlas event processing systems.

## Overview

Hermes and Atlas are parallel consumers of blockchain events that process space topology and knowledge graph edits. The system receives blockchain events through Substreams, which fall into several categories:

1. **Space changes** - creations, topology changes
2. **Edit publishing** - content modifications (contain IPFS hashes pointing to actual content)
3. **Governance** - proposals, voting, membership changes (future)
4. **Curation** - ranking, voting on entities (future)

```
                                    ┌────────────────────────────────────┐
                                    │  Hermes                            │
                                    │                                    │
                              ┌────▶│  Edit Events ──▶ IPFS Cache ───────│──▶ Kafka: edits
                              │     │                  (wait if miss)    │
Substreams ───────────────────┤     │                                    │
(globally ordered)            │     │  Space Events ─────────────────────│──▶ Kafka: spaces
                              │     │                                    │
                              │     └────────────────────────────────────┘
                              │
                              │     ┌────────────────────────────────────┐
                              └────▶│  Atlas                             │
                                    │                                    │──▶ Kafka: topology
                                    │  Topology ──▶ Canonical Graph      │
                                    └────────────────────────────────────┘

                              ┌────────────────────────────────────────┐
Substreams ──────────────────▶│  IPFS Cache (parallel, ahead-of-time) │
                              └────────────────────────────────────────┘
```

## Ordering Guarantees

| Event Type | Ordering Requirement |
|------------|---------------------|
| Edits | Globally ordered within edits (diffs depend on prior state) |
| Space/Governance/Curation | Globally ordered (substream order) |
| Canonical Graph (Atlas) | Ordered per emission (independent stream) |

Edit events must maintain global ordering because edits are diffs - each edit depends on the state established by prior edits.

## Components

### IPFS Cache Service

Pre-populates resolved IPFS contents ahead of time so Hermes doesn't block on network I/O.

**Location:** `cache/`

**How it works:**
1. Consumes the same Substream (parallelized)
2. For each edit event, fetches the IPFS content by CID
3. Stores resolved content in the cache
4. Runs ahead of Hermes so content is available when needed

**Cache miss behavior:** If Hermes encounters a cache miss, it waits and retries until the content appears. The cache should always be ahead, so misses indicate the cache is catching up.

### mock-substream (Library)

A shared library that generates deterministic blockchain events for testing. Both hermes-processor and atlas consume from this library to ensure consistent test data.

**Location:** `mock-substream/`

**Exports:**
- `MockEvent` - Enum of event types (SpaceCreated, TrustExtended, EditPublished)
- `test_topology::generate()` - Generates deterministic test topology
- Well-known IDs for spaces, topics, entities, etc.

### hermes-processor (Service)

Transforms raw substream events into Hermes protobuf messages and publishes to Kafka.

**Location:** `hermes-processor/`

**Input:** Substream events  
**Output:** Kafka topics

**Two transformation types:**

1. **Edit Resolution** (via IPFS Cache)
   - Edit events contain an IPFS hash (CID)
   - Content is fetched from the IPFS cache (pre-populated)
   - Decoded into the `Edit` protobuf format (see `wire/proto/grc20.proto`)
   - The `Edit` message contains: id, name, list of `Op`s (operations), authors, and optional language

2. **Space Events** (direct transformation)
   - Space creation and trust extension events
   - Normalized and emitted directly

| Event Type | Output Topic | Protobuf Message |
|------------|--------------|------------------|
| SpaceCreated | `spaces` | `HermesCreateSpace` |
| TrustExtended | `spaces` | `HermesSpaceTrustExtension` |
| EditPublished | `edits` | `HermesEdit` |

### atlas (Service)

Builds and maintains the canonical graph - the set of spaces that are "trusted" based on reachability from a root space. Atlas consumes the Substream independently from Hermes.

**Location:** `atlas/`

**Input:** Substream events (topology only - ignores edits)  
**Output:** `topology` Kafka topic

**Key modules:**
- `GraphState` - Stores all spaces, edges, and topic memberships
- `TransitiveProcessor` - Computes reachable spaces from any root
- `CanonicalProcessor` - Filters to only canonical (trusted) spaces

### hermes-schema (Library)

Protobuf definitions for Hermes messages.

**Location:** `hermes-schema/`

**Protos:**
- `knowledge.proto` - HermesEdit message
- `space.proto` - HermesCreateSpace, HermesSpaceTrustExtension
- `topology.proto` - CanonicalGraphUpdated, CanonicalTreeNode
- `blockchain_metadata.proto` - Common metadata fields

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
| `spaces` | hermes-processor | HermesCreateSpace, HermesSpaceTrustExtension | Space creation and trust changes |
| `edits` | hermes-processor | HermesEdit | Resolved knowledge graph edits |
| `topology` | atlas | CanonicalGraphUpdated | Canonical graph updates |
| `governance` | hermes-processor | (TBD) | Proposals, voting, membership (future) |
| `curation` | hermes-processor | (TBD) | Ranking, voting on entities (future) |

## Deployment

### Local Development

```bash
cd hermes
docker-compose up
```

This starts:
- Kafka broker (localhost:9092)
- Kafka UI (http://localhost:8080)
- hermes-processor
- atlas

### Kubernetes

Both services run as Jobs in the `kafka` namespace:

```bash
kubectl get jobs -n kafka
# hermes-processor
# atlas
```

Deployed via GitHub Actions on push to `main`.

## Data Flow Example

### Space and Topology Events

1. **Substream** emits: `SpaceCreated(Root)`, `SpaceCreated(A)`, `TrustExtended(Root→A)`

2. **hermes-processor**:
   - Converts to `HermesCreateSpace` and `HermesSpaceTrustExtension` protos
   - Publishes to `spaces` topic

3. **atlas** (parallel, independent consumer):
   - Updates `GraphState` with new space and edge
   - Recomputes canonical graph (Root + A are now canonical)
   - Publishes updated graph to `topology` topic

### Edit Events

1. **Substream** emits: `EditPublished { space_id, ipfs_cid }`

2. **IPFS Cache Service** (running ahead):
   - Already fetched content for `ipfs_cid`
   - Content stored in cache

3. **hermes-processor**:
   - Receives edit event
   - Reads resolved content from IPFS cache (waits if miss)
   - Decodes into `Edit` protobuf (ops, authors, etc.)
   - Enriches with blockchain metadata
   - Publishes `HermesEdit` to `edits` topic

4. **Downstream consumers** read from Kafka topics to:
   - Update search indices
   - Trigger notifications
   - Sync to other databases
