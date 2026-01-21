# 0006: Live Substream, Membership, and Graph Diffing Implementation Plan

Update Atlas to use live substreams, ingest membership changes, and emit graph diffs.

## Overview

This plan covers three related changes:
1. **Live Substream** - Replace mock data source with live blockchain data
2. **Membership Changes** - Treat EDITOR/MEMBER changes as topology edges
3. **Graph Diffing** - Emit incremental diffs instead of full tree snapshots

## Phase 1: Live Substream

**Goal:** Replace `StreamSource::mock()` with configurable live substream.

**File:** `atlas/src/main.rs`

**Changes:**
1. Add env var parsing following `hermes-pipeline` pattern:
   - `USE_MOCK` - boolean to use mock (default: false)
   - `SUBSTREAMS_ENDPOINT` - endpoint URL (default: geotest.substreams.pinax.network:443)
   - `SUBSTREAMS_START_BLOCK` - i64 (default: 82655)
   - `SUBSTREAMS_END_BLOCK` - u64 (default: u64::MAX)

2. Add import: `use hermes_relay::HermesModule;`

3. Replace line 265:
   ```rust
   let source = if use_mock {
       StreamSource::mock()
   } else {
       StreamSource::live(endpoint, HermesModule::Actions, start_block, end_block)
   };
   ```

4. Add configuration logging before `sink.run(source)`

---

## Phase 2: Membership Changes as Topology

**Goal:** Treat EDITOR_ADDED/REMOVED and MEMBER_ADDED/REMOVED as topology changes with new edge types.

### 2.1 Add new EdgeType variants

**File:** `atlas/src/graph/tree.rs`

Add to `EdgeType` enum:
```rust
pub enum EdgeType {
    Root,
    Verified,
    Related,
    Topic,
    Editor,  // NEW
    Member,  // NEW
}
```

### 2.2 Add new TrustExtension variants

**File:** `atlas/src/events.rs`

Add to `TrustExtension` enum:
```rust
pub enum TrustExtension {
    Verified { target_space_id: SpaceId },
    Related { target_space_id: SpaceId },
    Subtopic { target_topic_id: TopicId },
    EditorAdded { member_space_id: SpaceId },    // NEW
    EditorRemoved { member_space_id: SpaceId },  // NEW
    MemberAdded { member_space_id: SpaceId },    // NEW
    MemberRemoved { member_space_id: SpaceId },  // NEW
}
```

### 2.3 Add action conversion

**File:** `atlas/src/convert.rs`

Add handlers for membership actions:
- `EDITOR_ADDED` -> `TrustExtension::EditorAdded`
- `EDITOR_REMOVED` -> `TrustExtension::EditorRemoved`
- `MEMBER_ADDED` -> `TrustExtension::MemberAdded`
- `MEMBER_REMOVED` -> `TrustExtension::MemberRemoved`

Data extraction:
- `source_space_id` = `action.from_id[0..16]`
- `member_space_id` = `action.topic[0..16]`

### 2.4 Add membership edge storage

**File:** `atlas/src/graph/state.rs`

Add to `GraphState`:
```rust
pub membership_edges: HashMap<SpaceId, Vec<(SpaceId, MembershipType)>>,
```

Add `MembershipType` enum with `Editor` and `Member` variants.

Update `apply_trust_extended()` to handle add/remove operations.

### 2.5 Update transitive BFS

**File:** `atlas/src/graph/transitive.rs`

Include membership edges in BFS traversal alongside explicit edges.

### 2.6 Update protobuf schema

**File:** `hermes-schema/proto/topology.proto`

Add to `CanonicalTreeNode.edge` oneof:
```protobuf
EditorEdge editor = 7;
MemberEdge member = 8;
```

Add message definitions:
```protobuf
message EditorEdge {}
message MemberEdge {}
```

### 2.7 Update emitter

**File:** `atlas/src/kafka/emitter.rs`

Update `tree_node_to_proto()` to handle `EdgeType::Editor` and `EdgeType::Member`.

---

## Phase 3: Graph Diffs

**Goal:** Emit diffs (added/removed nodes) instead of full tree for both canonical and transitive states.

**Documentation:** See [0001-graph-diff-emission-adr.md](../../../../docs/rfcs/0001-graph-diff-emission-adr.md) for full design details and examples.

### Design Decisions

1. **Single canonical edge per node** - Each node has exactly one parent edge (shortest path via BFS). Alternative edges are not emitted. This is **lossy** - consumers cannot reconstruct edge-type-specific traversals.

2. **Moves are REMOVED + ADDED** - When a node's parent or distance changes, it appears as REMOVED then ADDED. No explicit MOVED operation.

3. **Distance cascades** - When a node moves closer to root, all descendants also have distance changes and appear in the diff.

### 3.1 Add diff protobuf messages

**File:** `hermes-schema/proto/topology.proto`

```protobuf
message CanonicalGraphDiff {
  bytes root_id = 1;
  repeated NodeChange changes = 2;
  blockchain_metadata.BlockchainMetadata meta = 3;
}

message TransitiveGraphDiff {
  bytes root_id = 1;
  repeated NodeChange changes = 2;
  blockchain_metadata.BlockchainMetadata meta = 3;
}

message NodeChange {
  bytes space_id = 1;
  ChangeType type = 2;              // ADDED, REMOVED
  optional uint32 distance = 3;     // for ADDED: minimum hops from root
  optional EdgeInfo parent_edge = 4; // for ADDED: how this node was reached
}

message EdgeInfo {
  bytes source = 1;                 // parent node
  oneof edge_type {
    VerifiedEdge verified = 2;
    RelatedEdge related = 3;
    TopicEdge topic = 4;            // includes topic_id
    EditorEdge editor = 5;
    MemberEdge member = 6;
  }
}

enum ChangeType {
  ADDED = 0;
  REMOVED = 1;
}
```

### 3.2 Create diff computation module

**New file:** `atlas/src/graph/diff.rs`

```rust
pub struct NodeChange {
    pub space_id: SpaceId,
    pub change_type: ChangeType,
    pub distance: Option<u32>,
    pub parent_edge: Option<ParentEdge>,
}

pub struct ParentEdge {
    pub source: SpaceId,
    pub edge_type: EdgeType,
    pub topic_id: Option<TopicId>,
}

pub enum ChangeType {
    Added,
    Removed,
}

pub struct GraphDiff {
    pub changes: Vec<NodeChange>,
}

impl GraphDiff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Compare old and new trees, return changes
pub fn compute_diff(old: Option<&CanonicalGraph>, new: &CanonicalGraph) -> GraphDiff
```

Export from `atlas/src/graph/mod.rs`.

### 3.3 Update CanonicalProcessor

**File:** `atlas/src/graph/canonical.rs`

- Store `last_graph: Option<CanonicalGraph>` alongside `last_hash`
- Add `compute_with_diff()` method returning `Option<(CanonicalGraph, GraphDiff)>`

### 3.4 Create diff emitter

**New file:** `atlas/src/kafka/diff_emitter.rs`

- Emit `CanonicalGraphDiff` to canonical topic
- Emit `TransitiveGraphDiff` to transitive topic
- Skip emission if diff is empty

Export from `atlas/src/kafka/mod.rs`.

### 3.5 Wire up in main

**File:** `atlas/src/main.rs`

- Create separate Kafka topics for canonical and transitive diffs
- Replace `CanonicalGraphEmitter` with `DiffEmitter`
- Update `process_event()` to use `compute_with_diff()` and emit both diffs

---

## Implementation Order

1. **Phase 1** - Live substream (single file change, testable independently)
2. **Phase 2** - Membership changes (incremental, can verify with mock data)
3. **Phase 3** - Graph diffs (requires phase 2 complete for full testing)

## Key Files

| File | Phase | Changes |
|------|-------|---------|
| `atlas/src/main.rs` | 1, 2, 3 | Env vars, logging, diff emitter |
| `atlas/src/graph/tree.rs` | 2 | EdgeType::Editor/Member |
| `atlas/src/events.rs` | 2 | TrustExtension variants |
| `atlas/src/convert.rs` | 2 | Membership action handlers |
| `atlas/src/graph/state.rs` | 2 | Membership edge storage |
| `atlas/src/graph/transitive.rs` | 2 | BFS membership traversal |
| `atlas/src/graph/canonical.rs` | 3 | Diff tracking |
| `atlas/src/graph/diff.rs` | 3 | NEW - diff computation |
| `atlas/src/kafka/emitter.rs` | 2 | Editor/Member edge handling |
| `atlas/src/kafka/diff_emitter.rs` | 3 | NEW - diff emission |
| `hermes-schema/proto/topology.proto` | 2, 3 | Edge types + diff messages |

## Considerations

- **Cache invalidation:** Membership changes need to invalidate transitive cache entries
- **Topic edges in diffs:** Must preserve `topic_id` in EdgeInfo for topic edges
- **Backwards compatibility:** May want separate Kafka topics for diffs vs full updates during transition
- **Lossiness:** Only canonical path edges are emitted; alternative edges (longer paths, different edge types to same node) are not included. Document this limitation for consumers.
- **Move verbosity:** Moves emit REMOVED + ADDED for the node and all descendants whose distance changes. This can be verbose for deep subtree moves.
- **Diff algorithm choice:** Use **sorted vector merge** (Approach #2) for diff computation. Benchmarking on mocked 100k-node topologies (1% add/remove/move) shows ~0.5–0.7ms diff-only and ~70–90ms including data construction; performance scales with change rate and stays <~1.3ms diff-only at 50% churn. Roaring/BTree are competitive but add operational complexity (UUID→integer mapping) without clear wins. Keep this as the default; revisit only if real-world profiling shows a bottleneck.
- **Diff batching:** Emit all changes for a given graph update in a single diff event, ordered deterministically (sorted by `space_id`). This enables atomic application and avoids transient inconsistencies.
- **Move encoding:** Use explicit `MOVED` changes for position updates (same payload as `ADDED`).

## Related Documentation

- [Graph Diff Emission](../../../../docs/rfcs/0001-graph-diff-emission-adr.md) - Full diff design with examples
- [Algorithm Overview](../../algorithm-overview.md) - How transitive and canonical algorithms work
- [Graph Concepts](../../graph-concepts.md) - Core graph concepts and edge types
