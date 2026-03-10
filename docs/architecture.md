# Hermes Architecture

This document describes the architecture of Hermes, the system that ingests blockchain data, transforms it, and emits normalized events to Kafka.

## Overview

Hermes is an umbrella system composed of independent transformers that process blockchain events. Each transformer:

- Connects to the blockchain data source via a shared library (relay)
- Filters for events it cares about
- Applies its transformation logic
- Emits to its Kafka topic(s)
- Maintains its own cursor for independent restart/replay

The system receives blockchain events which fall into several categories:

1. **Space changes** - registrations, migrations, subspace relationships
2. **Edit publishing** - content modifications (contain IPFS hashes pointing to actual content)
3. **Membership** - editor/member additions, removals, departures
4. **Moderation** - editor and content flagging/unflagging
5. **Topics** - topic declarations on spaces
6. **Governance** - proposals (create, update, vote, execute, settings)
7. **Curation** - upvotes, downvotes, unvotes on objects

```
                              ┌───────────────────────────────────────────────────────────────┐
                              │  Hermes                                                       │
                              │                                                               │
┌──────────────┐              │  ┌────────────────┐    ┌─────────────────────────────┐        │
│  Blockchain  │              │  │ hermes-        │    │  hermes-relay (lib)         │        │
│    (Geo)     │─────────────▶│  │ substream      │───▶│  - Connect to substream     │        │
└──────────────┘              │  │ (lib)          │    │  - Cursor/checkpoint mgmt   │        │
                              │  └────────────────┘    │  - Typed event stream       │        │
                              │                        └──────────────┬──────────────┘        │
                              │                                       │                       │
                              │                  ┌────────────────────┤                       │
                              │                  │                    │                       │
                              │                  ▼                    ▼                       │
                              │  ┌───────────────────────────┐  ┌──────────┐                  │
                              │  │  hermes-pipeline (bin)    │  │  atlas   │                  │
                              │  │  - All event pipelines    │  │  (bin)   │                  │
                              │  │  - IPFS cache integration │  └────┬─────┘                  │
                              │  └────────────┬──────────────┘       │                       │
                              │               │                      │                       │
                              └───────────────┼──────────────────────┼───────────────────────┘
                                              │                      │
                                              ▼                      ▼
                                           Kafka:                 Kafka:
                                           space.creations        topology.canonical
                                           space.trust.extensions
                                           space.membership
                                           space.moderation
                                           space.topics
                                           space.governance
                                           curation.votes
                                           knowledge.edits
                                           hermes.blocks

                              ┌────────────────────────────────────────┐
Blockchain ──────────────────▶│  IPFS Cache (parallel, ahead-of-time) │
Data Source                   │  (hermes-pipeline uses mock cache for  │
                              │   dev, live cache for production)      │
                              └────────────────────────────────────────┘
```

## Design Principles

### Consolidated Transformers

`hermes-pipeline` is the primary transformer that handles all blockchain events:

- Space registrations
- Trust relationships (subspaces)
- Membership (editor/member grants, revocations, departures)
- Moderation (editor and content flagging/unflagging)
- Topic declarations
- Governance (proposals: create, update, vote, execute, settings)
- Curation voting (upvote, downvote, unvote)
- Edit publishing (with IPFS cache integration)

This consolidation provides:

- **Simpler deployment** - One binary for all space-related events
- **Shared infrastructure** - Single Kafka producer, cursor management
- **Consistent patterns** - Same conversion/emit patterns across event types

`atlas` remains a separate binary for canonical graph computation, as it maintains complex in-memory state (with persistence/checkpointing) and has different scaling characteristics.

### Shared Libraries

**`hermes-substream`** decodes raw Ethereum logs from the Space Registry contract:

- Filters for Action events from the Space Registry
- Provides raw actions and pre-filtered typed event modules
- Runs on Substreams infrastructure (included in workspace as an `rlib`/`cdylib`)

**`hermes-relay`** provides shared infrastructure for data source access:

- Connection setup to hermes-substream
- Cursor/checkpoint persistence
- Typed event stream for transformers to consume

**`hermes-schema`** provides Kafka output message definitions:

- Protobuf definitions for all Kafka output messages
- Shared by all transformers

**`hermes-kafka`** provides shared Kafka producer infrastructure:

- Producer creation and configuration (SASL/SSL, compression, idempotency)
- Environment-based topic prefixing (`staging.` prefix for staging isolation)
- Shared by `hermes-pipeline` and `atlas`

**`hermes-instrumentation`** provides unified telemetry:

- Tracing with OpenTelemetry and Sentry integration
- Configurable backends (Console, Sentry with Axiom forwarding)
- Shared by all Hermes binaries

This separation keeps transformers independent while sharing common logic. If the underlying data source changes (e.g., swap substreams for something else), only relay changes.

## Ordering Guarantees

| Event Type                 | Ordering Requirement                                        |
| -------------------------- | ----------------------------------------------------------- |
| Edits                      | Globally ordered within edits (diffs depend on prior state) |
| Space/Governance/Curation  | Globally ordered (data source order)                        |
| Canonical Graph (topology) | Ordered per emission (independent stream)                   |

Edit events must maintain global ordering because edits are diffs - each edit depends on the state established by prior edits.

Within a block, `hermes-pipeline` emits events in a fixed order: spaces → membership → trust → moderation → topics → governance → voting → edits. Each event carries a `sequence` number (action array index) and the last event in the block is marked with `is_last = true` so consumers know when a block is complete.

## Components

### hermes-substream

Substream that filters and emits events from the Space Registry contract.

**Location:** `hermes-substream/`

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

| Module                   | Description           |
| ------------------------ | --------------------- |
| `map_actions`            | All raw Action events |
| `map_spaces_registered`  | Space registrations   |
| `map_spaces_migrated`    | Space migrations      |
| `map_proposals_created`  | Governance proposals  |
| `map_proposals_voted`    | Proposal votes        |
| `map_proposals_executed` | Executed proposals    |
| `map_editors_added`      | Editor additions      |
| `map_editors_removed`    | Editor removals       |
| `map_members_added`      | Member additions      |
| `map_members_removed`    | Member removals       |
| `map_edits_published`    | Published edits       |
| `map_subspaces_added`    | Subspace additions    |
| `map_subspaces_removed`  | Subspace removals     |
| `map_objects_upvoted`    | Object upvotes        |
| `map_objects_downvoted`  | Object downvotes      |
| `map_objects_unvoted`    | Vote removals         |

Consumers subscribe to specific modules to receive only the events they need.

### hermes-relay (Library)

Shared infrastructure for connecting to hermes-substream.

**Location:** `hermes-relay/`

**Provides:**

- `Sink` and `PreprocessedSink` traits for consuming events
- `StreamSource` config for explicitly choosing mock or live data sources
- `MockSource` and `mock_events` for custom test data
- Connection setup to hermes-substream
- Cursor/checkpoint management
- Typed event stream for transformers to consume
- Action type constants for client-side filtering

### hermes-schema (Library)

Protobuf definitions for Kafka output messages.

**Location:** `hermes-schema/`

**Protos:**

- `knowledge.proto` - HermesEdit message
- `space.proto` - HermesCreateSpace, HermesSpaceTrustExtension
- `topology.proto` - CanonicalGraphUpdated, CanonicalTreeNode
- `blockchain_metadata.proto` - Common metadata fields (block number, cursor, sequence, is_last)
- `membership.proto` - HermesRoleGranted, HermesRoleRevoked, HermesSpaceLeft
- `moderation.proto` - HermesEditorFlagged, HermesEditorUnflagged, HermesContentFlagged, HermesContentUnflagged
- `topics.proto` - HermesTopicDeclared
- `governance.proto` - HermesProposalCreated, HermesProposalUpdated, HermesProposalVoted, HermesProposalExecuted, HermesProposalSettingsUpdated
- `voting.proto` - HermesVoteCast
- `scoring.proto` - HermesScoresBatch
- `block_summary.proto` - HermesBlockSummary

### hermes-kafka (Library)

Shared Kafka producer infrastructure.

**Location:** `hermes-kafka/`

**Provides:**

- `create_producer` / `create_producer_with_config` - Producer creation with SASL/SSL, zstd compression, idempotent delivery
- `get_topic_prefix` / `prefixed_topic` - Environment-based topic prefixing (staging vs production)
- `strip_topic_prefix` - Canonical topic name extraction

### hermes-instrumentation (Library)

Unified telemetry for the Hermes ecosystem.

**Location:** `hermes-instrumentation/`

**Provides:**

- Tracing with `tracing` + `tracing-subscriber`
- OpenTelemetry integration via `tracing-opentelemetry`
- Sentry error tracking and performance monitoring
- Axiom log forwarding (optional, via Sentry backend)
- Configurable backends: `Console` (development) or `Sentry` (production)

### hermes-pipeline (Binary)

Primary transformer that handles all space-related events.

**Location:** `hermes-pipeline/`

**Uses:** `hermes-relay`, `hermes-kafka`, `hermes-schema`, `hermes-instrumentation`, IPFS cache (mock or live)
**Subscribes to:** `map_actions` (filters client-side for relevant action types)
**Output:** Multiple Kafka topics (see table below)

**Handles:**

- **Space registration** (`SPACE_REGISTERED`) → `space.creations` topic
- **Trust relationships** (`SUBSPACE_VERIFIED`, `SUBSPACE_RELATED`, `SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, `SUBSPACE_TOPIC_DECLARED`, `SUBSPACE_TOPIC_REMOVED`) → `space.trust.extensions` topic
- **Membership** (`EDITOR_ADDED`, `MEMBER_ADDED`, `EDITOR_REMOVED`, `MEMBER_REMOVED`, `SPACE_LEFT`) → `space.membership` topic
- **Moderation** (`EDITOR_FLAGGED`, `EDITOR_UNFLAGGED`, `FLAGGED`, `UNFLAGGED`) → `space.moderation` topic
- **Topics** (`TOPIC_DECLARED`) → `space.topics` topic
- **Governance** (`PROPOSAL_CREATED`, `PROPOSAL_UPDATED`, `PROPOSAL_VOTED`, `PROPOSAL_EXECUTED`, `PROPOSAL_SETTINGS_UPDATED`) → `space.governance` topic
- **Voting** (`UPVOTED`, `DOWNVOTED`, `UNVOTED`) → `curation.votes` topic
- **Edit publishing** (`EDITS_PUBLISHED`) → `knowledge.edits` topic
- **Block summary** (emitted after every block) → `hermes.blocks` topic

**Processing architecture:**

1. **Phase 0 (Prefetch)**: Batch all IPFS URI lookups for the block (edits and proposal content) so transform functions can be synchronous
2. **Phase 1 (Transform)**: All pipelines run synchronously using prefetched data
3. **Phase 1.5 (Mark last)**: Find the maximum sequence number across all events and mark it `is_last = true`
4. **Phase 2 (Emit)**: Send events to Kafka in order (spaces → membership → trust → moderation → topics → governance → voting → edits → block summary)

**IPFS Cache modes:**

- **Mock mode** (development): In-memory cache with pre-populated test edits
- **Live mode** (production): Reads from `hermes-ipfs-cache` PostgreSQL store

### atlas (Binary)

Computes the canonical graph from space topology events.

**Location:** `atlas/`

**Uses:** `hermes-relay`, `hermes-kafka`, `hermes-schema`, `hermes-instrumentation`
**Subscribes to:** `map_actions` (filters client-side for topology-relevant action types)
**Output:** `topology.canonical` Kafka topic

**Key modules:**

- `GraphState` - Stores all spaces, edges (explicit + topic), and topic memberships
- `TransitiveProcessor` - Computes reachable spaces from any root (with cache invalidation)
- `CanonicalProcessor` - Filters to only canonical (trusted) spaces
- `DiffTracker` - Computes added/removed/moved changes between canonical graph snapshots
- `CheckpointManager` - Persists graph state to PostgreSQL for fast restart
- `PersistedGraphState` - Serializable snapshot of `GraphState` for checkpoint/restore

**Processing model:** Per-block batching — all events in a block are applied to graph state, then canonical graph is recomputed once, diffed once, and emitted once. This avoids per-event intermediate diffs and ensures consumers see atomic block-level updates.

**Topology events handled:**

- `SpaceCreated` - New space registrations
- `TrustExtended` - Verified, related, subtopic, editor/member added/removed, and their removals

### IPFS Cache Service

Pre-populates resolved IPFS contents ahead of time so `hermes-pipeline` doesn't block on network I/O when processing edits.

**Location:** `hermes-ipfs-cache/`

**How it works:**

1. Connects to hermes-substream `map_edits_published` (parallelized, runs ahead)
2. For each edit event, fetches the IPFS content by CID
3. Stores resolved content in PostgreSQL cache

**Cache miss behavior:** If `hermes-pipeline` encounters a cache miss, it retries with exponential backoff (configurable via `RetryConfig`). The cache should always be ahead, so misses indicate the cache is catching up.

**Development mode:** `hermes-pipeline` includes a mock IPFS cache with pre-populated test edits, allowing development without running the full cache service.

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

| Topic                    | Producer        | Message Type(s)                                                                                                          | Description                                             |
| ------------------------ | --------------- | ------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| `space.creations`        | hermes-pipeline | HermesCreateSpace                                                                                                        | Space creation events                                   |
| `space.trust.extensions` | hermes-pipeline | HermesSpaceTrustExtension                                                                                                | Trust extension events (verified, related, subtopic)    |
| `space.membership`       | hermes-pipeline | HermesRoleGranted, HermesRoleRevoked, HermesSpaceLeft                                                                    | Membership changes (editor/member grant, revoke, leave) |
| `space.moderation`       | hermes-pipeline | HermesEditorFlagged, HermesEditorUnflagged, HermesContentFlagged, HermesContentUnflagged                                 | Editor and content flagging events                      |
| `space.topics`           | hermes-pipeline | HermesTopicDeclared                                                                                                      | Topic declarations on spaces                            |
| `space.governance`       | hermes-pipeline | HermesProposalCreated, HermesProposalUpdated, HermesProposalVoted, HermesProposalExecuted, HermesProposalSettingsUpdated | Governance proposal lifecycle events                    |
| `curation.votes`         | hermes-pipeline | HermesVoteCast                                                                                                           | Upvote, downvote, and unvote events                     |
| `knowledge.edits`        | hermes-pipeline | HermesEdit                                                                                                               | Resolved knowledge graph edits                          |
| `hermes.blocks`          | hermes-pipeline | HermesBlockSummary                                                                                                       | Per-block summary with event counts by topic/type       |
| `topology.canonical`     | atlas           | CanonicalGraphUpdated                                                                                                    | Canonical graph diff updates                            |

All topics support environment prefixing: in staging, topics are prefixed with `staging.` (e.g., `staging.knowledge.edits`).

## Crate Structure

```
gaia/
├── hermes-substream/      # Library: decodes Space Registry events (Substreams WASM + rlib)
├── hermes-relay/          # Library: connects to substream, provides typed event stream
├── hermes-schema/         # Library: Kafka output protobuf definitions
├── hermes-kafka/          # Library: shared Kafka producer config, topic prefixing
├── hermes-instrumentation/# Library: unified telemetry (tracing, OpenTelemetry, Sentry)
├── hermes-pipeline/       # Binary: primary transformer (all event types)
├── hermes-ipfs-cache/     # Service: IPFS content pre-fetcher (production)
└── atlas/                 # Binary: canonical graph computation, publishes diffs to Kafka
```

`hermes-pipeline` is the primary transformer, handling all event types (spaces, membership, trust, moderation, topics, governance, voting, edits). It depends on `hermes-relay` for data source access, `hermes-schema` for output types, `hermes-kafka` for Kafka production, `hermes-instrumentation` for telemetry, and uses either a mock or live IPFS cache for edit content resolution.

## Data Flow Example

### Space and Topology Events

1. **Space Registry contract** emits: `Action(GOVERNANCE.SPACE_ID_REGISTERED, ...)`

2. **hermes-substream** `map_actions`:
   - Passes through all Action events

3. **hermes-pipeline** (via hermes-relay):
   - Subscribes to `map_actions`, filters for `SPACE_REGISTERED`
   - Converts to `HermesCreateSpace` proto
   - Publishes to `space.creations` Kafka topic
   - Updates cursor

4. **atlas** (via hermes-relay, independent):
   - Subscribes to `map_actions`, filters for topology-relevant actions
   - Updates `GraphState` with new space
   - Recomputes canonical graph
   - Diffs against previous canonical state
   - Publishes diff to `topology.canonical` topic
   - Persists checkpoint to PostgreSQL

### Edit Events

1. **Space Registry contract** emits: `Action(GOVERNANCE.EDITS_PUBLISHED, ..., ipfs_cid)`

2. **IPFS Cache Service** (running ahead, production only):
   - Subscribes to `map_edits_published`
   - Fetches content for `ipfs_cid`
   - Stores in PostgreSQL cache

3. **hermes-substream** `map_actions`:
   - Passes through all Action events

4. **hermes-pipeline** (via hermes-relay):
   - Subscribes to `map_actions`, filters for `EDITS_PUBLISHED`
   - Phase 0: Prefetches IPFS content from cache (with retry/backoff)
   - Phase 1: Decodes into `Edit` protobuf, enriches with blockchain metadata
   - Phase 2: Publishes `HermesEdit` to `knowledge.edits` topic

5. **Downstream consumers** read from Kafka topics to:
   - Update search indices
   - Trigger notifications
   - Sync to other databases

## Deployment

Each transformer runs as an independent service:

```bash
# Local development (from hermes/ directory)
docker-compose -f hermes/docker-compose.yaml up

# Starts:
# - Kafka broker (localhost:9092)
# - Kafka UI (http://localhost:8080)
# - hermes-pipeline transformer
# - atlas (topology transformer)
# - hermes-ipfs-cache service
# - PostgreSQL (for IPFS cache)
```

All services run with mock data by default, processing a deterministic test topology and publishing to Kafka topics.

### Independent Operations

```bash
# Restart hermes-pipeline (handles all event types)
kubectl rollout restart deployment/hermes-pipeline

# Replay from specific block (requires USE_MOCK=false for live data)
kubectl set env deployment/hermes-pipeline USE_MOCK=false SUBSTREAMS_START_BLOCK=1000000
kubectl rollout restart deployment/hermes-pipeline

# atlas (topology) continues unaffected
kubectl rollout restart deployment/atlas
```

## Related Documents

- [Atlas Documentation](../atlas/docs/) - Canonical graph computation (topology consumer)
- [Hermes Infrastructure](../hermes/README.md) - Local development and deployment
- [Hermes Substream](../hermes-substream/README.md) - Event filtering from blockchain
