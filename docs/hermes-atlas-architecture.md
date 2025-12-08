# Hermes & Atlas Architecture

This document describes the architecture of the Hermes and Atlas event processing systems.

## Overview

Hermes and Atlas are parallel consumers of blockchain events that process space topology and knowledge graph edits:

```
                         Blockchain
                             │
                             ▼
                      ┌─────────────┐
                      │  Substream  │
                      └──────┬──────┘
                             │
              ┌──────────────┴──────────────┐
              │                             │
              ▼                             ▼
    ┌─────────────────┐           ┌─────────────────┐
    │hermes-processor │           │      atlas      │
    │                 │           │                 │
    │ Transforms to   │           │ Builds canonical│
    │ Hermes protos   │           │ graph           │
    └────────┬────────┘           └────────┬────────┘
             │                             │
             ▼                             ▼
    ┌─────────────────┐           ┌─────────────────┐
    │ space.creations │           │topology.canonical│
    │ space.trust.*   │           └─────────────────┘
    │ knowledge.edits │
    └─────────────────┘
```

## Components

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

**Input:** Mock substream events  
**Output:** Kafka topics

| Event Type | Output Topic | Protobuf Message |
|------------|--------------|------------------|
| SpaceCreated | `space.creations` | `HermesCreateSpace` |
| TrustExtended | `space.trust.extensions` | `HermesSpaceTrustExtension` |
| EditPublished | `knowledge.edits` | `HermesEdit` |

### atlas (Service)

Builds and maintains the canonical graph - the set of spaces that are "trusted" based on reachability from a root space.

**Location:** `atlas/`

**Input:** Mock substream events (topology only - ignores edits)  
**Output:** `topology.canonical` Kafka topic

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
| `space.creations` | hermes-processor | HermesCreateSpace | New space events |
| `space.trust.extensions` | hermes-processor | HermesSpaceTrustExtension | Trust relationship changes |
| `knowledge.edits` | hermes-processor | HermesEdit | Knowledge graph edits |
| `topology.canonical` | atlas | CanonicalGraph | Canonical graph updates |

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

1. **mock-substream** generates: `SpaceCreated(Root)`, `SpaceCreated(A)`, `TrustExtended(Root→A)`

2. **hermes-processor**:
   - Converts to `HermesCreateSpace` protos
   - Publishes to `space.creations` and `space.trust.extensions`

3. **atlas**:
   - Updates `GraphState` with new space and edge
   - Recomputes canonical graph (Root + A are now canonical)
   - Publishes updated graph to `topology.canonical`

4. **Downstream consumers** read from Kafka topics to:
   - Update search indices
   - Trigger notifications
   - Sync to other databases
