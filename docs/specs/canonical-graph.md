# Canonical Graph Specification

This document specifies the data model, algorithms, and wire format for the Atlas canonical graph system.

## Overview

The canonical graph system determines which spaces are "canonical" (trusted/verified) relative to a root space. It:

- **Computes reachability**: Finds all spaces reachable from a root via explicit trust edges
- **Builds a tree**: Represents the canonical set as a tree structure with edge metadata
- **Emits incremental diffs**: Publishes ADDED/REMOVED/MOVED changes to Kafka

## Data Model

### Space IDs

Spaces are identified by 16-byte identifiers:

```rust
type SpaceId = [u8; 16];
type TopicId = [u8; 16];
```

### Edge Types

Six edge types connect spaces in the graph:

```rust
enum EdgeType {
    Root,      // The root space (implicit, not stored as an edge)
    Verified,  // Explicit trust: "I verify this space"
    Related,   // Explicit trust: "This space is related to me"
    Topic,     // Indirect: "Spaces sharing this topic" (non-canonical-granting)
    Editor,    // Membership: "This personal space is an editor"
    Member,    // Membership: "This personal space is a member"
}
```

**Canonical-granting edges**: `Verified`, `Related`, `Editor`, `Member`
**Non-canonical-granting edges**: `Topic` (only connects already-canonical nodes)

### Graph State

The graph state tracks all edges in the system:

```rust
struct GraphState {
    // Explicit edges: source -> [(target, edge_type)]
    explicit_edges: HashMap<SpaceId, Vec<(SpaceId, EdgeType)>>,
    
    // Topic edges: source -> {topic_ids}
    topic_edges: HashMap<SpaceId, HashSet<TopicId>>,
    
    // Topic membership: topic -> {member_space_ids}
    topic_members: HashMap<TopicId, HashSet<SpaceId>>,
    
    // Reverse indices for O(1) lookup
    topic_edge_sources: HashMap<TopicId, HashSet<SpaceId>>,
}
```

### Canonical Graph

The computed canonical graph for a root space:

```rust
struct CanonicalGraph {
    root: SpaceId,
    tree: TreeNode,              // Tree structure with edge metadata
    flat: HashSet<SpaceId>,      // Flat set for O(1) membership checks
    hash: u64,                   // Hash for change detection
}

struct TreeNode {
    space_id: SpaceId,
    edge_type: EdgeType,
    topic_id: Option<TopicId>,   // Present only for Topic edges
    children: Vec<TreeNode>,
}
```

## Canonical Inclusion Rules

A space is canonical if and only if:

1. It is the root space, OR
2. It is reachable from the root via explicit edges (`Verified`, `Related`, `Editor`, `Member`)

**Topic edges never grant canonical inclusion.** They only add connections between nodes that are already canonical via explicit edges.

### Edge Semantics

| Edge Type | Source | Target | Grants Canonical? |
|-----------|--------|--------|-------------------|
| `Verified` | Any space | Any space | Yes |
| `Related` | Any space | Any space | Yes |
| `Editor` | DAO/org space | Personal space | Yes |
| `Member` | DAO/org space | Personal space | Yes |
| `Topic` | Any canonical space | Spaces with matching topic | No |

### Transitive Closure

The canonical set is computed as the transitive closure of explicit edges from the root:

```
canonical = {root} ∪ {s : s is reachable from root via explicit edges}
```

**Editor/Member targets are treated as normal spaces.** If a personal space (reached via Editor edge) has its own outgoing explicit edges, those are followed transitively.

Example:
```
Root → DAO_A (via Verified)
DAO_A → PersonalSpace_B (via Editor)
PersonalSpace_B → SomeOtherSpace_C (via Verified)
```
Result: `{Root, DAO_A, PersonalSpace_B, SomeOtherSpace_C}` are all canonical.

## Algorithms

### BFS Traversal

The canonical graph is computed using breadth-first search:

```
function compute_canonical(root, state):
    visited = {root}
    queue = [root]
    tree = TreeNode(root, Root)
    
    while queue is not empty:
        current = queue.pop_front()
        
        # Follow explicit edges (canonical-granting)
        for (target, edge_type) in state.explicit_edges[current]:
            if target not in visited:
                visited.add(target)
                queue.push(target)
                tree.add_child(current, target, edge_type)
        
        # Follow topic edges (non-canonical-granting, only for already-visited)
        for topic_id in state.topic_edges[current]:
            for member in state.topic_members[topic_id]:
                if member not in visited:
                    visited.add(member)
                    queue.push(member)
                    tree.add_child(current, member, Topic, topic_id)
    
    return CanonicalGraph(root, tree, visited)
```

**Key properties:**
- Time complexity: O(V + E) where V = spaces, E = edges
- First path wins: A space appears once in the tree at its shortest path
- Deterministic: Edges are sorted by target SpaceId before processing

### Diff Computation

Diffs are computed by comparing position maps between graph states:

```rust
struct Position {
    distance: u32,           // Distance from root
    parent: SpaceId,         // Parent node
    edge_type: EdgeType,     // Edge type to parent
    topic_id: Option<TopicId>,
}
```

The diff algorithm uses sorted vector merge-join:

```
function compute_diff(old_positions, new_positions):
    changes = []
    old_iter = old_positions.sorted_by_space_id()
    new_iter = new_positions.sorted_by_space_id()
    
    while old_iter or new_iter has elements:
        if only old has current:
            changes.push(REMOVED, old.space_id)
            old_iter.next()
        else if only new has current:
            changes.push(ADDED, new.space_id, new.position)
            new_iter.next()
        else if old.space_id < new.space_id:
            changes.push(REMOVED, old.space_id)
            old_iter.next()
        else if old.space_id > new.space_id:
            changes.push(ADDED, new.space_id, new.position)
            new_iter.next()
        else:  # same space_id
            if old.position != new.position:
                changes.push(MOVED, space_id, new.position)
            old_iter.next()
            new_iter.next()
    
    return changes
```

**Performance characteristics:**
- Time: O(n log n) dominated by sort
- Space: O(n) for position storage
- Allocations: Near-zero after warmup (buffer reuse)

### DiffTracker

The `DiffTracker` maintains state between diff computations:

```rust
struct DiffTracker {
    last_positions: Vec<(SpaceId, Position)>,  // Sorted by SpaceId
    scratch: Vec<(SpaceId, Position)>,         // Reusable buffer
    initialized: bool,
}
```

**Buffer reuse pattern:**
1. Build new positions into `scratch` buffer
2. Sort `scratch` by SpaceId
3. Compute diff between `last_positions` and `scratch`
4. Swap buffers: `last_positions` ↔ `scratch`

This achieves near-zero allocations after the first call.

## Wire Format (Protobuf)

### CanonicalGraphDiff

```protobuf
message CanonicalGraphDiff {
  bytes root_id = 1;
  repeated NodeChange changes = 2;
  BlockchainMetadata meta = 3;
}

message NodeChange {
  bytes space_id = 1;
  ChangeType change_type = 2;
  optional uint32 distance = 3;      // Present for ADDED/MOVED
  optional EdgeInfo parent_edge = 4; // Present for ADDED/MOVED
}

message EdgeInfo {
  bytes parent_id = 1;
  oneof edge_type {
    VerifiedEdge verified = 2;
    RelatedEdge related = 3;
    TopicEdge topic = 4;    // includes topic_id
    EditorEdge editor = 5;
    MemberEdge member = 6;
  }
}

enum ChangeType {
  CHANGE_TYPE_UNSPECIFIED = 0;
  CHANGE_TYPE_ADDED = 1;
  CHANGE_TYPE_REMOVED = 2;
  CHANGE_TYPE_MOVED = 3;
}
```

### Change Type Semantics

| Change Type | Meaning | Fields Present |
|-------------|---------|----------------|
| `ADDED` | Space became canonical | `distance`, `parent_edge` |
| `REMOVED` | Space is no longer canonical | (none) |
| `MOVED` | Space changed position in tree | `distance`, `parent_edge` |

**MOVED is emitted when:**
- Parent changed
- Edge type changed
- Distance from root changed

### Example Messages

**Node added to canonical set:**
```json
{
  "space_id": "<16 bytes>",
  "change_type": "ADDED",
  "distance": 2,
  "parent_edge": {
    "parent_id": "<parent's 16 bytes>",
    "verified": {}
  }
}
```

**Node removed from canonical set:**
```json
{
  "space_id": "<16 bytes>",
  "change_type": "REMOVED"
}
```

**Node moved to different parent:**
```json
{
  "space_id": "<16 bytes>",
  "change_type": "MOVED",
  "distance": 3,
  "parent_edge": {
    "parent_id": "<new parent's 16 bytes>",
    "topic": { "topic_id": "<16 bytes>" }
  }
}
```

## Event Processing

### Input Events

The system processes these blockchain events:

| Event | Effect | Can Expand Canonical? |
|-------|--------|----------------------|
| `SpaceCreated` | Add space, set topic membership | No |
| `TrustExtended::Verified` | Add verified edge | Yes |
| `TrustExtended::Related` | Add related edge | Yes |
| `TrustExtended::Subtopic` | Add topic edge | No |
| `TrustExtended::EditorAdded` | Add editor edge | Yes |
| `TrustExtended::MemberAdded` | Add member edge | Yes |
| `TrustExtended::VerifiedRemoved` | Remove verified edge | (can shrink) |
| `TrustExtended::RelatedRemoved` | Remove related edge | (can shrink) |
| `TrustExtended::SubtopicRemoved` | Remove topic edge | No |
| `TrustExtended::EditorRemoved` | Remove editor edge | (can shrink) |
| `TrustExtended::MemberRemoved` | Remove member edge | (can shrink) |

### Processing Pipeline

```
Substream Events
       │
       ▼
  GraphState.apply_event()     ← Update edges
       │
       ▼
  TransitiveProcessor.handle_event()  ← Invalidate caches
       │
       ▼
  CanonicalProcessor.compute()  ← Recompute if affected
       │
       ▼
  DiffTracker.track()          ← Compute diff
       │
       ▼
  CanonicalGraphEmitter.emit_diff()  ← Publish to Kafka
```

### Selective Recomputation

The canonical graph is only recomputed when an event affects it:

```rust
fn affects_canonical(event, canonical_set) -> bool {
    match event {
        SpaceCreated { space_id, .. } => {
            // Only if existing topic edges point to this space's topic
            canonical_set.has_topic_edge_to(space_id.topic)
        }
        TrustExtended { source, .. } => {
            // Only if source is canonical (can propagate changes)
            canonical_set.contains(source)
        }
    }
}
```

## Performance

### Benchmarks (Apple Silicon)

| Nodes | Bootstrap | No Change | Throughput |
|------:|----------:|----------:|-----------:|
| 1,000 | 37 µs | 33 µs | ~27-31 M nodes/s |
| 10,000 | 479 µs | 484 µs | ~20-21 M nodes/s |
| 50,000 | 3.2 ms | 3.1 ms | ~15-17 M nodes/s |
| 100,000 | 8.3 ms | 7.0 ms | ~12-14 M nodes/s |

### Memory Usage

| Component | Size per Node |
|-----------|---------------|
| Position storage | ~56 bytes |
| TreeNode | ~250 bytes |
| GraphState edge | ~125 bytes |

| Nodes | DiffTracker Memory |
|------:|-------------------:|
| 10,000 | ~560 KB |
| 100,000 | ~5.6 MB |
| 1,000,000 | ~56 MB |

### Optimization Techniques

1. **Sorted Vec over HashMap**: Positions stored as sorted `Vec<(SpaceId, Position)>` for cache-friendly merge-join
2. **Buffer reuse**: Scratch buffers swapped instead of reallocated
3. **Hash-based change detection**: Skip recomputation when graph hash unchanged
4. **Transitive cache**: Memoize BFS results, invalidate selectively on events
5. **Single-pass subtree attachment**: Topic subtrees collected then attached in one DFS pass (see design decision below)
6. **Iterative tree traversal**: All tree functions use explicit stacks instead of recursion to avoid stack overflow on deep graphs

## Design Decisions

### Why MOVED Instead of REMOVE+ADD?

**Decision**: Emit explicit `MOVED` changes instead of `REMOVED` followed by `ADDED`.

**Rationale**: 
- Downstream consumers can distinguish "space left canonical set" from "space changed position"
- Enables optimized handling (e.g., update parent pointer vs. full re-index)
- Preserves semantic intent of the change

### Why Sorted Vec Over HashMap?

**Decision**: Store positions as `Vec<(SpaceId, Position)>` sorted by SpaceId.

**Rationale**:
- The diff algorithm requires sorted iteration (merge-join)
- HashMap would be cloned then sorted on every diff
- Vec has better cache locality for the linear merge scan
- ~40% less memory overhead than HashMap

### Why Full BFS Recomputation?

**Decision**: Recompute entire canonical graph on each affecting event.

**Rationale**:
- Correctness first: Incremental updates are complex and error-prone
- BFS is fast enough (12M+ nodes/sec)
- Change detection via hash avoids unnecessary downstream work
- Future optimization possible if needed

### Why Topic Edges Don't Grant Canonical?

**Decision**: Topic edges connect canonical nodes but never expand the canonical set.

**Rationale**:
- Topics are user-declared, not trust relationships
- Prevents spam: Anyone can create a space with any topic
- Explicit edges (Verified/Related/Editor/Member) represent deliberate trust decisions

### Why Editor/Member Edges Are Followed Transitively?

**Decision**: Personal spaces reached via Editor/Member can have their own edges followed.

**Rationale**:
- Editor/Member spaces are full spaces, not special leaf nodes
- A personal space might verify other spaces
- Consistent with "reachable via explicit edges" rule

### Why Single-Pass Subtree Attachment?

**Decision**: Collect all topic subtrees into a `HashMap<SpaceId, Vec<TreeNode>>`, then attach them in a single DFS pass over the tree.

**Context**: Phase 2 of canonical computation attaches filtered subtrees at source nodes in the tree for each topic edge. The original implementation called `attach_subtree()` per topic member, which performed a full DFS to find the source node each time — O(T x N) total where T = topic attachments and N = tree size.

**Rationale**:
- Profiling showed Phase 2 consumed ~94% of total compute time on topic-heavy graphs
- Single-pass reduces O(T x N) to O(N + T): one DFS over the tree with O(1) HashMap lookups
- HashMap was chosen over sorted Vec for the pending attachments because the lookup is per-node during a single DFS (not a batch of binary searches), and the number of distinct source nodes is typically small

**Benchmarks (Apple Silicon, 1000 canonical nodes + 500 non-canonical + topic edges)**:

| Benchmark | Before | After | Speedup |
|-----------|--------|-------|---------|
| `canonical_with_topics/large` | 15.8 ms | 3.9 ms | 4.0x |
| `end_to_end/full_pipeline` | 17.0 ms | 3.7 ms | 4.6x |
| `end_to_end/warm_cache` | 14.7 ms | 0.8 ms | 18.4x |

No regression on explicit-only graphs (no topic edges).

### Why Iterative Tree Traversal?

**Decision**: All tree traversal functions use iterative DFS with explicit `Vec` stacks instead of recursion.

**Rationale**:
- Rust's default thread stack is 8 MB; each recursive frame is ~100-200 bytes
- A linear chain of ~80K nodes (possible with blockchain-derived graph data) overflows the stack
- Iterative traversal moves the "stack" to the heap where it can grow to available memory
- Benchmarks show equal or better performance vs. recursive (8-13% faster for tree construction due to better cache behavior with an index-based stack)
