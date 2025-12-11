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

1. **Space changes** - registrations, migrations, subspace relationships
2. **Edit publishing** - content modifications (contain IPFS hashes pointing to actual content)
3. **Governance** - proposals, voting, membership changes
4. **Curation** - ranking, voting on entities

```
                              ┌──────────────────────────────────────────────────────────┐
                              │  Hermes                                                  │
                              │                                                          │
┌──────────────┐              │  ┌────────────────┐    ┌─────────────────────────────┐   │
│  Blockchain  │              │  │ hermes-        │    │  hermes-relay (lib)         │   │
│    (Geo)     │─────────────▶│  │ substream      │───▶│  - Connect to substream     │   │
└──────────────┘              │  │ (excluded from │    │  - Cursor/checkpoint mgmt   │   │
                              │  │  workspace)    │    │  - Typed event stream       │   │
                              │  └────────────────┘    └──────────────┬──────────────┘   │
                              │                                       │                  │
                              │                  ┌────────────────────┼────────────┐     │
                              │                  │                    │            │     │
                              │                  ▼                    ▼            ▼     │
                              │            ┌──────────┐        ┌──────────┐  ┌─────────┐ │
                              │            │  spaces  │        │  edits   │  │ future  │ │
                              │            │  (bin)   │        │  (bin)   │  │  ...    │ │
                              │            └────┬─────┘        └────┬─────┘  └────┬────┘ │
                              │                 │                   │             │      │
                              └─────────────────┼───────────────────┼─────────────┼──────┘
                                                │                   │             │
                                                ▼                   ▼             ▼
                                             Kafka:              Kafka:        Kafka:
                                             spaces              edits          ...

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

**`hermes-substream`** decodes raw Ethereum logs from the Space Registry contract:
- Filters for Action events from the Space Registry
- Provides raw actions and pre-filtered typed event modules
- Runs on Substreams infrastructure (excluded from Cargo workspace)

**`hermes-relay`** provides shared infrastructure for data source access:
- Connection setup to hermes-substream
- Cursor/checkpoint persistence
- Typed event stream for transformers to consume

**`hermes-schema`** provides Kafka output message definitions:
- Protobuf definitions for all Kafka output messages
- Shared by all transformers

This separation keeps transformers independent while sharing common logic. If the underlying data source changes (e.g., swap substreams for something else), only relay changes.

## Ordering Guarantees

| Event Type | Ordering Requirement |
|------------|---------------------|
| Edits | Globally ordered within edits (diffs depend on prior state) |
| Space/Governance/Curation | Globally ordered (data source order) |
| Canonical Graph (topology) | Ordered per emission (independent stream) |

Edit events must maintain global ordering because edits are diffs - each edit depends on the state established by prior edits.

## Components

### hermes-substream

Substream that filters and emits events from the Space Registry contract.

**Location:** `hermes-substream/` (excluded from workspace)

**Provides:**
- Raw `Action` events via `map_actions` module
- Pre-filtered typed events via dedicated modules (e.g., `map_edits_published`, `map_spaces_registered`)

**Action Event Structure:**
```
Action {
    from_id: bytes16,    // Source space ID
    to_id: bytes16,      // Target space ID  
    action: bytes32,     // Action type (keccak256 hash)
    topic: bytes32,      // Context-dependent field
    data: bytes,         // Action-specific payload
}
```

**Available Modules:**

| Module | Description |
|--------|-------------|
| `map_actions` | All raw Action events |
| `map_spaces_registered` | Space registrations |
| `map_spaces_migrated` | Space migrations |
| `map_proposals_created` | Governance proposals |
| `map_proposals_voted` | Proposal votes |
| `map_proposals_executed` | Executed proposals |
| `map_editors_added` | Editor additions |
| `map_editors_removed` | Editor removals |
| `map_members_added` | Member additions |
| `map_members_removed` | Member removals |
| `map_edits_published` | Published edits |
| `map_subspaces_added` | Subspace additions |
| `map_subspaces_removed` | Subspace removals |
| `map_objects_upvoted` | Object upvotes |
| `map_objects_downvoted` | Object downvotes |
| `map_objects_unvoted` | Vote removals |

Consumers subscribe to specific modules to receive only the events they need.

### hermes-relay (Library)

Shared infrastructure for connecting to hermes-substream.

**Location:** `hermes-relay/`

**Provides:**
- Connection setup to hermes-substream
- Cursor/checkpoint management
- Typed event stream for transformers to consume

### hermes-schema (Library)

Protobuf definitions for Kafka output messages.

**Location:** `hermes-schema/`

**Protos:**
- `knowledge.proto` - HermesEdit message
- `space.proto` - HermesCreateSpace, HermesSpaceTrustExtension
- `topology.proto` - CanonicalGraphUpdated, CanonicalTreeNode
- `blockchain_metadata.proto` - Common metadata fields

### spaces (Binary)

Transforms space events and emits to Kafka.

**Uses:** `hermes-relay`  
**Subscribes to:** `map_spaces_registered`, `map_subspaces_added`, `map_subspaces_removed`  
**Output:** `spaces` Kafka topic

**Handles:**
- Space registration events
- Subspace relationship changes

### edits (Binary)

Transforms edit events, resolving IPFS content via cache, and emits to Kafka.

**Uses:** `hermes-relay`, IPFS cache  
**Subscribes to:** `map_edits_published`  
**Output:** `edits` Kafka topic

**How it works:**
1. Receives `EditsPublished` event (contains IPFS CID in data field)
2. Reads resolved content from IPFS cache
3. Decodes into `Edit` protobuf (ops, authors, etc.)
4. Enriches with blockchain metadata
5. Emits `HermesEdit` to Kafka

### topology (Binary) [Atlas]

Computes the canonical graph from space topology events.

**Uses:** `hermes-relay`  
**Subscribes to:** `map_spaces_registered`, `map_subspaces_added`, `map_subspaces_removed`  
**Output:** `topology` Kafka topic

**Key modules:**
- `GraphState` - Stores all spaces, edges, and topic memberships
- `TransitiveProcessor` - Computes reachable spaces from any root
- `CanonicalProcessor` - Filters to only canonical (trusted) spaces

### IPFS Cache Service

Pre-populates resolved IPFS contents ahead of time so the edits transformer doesn't block on network I/O.

**Location:** `cache/`

**How it works:**
1. Connects to hermes-substream `map_edits_published` (parallelized, runs ahead)
2. For each edit event, fetches the IPFS content by CID
3. Stores resolved content in the cache

**Cache miss behavior:** If edits encounters a cache miss, it waits and retries until the content appears. The cache should always be ahead, so misses indicate the cache is catching up.

### mock-substream (Library)

Generates deterministic blockchain events for testing.

**Location:** `mock-substream/`

**Exports:**
- `MockEvent` - Enum of event types (SpaceRegistered, EditsPublished, etc.)
- `test_topology::generate()` - Generates deterministic test topology
- Well-known IDs for spaces, topics, entities, etc.

## Event Types

### Space Registered

A new space is registered on-chain.

```
SpaceRegistered {
    space_id: bytes16,       // Unique space identifier
    space_address: bytes20,  // Contract address
    data: bytes,             // Additional data
}
```

### Edits Published

An edit (set of GRC-20 operations) is published to a space.

```
EditsPublished {
    space_id: bytes16,  // Space the edit belongs to
    data: bytes,        // Contains IPFS CID
}
```

### Subspace Added/Removed

A space adds or removes a subspace relationship.

```
SubspaceAdded {
    parent_space_id: bytes16,  // Parent space
    subspace_id: bytes16,      // Child space
    data: bytes,
}
```

## Canonical Graph

The canonical graph represents the "trusted" portion of the knowledge graph.

### Rules

1. The **root space** is always canonical
2. A space is canonical if reachable from root via **explicit edges only** (subspace relationships)
3. **Topic edges** can attach subtrees, but only canonical members are included

### Example Topology

```
CANONICAL (reachable from Root):

  Root
   ├─subspace─▶ A ─subspace─▶ C ─subspace─▶ F
   │             │              └─subspace─▶ G
   │             └─subspace─▶ D
   ├─subspace─▶ B ─subspace─▶ E
   └─subspace─▶ H ─subspace─▶ I
                └─subspace─▶ J

NON-CANONICAL (isolated islands):

  Island 1: X ─▶ Y ─▶ Z
             └─▶ W

  Island 2: P ─▶ Q

  Island 3: S (isolated)
```

## Kafka Topics

| Topic | Producer | Message Type | Description |
|-------|----------|--------------|-------------|
| `spaces` | spaces | HermesCreateSpace | Space creation and relationship changes |
| `edits` | edits | HermesEdit | Resolved knowledge graph edits |
| `topology` | topology | CanonicalGraphUpdated | Canonical graph updates |
| `governance` | (future) | (TBD) | Proposals, voting, membership |
| `curation` | (future) | (TBD) | Ranking, voting on entities |

## Crate Structure

```
gaia/
├── hermes-substream/    # Substream: decodes Space Registry events (excluded from workspace)
├── hermes-relay/        # Library: connects to substream, provides typed event stream
├── hermes-schema/       # Library: Kafka output protobuf definitions
├── hermes-processor/    # Binary: main processor (TBD)
├── cache/               # Service: IPFS content pre-fetcher
├── mock-substream/      # Library: test event generation
└── atlas/               # Library: canonical graph computation
```

Each binary depends on `hermes-relay` for data source access and `hermes-schema` for output types.

## Data Flow Example

### Space and Topology Events

1. **Space Registry contract** emits: `Action(GOVERNANCE.SPACE_ID_REGISTERED, ...)`

2. **hermes-substream** `map_spaces_registered`:
   - Filters for space registration actions
   - Emits `SpaceRegistered` protobuf

3. **spaces transformer** (via hermes-relay):
   - Subscribes to `map_spaces_registered`
   - Converts to `HermesCreateSpace` proto
   - Publishes to `spaces` Kafka topic
   - Updates cursor

4. **topology transformer** (via hermes-relay, independent):
   - Subscribes to `map_spaces_registered`, `map_subspaces_added`
   - Updates `GraphState` with new space
   - Recomputes canonical graph
   - Publishes updated graph to `topology` topic

### Edit Events

1. **Space Registry contract** emits: `Action(GOVERNANCE.EDITS_PUBLISHED, ..., ipfs_cid)`

2. **IPFS Cache Service** (running ahead):
   - Subscribes to `map_edits_published`
   - Fetches content for `ipfs_cid`
   - Stores in cache

3. **hermes-substream** `map_edits_published`:
   - Filters for edit actions
   - Emits `EditsPublished` protobuf

4. **edits transformer** (via hermes-relay):
   - Subscribes to `map_edits_published`
   - Reads resolved content from IPFS cache (waits if miss)
   - Decodes into `Edit` protobuf
   - Enriches with blockchain metadata
   - Publishes `HermesEdit` to `edits` topic

5. **Downstream consumers** read from Kafka topics to:
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
