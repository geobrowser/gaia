# 0001: Canonical Graph Implementation Plan

## Summary

This document outlines the plan to implement push-based canonical graph computation for Atlas, integrated with the Hermes event streaming infrastructure.

## Background

### Current State

Atlas is currently in documentation phase. The conceptual design exists in `atlas/docs/`:
- `graph-concepts.md` - Core graph concepts and algorithms
- `storage.md` - Storage structure design
- `topics-and-edits.md` - Event model for communities and topics

Hermes infrastructure is operational with:
- Kafka broker and producer
- Protocol buffer messages for space creation and trust extensions (`HermesCreateSpace`, `HermesSpaceTrustExtension`)
- Message serialization and emission working

### Problem

The current design is pull-based: canonical graphs are computed on-demand at query time. We need to make this push-based so that:
1. Canonical graph changes are emitted as messages to the event stream
2. Downstream systems can consume these messages to stay up-to-date
3. Distance from root is preserved in the emitted tree structure (important for downstream consumers)

## Understanding

### Core Concepts

**Spaces and Topics:**
- A Space is created via `HermesCreateSpace` and announces a topic (`topic_id`) at creation
- Spaces can point to other topics via `SubtopicExtension`, creating topic edges
- Topic resolution: find all spaces that announced a given topic

**Edge Types:**
- **Explicit Edges**: Direct space-to-space connections (`VerifiedExtension`, `RelatedExtension`)
- **Topic Edges**: Indirect connections through topic membership (`SubtopicExtension`)

**Graph Types (from original docs):**

1. **Global Graph**: Complete graph with all nodes and edges (source of truth)
2. **Local Graph**: Per-node, immediate children only (one-hop view)
3. **Transitive Graph**: Per-node, all reachable nodes as a DAG
4. **Canonical Graph**: Single global graph from root with trust boundary rules

**Canonical Graph:**
- Computed from a known root node
- Uses two-phase BFS algorithm:
  - Phase 1: Traverse explicit edges only to establish canonical (trusted) nodes
  - Phase 2: Add topic edges, but only connecting nodes already in the canonical set
- Trust flows only through explicit edges; topic edges cannot grant trust

**Transitive Graph:**
- Computed per-space
- Standard BFS traversal (explicit + topic edges)
- No trust boundary restrictions

**Key Design Decisions:**
- Single global canonical graph with a known root
- Per-space transitive graphs
- Emit full tree structure on each change (not deltas)
- Tree structure matters because downstream systems care about distance from root
- Persist graphs durably to PostgreSQL
- Only emit when graph actually changes (hash-based change detection)
- Separate processors for different graph types (Canonical, Transitive)

### Message Mapping

| Hermes Message | Graph Effect |
|----------------|--------------|
| `HermesCreateSpace { space_id, topic_id }` | Add node, associate space with topic |
| `VerifiedExtension { source, target }` | Add explicit edge (verified/trust) |
| `RelatedExtension { source, target }` | Add explicit edge (related) |
| `SubtopicExtension { source, target_topic_id }` | Add topic edge (source space → topic) |

### Topic Edge Resolution

When a space points to a topic via `SubtopicExtension`:
1. Look up all spaces that announced that topic (via `HermesCreateSpace`)
2. Filter to spaces already in the canonical set
3. Add those spaces and their transitive subtrees to the tree

## Implementation Plan

### 1. Protobuf Message Definition

Create `hermes-schema/proto/topology.proto`:

```protobuf
syntax = "proto3";
package topology;

message CanonicalGraphUpdated {
  bytes root_id = 1;
  CanonicalTreeNode tree = 2;
  repeated bytes canonical_space_ids = 3;
  uint64 sequence_number = 4;
  uint64 timestamp = 5;
}

message CanonicalTreeNode {
  bytes space_id = 1;
  EdgeType edge_type = 2;
  bytes topic_id = 3;  // Only set when edge_type is TOPIC
  repeated CanonicalTreeNode children = 4;
}

enum EdgeType {
  EDGE_TYPE_UNSPECIFIED = 0;
  EDGE_TYPE_VERIFIED = 1;
  EDGE_TYPE_RELATED = 2;
  EDGE_TYPE_TOPIC = 3;
}
```

### 2. Atlas Architecture

Atlas is the topology system. It maintains the global graph state and runs separate processors for different graph computations.

**Shared Graph State:**

The global graph state is shared across all processors:

```rust
struct GraphState {
    // Nodes
    spaces: HashMap<SpaceId, SpaceMetadata>,

    // Space → announced topic (1:1 from creation)
    space_topic: HashMap<SpaceId, TopicId>,

    // Topic → spaces that announced it (reverse index)
    topic_spaces: HashMap<TopicId, HashSet<SpaceId>>,

    // Explicit edges: source → target → edge type
    explicit_edges: HashMap<SpaceId, HashMap<SpaceId, EdgeType>>,

    // Topic edges: source → topic_id (from SubtopicExtension)
    topic_edges: HashMap<SpaceId, HashSet<TopicId>>,
}
```

**Processors:**

Each processor subscribes to graph state changes and computes its specific graph type:

1. **Canonical Processor**
   - Computes single global canonical graph from root
   - Two-phase BFS algorithm (explicit edges first, then topic edges)
   - Emits `CanonicalGraphUpdated` on changes

2. **Transitive Processor** (future)
   - Computes per-space transitive graphs
   - Standard BFS (explicit + topic edges)
   - Could be push-based (emit on change) or pull-based (compute on request)
   - Emits `TransitiveGraphUpdated` per affected space

**Processing Flow:**

```
Kafka (HermesCreateSpace, HermesSpaceTrustExtension)
    ↓
Atlas::process_event()
    ↓
Update GraphState
    ↓
Notify Processors
    ↓
┌─────────────────────────────────────────────┐
│  Canonical Processor    Transitive Processor │
│         ↓                      ↓             │
│  Recompute graph       Recompute affected    │
│  Hash & compare        graphs per space      │
│         ↓                      ↓             │
│  If changed: emit      If changed: emit      │
└─────────────────────────────────────────────┘
    ↓
Persist to PostgreSQL + Emit to Kafka
```

### 3. Canonical Graph Algorithm

Two-phase BFS as specified in `graph-concepts.md`:

**Phase 1 - Explicit Edges Only:**
1. Initialize visited set, tree nodes map, queue, deferred topic edges
2. Start from root node
3. BFS traversal processing only explicit edges
4. Collect topic edges for phase 2
5. Result: visited set contains all canonical nodes

**Phase 2 - Topic Edge Addition:**
1. For each deferred topic edge (source → topic_id):
   - Resolve topic to spaces that announced it
   - Filter to spaces in canonical set (visited)
   - For each canonical member:
     - Add edge from source to member
     - Recursively add member's transitive subtree (only canonical descendants)
2. Result: complete tree with topic edges included

**Implementation Details:**
- Deterministic child ordering (sort by SpaceId) for consistent hashing
- Tree serialization and hashing for change detection
- BFS "first visit wins" handles cycles naturally

### 4. PostgreSQL Schema

```sql
-- Current canonical graph (single row, updated on change)
CREATE TABLE canonical_graph (
    id INTEGER PRIMARY KEY DEFAULT 1,
    root_id BYTEA NOT NULL,
    tree JSONB NOT NULL,
    canonical_space_ids BYTEA[] NOT NULL,
    sequence_number BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT single_row CHECK (id = 1)
);

-- Topology state snapshot for fast startup
CREATE TABLE topology_state (
    id INTEGER PRIMARY KEY DEFAULT 1,
    state JSONB NOT NULL,
    last_cursor TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### 5. Kafka Topics

- **Input**: Existing topic(s) with `HermesCreateSpace` and `HermesSpaceTrustExtension` messages
- **Output**: New topic for `CanonicalGraphUpdated` messages

## File Structure

```
gaia/
├── hermes-schema/
│   └── proto/
│       └── topology.proto              # Graph update messages
└── atlas/
    ├── Cargo.toml
    ├── src/
    │   ├── main.rs                     # Kafka consumer/producer setup
    │   ├── lib.rs
    │   ├── state.rs                    # GraphState struct and event handling
    │   ├── processor/
    │   │   ├── mod.rs                  # Processor trait
    │   │   ├── canonical.rs            # Canonical graph processor (two-phase BFS)
    │   │   └── transitive.rs           # Transitive graph processor (future)
    │   ├── hash.rs                     # Tree hashing for change detection
    │   └── persistence.rs              # PostgreSQL read/write
    └── docs/
        ├── graph-concepts.md
        ├── storage.md
        ├── topics-and-edits.md
        └── agents/
            └── plans/
                └── 0001-canonical-graph-implementation-plan.md
```

## Future Optimizations

### Subtree Reuse in Phase 2

The current Phase 2 algorithm re-traverses subtrees when adding canonical members via topic edges. Since these subtrees were already computed in Phase 1, we can cache and reuse them:

```rust
// During Phase 1, store each node's computed subtree
node_trees: HashMap<SpaceId, TreeNode>

// Phase 2: reuse instead of re-traversing
if canonical_set.contains(&member) {
    let subtree = node_trees.get(&member).clone();
    source_node.add_child(subtree);
}
```

This reduces Phase 2 from O(topic_edges × subtree_size) to O(topic_edges × clone_cost).

### Incremental Canonical Graph Updates

Instead of full recomputation on every event, track what changed and update incrementally.

**Data Structures:**

```rust
struct IncrementalState {
    // Current canonical set
    canonical_set: HashSet<SpaceId>,

    // Current tree structure
    tree: TreeNode,

    // Reverse index: which spaces does each topic edge affect?
    // topic_id → set of spaces that have SubtopicExtension pointing to this topic
    topic_edge_sources: HashMap<TopicId, HashSet<SpaceId>>,
}
```

**Incremental Update Rules:**

1. **New space created (`HermesCreateSpace`)**
   - Add to `topic_spaces[topic_id]`
   - If any canonical space has a topic edge to this topic:
     - Check if new space is canonical (reachable via explicit edges)
     - If yes, it was already added in Phase 1 when it became canonical
     - Topic edges only add connections, not new canonical nodes

2. **New explicit edge added (`VerifiedExtension`, `RelatedExtension`)**
   - If source is canonical and target is not yet canonical:
     - Add target (and its explicit-edge descendants) to canonical set
     - Re-evaluate topic edges: any topic edge pointing to topics announced by newly-canonical spaces may now resolve to more members
   - If both source and target already canonical:
     - Tree structure changes (new edge), but canonical set unchanged
     - May affect depth of target in tree

3. **New topic edge added (`SubtopicExtension`)**
   - Look up `topic_spaces[target_topic_id]`
   - Filter to canonical members
   - Add edges from source to canonical members (with their subtrees)
   - Does not expand canonical set

4. **Space removed / Edge removed**
   - More complex: need to check if removed edge was the only path to some nodes
   - May need full recomputation, or maintain reference counts

**When to Fall Back to Full Recomputation:**

- Removal events (edge or space deleted)
- When incremental update touches > 50% of canonical set
- Periodic full recomputation to correct any drift

**Trade-offs:**

| Approach | Pros | Cons |
|----------|------|------|
| Full recomputation | Simple, always correct | O(canonical_set) per event |
| Incremental | Fast for small changes | Complex, edge cases, removal is hard |

**Recommendation:**

Start with full recomputation + subtree reuse. Add incremental updates later if profiling shows recomputation is a bottleneck. The canonical set is likely small relative to event frequency, making full recomputation acceptable initially.

## Open Questions

1. **Root Configuration**: How is the root space ID provided? Environment variable? Database config?
2. **Startup Behavior**: On startup, should we rebuild state from Kafka or load snapshot from PostgreSQL?
3. **Removal Events**: Are there events for removing spaces or edges? Current messages only cover creation/extension.

## Next Steps

1. Define protobuf messages in `hermes-schema/proto/topology.proto`
2. Set up `atlas` crate with basic structure
3. Implement `GraphState` and event processing
4. Implement processor trait and `CanonicalProcessor` (two-phase BFS)
5. Add tree hashing and change detection
6. Add PostgreSQL persistence
7. Add Kafka consumer/producer integration
8. Testing and integration
9. (Future) Implement `TransitiveProcessor` for per-space graphs
