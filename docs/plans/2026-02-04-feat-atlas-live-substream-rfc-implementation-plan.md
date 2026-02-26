---
title: "feat: Atlas Live Substream + RFC Implementation"
type: feat
date: 2026-02-04
---

# Atlas Live Substream + RFC Implementation

## Overview

Update Atlas to use live blockchain data and implement the canonical graph RFCs. This involves three coordinated changes:

1. **Live Substream Migration** - Switch from `StreamSource::Mock` to `StreamSource::Live`
2. **RFC 0001: Canonical Graph Inputs** - Add Editor/Member edges as canonical-granting, handle edge removals
3. **RFC 0002: Graph Diff Emission** - Emit incremental diffs with explicit MOVED semantics (replaces full snapshots)

## Problem Statement / Motivation

Atlas currently runs against mock data and emits full graph snapshots on every change. To be production-ready:

- Must consume real blockchain events via live substream
- Must handle all topology-affecting events (including Editor/Member edges per RFC 0001)
- Must emit efficient incremental diffs instead of full snapshots (per RFC 0002)

The RFCs were audited for edge cases in a [brainstorm session](../brainstorms/2026-02-04-atlas-update-brainstorm.md). Key decisions are incorporated into this plan.

## Proposed Solution

Implement in four phases, each independently testable:

1. **Phase 1**: Add missing actions to hermes-substream (edge removals)
2. **Phase 2**: Live substream configuration in Atlas
3. **Phase 3**: Membership + edge removal handling (RFC 0001)
4. **Phase 4**: Graph diff emission (RFC 0002) - replaces full snapshot emission

## Technical Approach

### Phase 1: Add Missing Actions to Substream

**Goal:** Add `SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, and `SUBSPACE_TOPIC_REMOVED` actions to the pipeline so Atlas can handle edge removals.

#### 1.1 Add action constants to hermes-substream

**File:** `hermes-substream/src/lib.rs`

Add constants (following existing pattern):

```rust
pub const ACTION_SUBSPACE_UNVERIFIED: [u8; 32] = /* keccak256('GOVERNANCE.SUBSPACE_UNVERIFIED') */;
pub const ACTION_SUBSPACE_UNRELATED: [u8; 32] = /* keccak256('GOVERNANCE.SUBSPACE_UNRELATED') */;
pub const ACTION_SUBSPACE_TOPIC_REMOVED: [u8; 32] = /* keccak256('GOVERNANCE.SUBSPACE_TOPIC_REMOVED') */;
```

#### 1.2 Re-export from hermes-relay

**File:** `hermes-relay/src/actions.rs`

Add re-exports:

```rust
pub use hermes_substream::ACTION_SUBSPACE_UNVERIFIED as SUBSPACE_UNVERIFIED;
pub use hermes_substream::ACTION_SUBSPACE_UNRELATED as SUBSPACE_UNRELATED;
pub use hermes_substream::ACTION_SUBSPACE_TOPIC_REMOVED as SUBSPACE_TOPIC_REMOVED;
```

---

### Phase 2: Live Substream Configuration

**Goal:** Replace `StreamSource::mock()` with configurable live substream.

#### 2.1 Add environment variable parsing

**File:** `atlas/src/main.rs`

Add env var parsing (following hermes-pipeline pattern):

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `USE_MOCK` | bool | `false` | Use mock data source |
| `SUBSTREAMS_ENDPOINT` | String | `geotest.substreams.pinax.network:443` | Substream endpoint |
| `SUBSTREAMS_START_BLOCK` | i64 | `82655` | Start block number |
| `SUBSTREAMS_END_BLOCK` | u64 | `u64::MAX` | End block number |

#### 2.2 Update source initialization

**File:** `atlas/src/main.rs` (around line 265)

Replace hardcoded mock with conditional:

```rust
let source = if use_mock {
    info!("Using mock data source");
    StreamSource::mock()
} else {
    info!(
        endpoint = %endpoint,
        start_block = start_block,
        end_block = end_block,
        "Using live substream"
    );
    StreamSource::live(endpoint, HermesModule::Actions, start_block, end_block)
};
```

---

### Phase 3: Membership + Edge Removal (RFC 0001)

**Goal:** Treat Editor/Member edges as canonical-granting and handle all edge removals.

#### 3.1 Add new EdgeType variants

**File:** `atlas/src/graph/tree.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeType {
    Root,
    Verified,
    Related,
    Topic,
    Editor,  // NEW
    Member,  // NEW
}
```

#### 3.2 Add new TrustExtension variants

**File:** `atlas/src/events.rs`

```rust
pub enum TrustExtension {
    // Existing
    Verified { target_space_id: SpaceId },
    Related { target_space_id: SpaceId },
    Subtopic { target_topic_id: TopicId },
    
    // NEW: Additions
    EditorAdded { member_space_id: SpaceId },
    MemberAdded { member_space_id: SpaceId },
    
    // NEW: Removals
    VerifiedRemoved { target_space_id: SpaceId },
    RelatedRemoved { target_space_id: SpaceId },
    EditorRemoved { member_space_id: SpaceId },
    MemberRemoved { member_space_id: SpaceId },
    SubtopicRemoved { target_topic_id: TopicId },
}
```

#### 3.3 Add action conversion handlers

**File:** `atlas/src/convert.rs`

Add handlers for new actions. **Note:** Verify byte ranges against on-chain format during implementation.

| Action | Conversion | Data Extraction |
|--------|------------|-----------------|
| `EDITOR_ADDED` | `TrustExtension::EditorAdded` | `member_space_id` = `action.topic[0..16]` |
| `EDITOR_REMOVED` | `TrustExtension::EditorRemoved` | `member_space_id` = `action.topic[0..16]` |
| `MEMBER_ADDED` | `TrustExtension::MemberAdded` | `member_space_id` = `action.topic[0..16]` |
| `MEMBER_REMOVED` | `TrustExtension::MemberRemoved` | `member_space_id` = `action.topic[0..16]` |
| `SUBSPACE_UNVERIFIED` | `TrustExtension::VerifiedRemoved` | `target_space_id` = `action.topic[0..16]` |
| `SUBSPACE_UNRELATED` | `TrustExtension::RelatedRemoved` | `target_space_id` = `action.topic[0..16]` |
| `SUBSPACE_TOPIC_REMOVED` | `TrustExtension::SubtopicRemoved` | `target_topic_id` = `action.topic[16..32]` |

#### 3.4 Update GraphState for edge storage and removal

**File:** `atlas/src/graph/state.rs`

Update `apply_trust_extended()` to handle new variants:

```rust
fn apply_trust_extended(&mut self, extended: &TrustExtended) {
    match &extended.extension {
        // Additions (existing + new)
        TrustExtension::Verified { target_space_id } => {
            self.add_explicit_edge(extended.source_space_id, *target_space_id, EdgeType::Verified);
        }
        TrustExtension::Related { target_space_id } => {
            self.add_explicit_edge(extended.source_space_id, *target_space_id, EdgeType::Related);
        }
        TrustExtension::EditorAdded { member_space_id } => {
            self.add_explicit_edge(extended.source_space_id, *member_space_id, EdgeType::Editor);
        }
        TrustExtension::MemberAdded { member_space_id } => {
            self.add_explicit_edge(extended.source_space_id, *member_space_id, EdgeType::Member);
        }
        TrustExtension::Subtopic { target_topic_id } => {
            self.add_topic_edge(extended.source_space_id, *target_topic_id);
        }
        
        // Removals (new)
        TrustExtension::VerifiedRemoved { target_space_id } => {
            self.remove_explicit_edge(extended.source_space_id, *target_space_id, EdgeType::Verified);
        }
        TrustExtension::RelatedRemoved { target_space_id } => {
            self.remove_explicit_edge(extended.source_space_id, *target_space_id, EdgeType::Related);
        }
        TrustExtension::EditorRemoved { member_space_id } => {
            self.remove_explicit_edge(extended.source_space_id, *member_space_id, EdgeType::Editor);
        }
        TrustExtension::MemberRemoved { member_space_id } => {
            self.remove_explicit_edge(extended.source_space_id, *member_space_id, EdgeType::Member);
        }
        TrustExtension::SubtopicRemoved { target_topic_id } => {
            self.remove_topic_edge(extended.source_space_id, *target_topic_id);
        }
    }
}
```

Add helper methods:

```rust
fn remove_explicit_edge(&mut self, source: SpaceId, target: SpaceId, edge_type: EdgeType) {
    if let Some(edges) = self.explicit_edges.get_mut(&source) {
        edges.retain(|(t, et)| !(*t == target && *et == edge_type));
    }
}

fn remove_topic_edge(&mut self, source: SpaceId, topic_id: TopicId) {
    // Remove from forward index
    if let Some(topics) = self.topic_edges.get_mut(&source) {
        topics.retain(|t| *t != topic_id);
    }
    // Remove from reverse index
    if let Some(sources) = self.topic_edge_sources.get_mut(&topic_id) {
        sources.remove(&source);
    }
}
```

#### 3.5 Update transitive BFS to include membership edges

**File:** `atlas/src/graph/transitive.rs`

Membership edges (Editor/Member) are already explicit edges stored in `explicit_edges`, so BFS should already traverse them. Verify this works correctly.

#### 3.6 Update protobuf schema

**File:** `hermes-schema/proto/topology.proto`

Add to `CanonicalTreeNode.edge` oneof:

```protobuf
message CanonicalTreeNode {
  bytes space_id = 1;
  
  oneof edge {
    RootEdge root = 2;
    VerifiedEdge verified = 3;
    RelatedEdge related = 4;
    TopicEdge topic = 5;
    EditorEdge editor = 6;  // NEW
    MemberEdge member = 7;  // NEW
  }
  
  repeated CanonicalTreeNode children = 8;
}

// NEW
message EditorEdge {}
message MemberEdge {}
```

#### 3.7 Update Kafka emitter

**File:** `atlas/src/kafka/emitter.rs`

Update `tree_node_to_proto()` to handle new edge types:

```rust
EdgeType::Editor => proto::canonical_tree_node::Edge::Editor(proto::EditorEdge {}),
EdgeType::Member => proto::canonical_tree_node::Edge::Member(proto::MemberEdge {}),
```

---

### Phase 4: Graph Diff Emission (RFC 0002)

**Goal:** Replace full snapshot emission with incremental diffs using ADDED/REMOVED/MOVED semantics.

#### 4.1 Add diff protobuf messages

**File:** `hermes-schema/proto/topology.proto`

```protobuf
// Incremental diff for canonical graph changes (replaces CanonicalGraphUpdated)
message CanonicalGraphDiff {
  bytes root_id = 1;
  repeated NodeChange changes = 2;
  blockchain_metadata.BlockchainMetadata meta = 3;
}

// A single node change in a diff
message NodeChange {
  bytes space_id = 1;
  ChangeType type = 2;
  optional uint32 distance = 3;      // Required for ADDED/MOVED
  optional EdgeInfo parent_edge = 4; // Required for ADDED/MOVED
}

// Information about the edge used to reach a node
message EdgeInfo {
  bytes source = 1;  // Parent node space_id
  oneof edge_type {
    VerifiedEdge verified = 2;
    RelatedEdge related = 3;
    TopicEdge topic = 4;
    EditorEdge editor = 5;
    MemberEdge member = 6;
  }
}

enum ChangeType {
  ADDED = 0;
  REMOVED = 1;
  MOVED = 2;
}
```

#### 4.2 Add graph hashing module

**New file:** `atlas/src/graph/hash.rs`

```rust
use super::{CanonicalGraph, TreeNode};

/// Compute a hash of the canonical graph for change detection.
/// Uses the existing hash_tree function.
pub fn hash_graph(graph: &CanonicalGraph) -> u64 {
    super::hash_tree(&graph.tree)
}
```

Export from `atlas/src/graph/mod.rs`.

#### 4.3 Create DiffTracker component

**New file:** `atlas/src/graph/diff.rs`

```rust
use super::{CanonicalGraph, EdgeType, TreeNode};
use crate::events::{SpaceId, TopicId};
use std::collections::HashMap;

/// A position in the canonical tree
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub distance: u32,
    pub parent: SpaceId,
    pub edge_type: EdgeType,
    pub topic_id: Option<TopicId>,
}

/// A single change in a graph diff
#[derive(Debug, Clone)]
pub struct NodeChange {
    pub space_id: SpaceId,
    pub change_type: ChangeType,
    pub position: Option<Position>, // Present for ADDED/MOVED
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Added,
    Removed,
    Moved,
}

/// Diff between two graph states
#[derive(Debug, Clone, Default)]
pub struct GraphDiff {
    pub changes: Vec<NodeChange>,
}

impl GraphDiff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Tracks previous graph state and computes diffs.
/// Separated from CanonicalProcessor to maintain single responsibility.
#[derive(Debug, Default)]
pub struct DiffTracker {
    last_positions: Option<HashMap<SpaceId, Position>>,
}

impl DiffTracker {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Track a new graph and compute diff from previous state.
    /// Returns the diff (which may be empty if nothing changed).
    pub fn track(&mut self, graph: &CanonicalGraph) -> GraphDiff {
        let new_positions = build_position_map(&graph.tree);
        let diff = compute_diff(self.last_positions.as_ref(), &new_positions);
        self.last_positions = Some(new_positions);
        diff
    }
}

/// Compute diff between old and new position maps using sorted vector merge.
/// Returns changes sorted by space_id for deterministic output.
fn compute_diff(
    old: Option<&HashMap<SpaceId, Position>>,
    new: &HashMap<SpaceId, Position>,
) -> GraphDiff {
    let old_positions = old.cloned().unwrap_or_default();
    
    // Convert to sorted vectors for merge
    let mut old_vec: Vec<_> = old_positions.into_iter().collect();
    let mut new_vec: Vec<_> = new.iter().map(|(k, v)| (*k, *v)).collect();
    old_vec.sort_by_key(|(id, _)| *id);
    new_vec.sort_by_key(|(id, _)| *id);
    
    // Merge-join to find changes
    let mut changes = Vec::new();
    let mut old_iter = old_vec.into_iter().peekable();
    let mut new_iter = new_vec.into_iter().peekable();
    
    loop {
        match (old_iter.peek(), new_iter.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                let (space_id, _) = old_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id,
                    change_type: ChangeType::Removed,
                    position: None,
                });
            }
            (None, Some(_)) => {
                let (space_id, pos) = new_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id,
                    change_type: ChangeType::Added,
                    position: Some(pos),
                });
            }
            (Some((old_id, _)), Some((new_id, _))) => {
                match old_id.cmp(new_id) {
                    std::cmp::Ordering::Less => {
                        let (space_id, _) = old_iter.next().unwrap();
                        changes.push(NodeChange {
                            space_id,
                            change_type: ChangeType::Removed,
                            position: None,
                        });
                    }
                    std::cmp::Ordering::Greater => {
                        let (space_id, pos) = new_iter.next().unwrap();
                        changes.push(NodeChange {
                            space_id,
                            change_type: ChangeType::Added,
                            position: Some(pos),
                        });
                    }
                    std::cmp::Ordering::Equal => {
                        let (space_id, old_pos) = old_iter.next().unwrap();
                        let (_, new_pos) = new_iter.next().unwrap();
                        if old_pos != new_pos {
                            changes.push(NodeChange {
                                space_id,
                                change_type: ChangeType::Moved,
                                position: Some(new_pos),
                            });
                        }
                    }
                }
            }
        }
    }
    
    GraphDiff { changes }
}

/// Build a map of space_id -> Position from tree traversal
fn build_position_map(tree: &TreeNode) -> HashMap<SpaceId, Position> {
    let mut map = HashMap::new();
    build_position_map_recursive(tree, &mut map, 0, tree.space_id, EdgeType::Root, None);
    map
}

fn build_position_map_recursive(
    node: &TreeNode,
    map: &mut HashMap<SpaceId, Position>,
    distance: u32,
    parent: SpaceId,
    edge_type: EdgeType,
    topic_id: Option<TopicId>,
) {
    // Don't include root in diff (it's implicit)
    if distance > 0 {
        map.insert(node.space_id, Position {
            distance,
            parent,
            edge_type,
            topic_id,
        });
    }
    
    for child in &node.children {
        build_position_map_recursive(
            child,
            map,
            distance + 1,
            node.space_id,
            child.edge_type,
            child.topic_id,
        );
    }
}
```

Export from `atlas/src/graph/mod.rs`.

#### 4.4 Update emitter for diff emission

**File:** `atlas/src/kafka/emitter.rs`

Replace full snapshot emission with diff emission:

```rust
impl CanonicalGraphEmitter {
    /// Emit a canonical graph diff to Kafka.
    pub async fn emit(
        &self,
        root_id: &[u8; 16],
        diff: &GraphDiff,
        meta: &BlockMetadata,
    ) -> Result<(), AtlasError> {
        if diff.is_empty() {
            return Ok(());
        }
        
        let proto = self.diff_to_proto(root_id, diff, meta);
        let payload = proto.encode_to_vec();
        
        self.producer.send(&self.topic, root_id, &payload).await
    }
    
    fn diff_to_proto(
        &self,
        root_id: &[u8; 16],
        diff: &GraphDiff,
        meta: &BlockMetadata,
    ) -> proto::CanonicalGraphDiff {
        proto::CanonicalGraphDiff {
            root_id: root_id.to_vec(),
            changes: diff.changes.iter().map(node_change_to_proto).collect(),
            meta: Some(block_meta_to_proto(meta)),
        }
    }
}

fn node_change_to_proto(change: &NodeChange) -> proto::NodeChange {
    proto::NodeChange {
        space_id: change.space_id.to_vec(),
        r#type: match change.change_type {
            ChangeType::Added => proto::ChangeType::Added as i32,
            ChangeType::Removed => proto::ChangeType::Removed as i32,
            ChangeType::Moved => proto::ChangeType::Moved as i32,
        },
        distance: change.position.as_ref().map(|p| p.distance),
        parent_edge: change.position.as_ref().map(position_to_edge_info),
    }
}

fn position_to_edge_info(pos: &Position) -> proto::EdgeInfo {
    proto::EdgeInfo {
        source: pos.parent.to_vec(),
        edge_type: Some(match pos.edge_type {
            EdgeType::Verified => proto::edge_info::EdgeType::Verified(proto::VerifiedEdge {}),
            EdgeType::Related => proto::edge_info::EdgeType::Related(proto::RelatedEdge {}),
            EdgeType::Topic => proto::edge_info::EdgeType::Topic(proto::TopicEdge {
                topic_id: pos.topic_id.map(|t| t.to_vec()).unwrap_or_default(),
            }),
            EdgeType::Editor => proto::edge_info::EdgeType::Editor(proto::EditorEdge {}),
            EdgeType::Member => proto::edge_info::EdgeType::Member(proto::MemberEdge {}),
            EdgeType::Root => unreachable!("Root nodes don't appear in diffs"),
        }),
    }
}
```

#### 4.5 Wire up in main

**File:** `atlas/src/main.rs`

1. Create `DiffTracker` alongside `CanonicalProcessor`
2. Update processing loop to compute diffs and emit

```rust
// In main() or AtlasSink initialization
let mut diff_tracker = DiffTracker::new();

// In process_event()
if let Some(graph) = canonical_processor.compute(&state, &mut transitive) {
    let diff = diff_tracker.track(&graph);
    if !diff.is_empty() {
        emitter.emit(&root_id, &diff, &meta).await?;
    }
}
```

---

## Acceptance Criteria

### Functional Requirements

- [ ] Atlas can run against live substream with `USE_MOCK=false`
- [ ] Editor/Member edges are treated as canonical-granting (RFC 0001)
- [ ] Edge removals (`SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, `SUBSPACE_TOPIC_REMOVED`, `EDITOR_REMOVED`, `MEMBER_REMOVED`) correctly update graph state
- [ ] Graph diffs are emitted with ADDED/REMOVED/MOVED semantics (RFC 0002)
- [ ] MOVED changes include distance and parent edge info
- [ ] When a node is removed, all descendants are explicitly listed as REMOVED
- [ ] When a node moves, all descendants with changed distances are listed as MOVED
- [ ] Bootstrap scenario emits diff with all nodes as ADDED
- [ ] Empty diffs are not emitted
- [ ] Full snapshot emission is removed (diffs only)

### Non-Functional Requirements

- [ ] Diff computation completes in <100ms for 100k nodes
- [ ] Diffs are deterministic (sorted by space_id)
- [ ] All new code has unit tests
- [ ] Integration test validates end-to-end diff emission

### Quality Gates

- [ ] `cargo test` passes in atlas crate
- [ ] `cargo clippy` has no warnings
- [ ] Protobuf schema compiles successfully

---

## Dependencies & Prerequisites

- `hermes-substream` must be updated with new action constants (Phase 1)
- `hermes-relay` must re-export new actions (Phase 1)
- `hermes-schema` protobuf changes must be committed before Rust code that uses them

---

## Risk Analysis & Mitigation

| Risk | Impact | Mitigation |
|------|--------|------------|
| Live substream data format differs from mock | High | Test with staging substream first; add validation |
| Edge removal events not yet emitted on-chain | Medium | Code is ready; will work when events arrive |
| Diff computation performance at scale | Medium | Benchmarks show acceptable performance; incremental optimization documented for future |
| Action byte range extraction incorrect | Medium | Verify against on-chain format during implementation |

---

## Key Files

| File | Phase | Changes |
|------|-------|---------|
| `hermes-substream/src/lib.rs` | 1 | Add removal action constants |
| `hermes-relay/src/actions.rs` | 1 | Re-export removal actions |
| `atlas/src/main.rs` | 2, 4 | Env vars, DiffTracker wiring |
| `atlas/src/graph/tree.rs` | 3 | `EdgeType::Editor/Member` |
| `atlas/src/events.rs` | 3 | `TrustExtension` removal variants |
| `atlas/src/convert.rs` | 3 | Action conversion handlers |
| `atlas/src/graph/state.rs` | 3 | Edge removal methods (with reverse index cleanup) |
| `atlas/src/graph/hash.rs` | 4 | NEW - graph hashing |
| `atlas/src/graph/diff.rs` | 4 | NEW - DiffTracker + diff computation |
| `atlas/src/kafka/emitter.rs` | 4 | Updated emit() for diffs |
| `hermes-schema/proto/topology.proto` | 3, 4 | Edge types + diff messages |

---

## References & Research

### Internal References

- Brainstorm: [2026-02-04-atlas-update-brainstorm.md](../brainstorms/2026-02-04-atlas-update-brainstorm.md)
- RFC 0001: [Canonical Graph Inputs](../rfcs/0001-canonical-graph-inputs.md)
- RFC 0002: [Graph Diff Emission](../rfcs/0002-graph-diff-emission.md)
- Action data mapping: [hermes-pipeline/docs/action-data-mapping.md](../../hermes-pipeline/docs/action-data-mapping.md)

### Key Patterns

- Event handling: `atlas/src/events.rs:32-76`
- Edge types: `atlas/src/graph/tree.rs:9-19`
- Action conversion: `atlas/src/convert.rs:44-58`
- Graph state: `atlas/src/graph/state.rs:44-100`
- Kafka emitter: `atlas/src/kafka/emitter.rs:38-98`
- Env config: `atlas/src/main.rs:7-28, 261-264`

---

## Review Feedback Applied

Changes based on code review:

1. ✅ **Reverse index cleanup** - `remove_topic_edge` now updates `topic_edge_sources`
2. ✅ **Separate DiffTracker** - Diff tracking separated from CanonicalProcessor
3. ✅ **Store positions only** - DiffTracker stores `HashMap<SpaceId, Position>` not full graph
4. ✅ **Use existing emitter** - Updated `emit()` signature for diffs, no separate DiffEmitter
5. ✅ **Removed TransitiveGraphDiff** - Focus on canonical diffs only
6. ✅ **Hash via function** - Added `hash_graph()` function instead of field on struct
7. ✅ **Kept unreachable!()** - Invariant is solid; crash is appropriate for impossible state
8. ✅ **Kept ADDED = 0** - Simpler; we trust the producer
