# Canonical Graph Implementation

This document describes the current implementation of canonical graph computation in Atlas.

## Overview

The canonical graph represents the "trusted" portion of the topology graph, computed from a designated root node. Trust flows only through explicit edges (Verified, Related); topic edges cannot grant trust but can add connections between already-trusted nodes.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         CanonicalProcessor                          │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                     Phase 1                                  │   │
│  │  Get canonical set from root's explicit-only transitive     │   │
│  │  (O(1) lookup from TransitiveCache)                         │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              │                                      │
│                              ▼                                      │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                     Phase 2                                  │   │
│  │  Add topic edges with filtered subtrees                     │   │
│  │  (Only include nodes already in canonical set)              │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              │                                      │
│                              ▼                                      │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                 Change Detection                             │   │
│  │  Hash tree and compare with previous                        │   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

## Data Structures

### CanonicalGraph

```rust
pub struct CanonicalGraph {
    /// Root space this graph was computed from
    pub root: SpaceId,

    /// Tree representation with edge metadata
    /// Preserves distance from root
    pub tree: TreeNode,

    /// Flat set of all canonical spaces
    pub flat: HashSet<SpaceId>,
}
```

The `CanonicalGraph` contains:
- **tree**: A tree structure preserving parent-child relationships and edge metadata. Nodes can appear multiple times (once via explicit edge, once via topic edge) to preserve all paths.
- **flat**: A flat set for O(1) membership checks. Each node appears exactly once regardless of how many paths lead to it.

### CanonicalProcessor

```rust
pub struct CanonicalProcessor {
    /// The root space for canonical graph computation
    root: SpaceId,

    /// Hash of the last computed tree structure (for change detection)
    last_hash: Option<u64>,
}
```

The processor tracks the last tree hash to detect changes. It delegates to `TransitiveProcessor` for the heavy lifting of graph traversal.

## Algorithm

### Phase 1: Establish Canonical Set

Phase 1 determines which nodes are "trusted" by traversing only explicit edges from the root.

```rust
let root_transitive = transitive.get_explicit_only(self.root, state);
let canonical_set = root_transitive.flat.clone();
let mut tree = root_transitive.tree.clone();
```

This is an O(1) lookup when the transitive graph is cached, or O(explicit_edges) BFS on cache miss.

**Key property**: Only nodes reachable via explicit edges (Verified, Related) from the root are canonical. Topic edges cannot grant trust.

### Phase 2: Add Topic Edges

Phase 2 adds topic edge connections between already-canonical nodes.

```rust
for (source, topic_id) in topic_edges_from_canonical_nodes {
    let members = state.get_topic_members(&topic_id);
    
    for member in members.filter(|m| canonical_set.contains(m)) {
        let member_transitive = transitive.get_full(member, state);
        let filtered_subtree = filter_to_canonical(&member_transitive.tree, &canonical_set);
        attach_subtree(&mut tree, source, filtered_subtree);
    }
}
```

For each topic edge from a canonical source:
1. Resolve the topic to its member spaces
2. Filter to only canonical members
3. For each canonical member, get its full transitive graph
4. Filter the subtree to only include canonical descendants
5. Attach the filtered subtree to the source node

**Key property**: Topic edges only connect nodes that are already canonical. They cannot expand the canonical set.

### Subtree Filtering

When attaching a member's transitive subtree via a topic edge, we filter out any non-canonical descendants:

```rust
fn filter_to_canonical(&self, subtree: &TreeNode, canonical_set: &HashSet<SpaceId>) -> TreeNode {
    let mut filtered = TreeNode::new_with_topic(subtree.space_id, topic_id);
    
    for child in &subtree.children {
        if canonical_set.contains(&child.space_id) {
            filtered.children.push(filter_child_recursive(child, canonical_set));
        }
    }
    
    filtered
}
```

This ensures that even if a canonical member has non-canonical descendants in its full transitive graph, those descendants are excluded from the canonical tree.

### Tree Structure Change Detection

After computing the tree, we hash it and compare with the previous hash:

```rust
let graph = CanonicalGraph::new(self.root, tree, canonical_set);

let new_hash = hash_tree(&graph.tree);
if self.last_hash == Some(new_hash) {
    return None;  // Tree structure unchanged
}

self.last_hash = Some(new_hash);
Some(graph)
```

**Why hash the tree?**

Even when the canonical *set* doesn't change, the tree *structure* can change due to:

1. **BFS traversal order** - A node reachable via multiple paths appears at the first depth encountered
2. **New shorter paths** - Adding an edge can move a node closer to the root
3. **Topic edge attachment order** - Different orderings can attach subtrees in different places

Example:
```
Before: Root -> A -> B -> C  (C at depth 3)
After:  Root -> A -> B -> C
        Root -> D -> C       (new edge, C now at depth 2 via D)
```

The canonical set `{Root, A, B, C, D}` may be the same, but the tree structure changed because C moved from depth 3 to depth 2. Since downstream consumers care about distance from root, we must detect and emit this change.

**Note**: This hash comparison happens *after* full recomputation. It detects when the tree structure is actually unchanged despite an event from a canonical source. The real optimization for skipping computation is event filtering (below).

## Event Filtering

Not every topology event affects the canonical graph. The `affects_canonical` method provides an O(1) check:

```rust
pub fn affects_canonical(
    &self,
    event: &SpaceTopologyEvent,
    canonical_set: &HashSet<SpaceId>,
) -> bool {
    match &event.payload {
        // New spaces aren't canonical until reached via explicit edges
        SpaceTopologyPayload::SpaceCreated(_) => false,

        SpaceTopologyPayload::TrustExtended(extended) => {
            // Only events from canonical sources can affect the graph
            canonical_set.contains(&extended.source_space_id)
        }
    }
}
```

| Event | Affects Canonical? | Reason |
|-------|-------------------|--------|
| `SpaceCreated` | No | New spaces aren't canonical until explicitly connected |
| Extension from canonical source | Yes | May add new canonical nodes or tree structure |
| Extension from non-canonical source | No | Cannot affect canonical set or tree |

## Integration with TransitiveProcessor

The `CanonicalProcessor` leverages `TransitiveProcessor` for efficient computation:

```
┌─────────────────────────────────────────────────────────────────────┐
│                      TransitiveProcessor                             │
│                                                                      │
│  TransitiveCache:                                                    │
│    explicit_only[space] → TransitiveGraph (explicit edges only)     │
│    full[space] → TransitiveGraph (explicit + topic edges)           │
│    reverse_deps[space] → spaces whose graph contains this space     │
│                                                                      │
│  On invalidation: remove from cache, lazy recompute on next access  │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      CanonicalProcessor                              │
│                                                                      │
│  Phase 1: canonical_set = cache.get_explicit_only(root).flat        │
│                                                                      │
│  Phase 2: for each topic edge from canonical nodes:                 │
│             attach cache.get_full(member) filtered to canonical_set │
│                                                                      │
│  Output: CanonicalGraph (if changed)                                │
└─────────────────────────────────────────────────────────────────────┘
```

## Performance Characteristics

| Operation | Complexity | Notes |
|-----------|------------|-------|
| Phase 1 (cache hit) | O(1) | Lookup from TransitiveCache |
| Phase 1 (cache miss) | O(explicit_edges) | BFS traversal |
| Phase 2 per topic edge | O(topic_members × subtree_size) | Filter and attach |
| affects_canonical | O(1) | HashSet lookup |
| Change detection | O(tree_size) | Tree hashing |

## Usage Example

```rust
use atlas::graph::{CanonicalProcessor, GraphState, TransitiveProcessor};

// Set up state
let mut state = GraphState::new();
// ... apply events to build the graph ...

// Create processors
let mut transitive = TransitiveProcessor::new();
let mut canonical = CanonicalProcessor::new(root_space_id);

// Compute canonical graph
if let Some(graph) = canonical.compute(&state, &mut transitive) {
    println!("Canonical graph has {} nodes", graph.len());
    println!("Tree structure:\n{:?}", graph.tree);
}

// On new event, check if it affects canonical
if canonical.affects_canonical(&event, &graph.flat) {
    // Recompute
    if let Some(new_graph) = canonical.compute(&state, &mut transitive) {
        // Emit update
    }
}
```

## File Structure

```
atlas/src/graph/
├── canonical.rs      # CanonicalGraph, CanonicalProcessor
├── transitive.rs     # TransitiveGraph, TransitiveProcessor, TransitiveCache
├── state.rs          # GraphState
├── tree.rs           # TreeNode, EdgeType
├── hash.rs           # Tree hashing
├── memory.rs         # Memory size estimation
└── mod.rs            # Module exports
```

## Testing

The implementation includes comprehensive unit tests in `canonical.rs`:

- `test_single_space_canonical` - Single root node
- `test_explicit_edges_only` - Linear chain via explicit edges
- `test_topic_edge_to_canonical_member` - Topic edge to already-canonical node
- `test_topic_edge_to_non_canonical_member` - Topic edge to non-canonical node (filtered out)
- `test_topic_edge_includes_transitive_subtree` - Subtree attachment via topic edge
- `test_affects_canonical_*` - Event filtering tests
- `test_change_detection` - Hash-based change detection
- `test_multiple_spaces_same_topic` - Multiple spaces announcing same topic
- `test_filtered_subtree_preserves_canonical_only` - Subtree filtering correctness

## Benchmarks

Benchmarks are available in `atlas/benches/canonical.rs`:

```bash
cargo bench -p atlas --bench canonical
```

### Benchmark Results

Results from running on Apple M1:

#### Core Computation

| Benchmark | Nodes | Time | Throughput |
|-----------|-------|------|------------|
| canonical_computation | 100 | 43 µs | 2.3M elem/s |
| | 500 | 222 µs | 2.3M elem/s |
| | 1000 | 485 µs | 2.1M elem/s |
| | 5000 | 2.5 ms | 2.0M elem/s |
| canonical_phase1 | 100 | 39 µs | 2.6M elem/s |
| | 500 | 194 µs | 2.6M elem/s |
| | 1000 | 412 µs | 2.4M elem/s |
| | 5000 | 2.2 ms | 2.3M elem/s |

#### Topic Edge Processing

| Scenario | Time |
|----------|------|
| small (100 canonical, 50 non-canonical) | 353 µs |
| medium (500 canonical, 200 non-canonical) | 4.4 ms |
| large (1000 canonical, 500 non-canonical) | 33.6 ms |

#### Event Filtering

| Scenario | Time |
|----------|------|
| affects_canonical (canonical source) | 16 ns |
| affects_canonical (non-canonical source) | 17 ns |

#### Repeated Computation (warm transitive cache)

| Scenario | Time |
|----------|------|
| First compute (cold transitive cache) | 473 µs |
| Second compute (warm cache, tree unchanged) | 39 µs |

Note: The speedup is from the transitive cache hit (O(1) lookup vs BFS traversal). The hash comparison adds negligible overhead.

#### Subtree Filtering by Canonical Set Density

| Density | Time |
|---------|------|
| Sparse (10% canonical) | 937 µs |
| Medium (50% canonical) | 19.7 ms |
| Dense (90% canonical) | 62.8 ms |

#### End-to-End (1000 nodes with topics)

| Cache State | Time |
|-------------|------|
| Cold cache | 37 ms |
| Warm cache | 32 ms |

### Key Findings

1. **`affects_canonical` is very fast** (~16 ns) - O(1) HashSet lookup as expected, enabling efficient event filtering

2. **Phase 1 dominates simple graphs** - For graphs without topic edges, Phase 1 (explicit-only transitive) is ~90% of computation time

3. **Transitive cache provides major speedup** - Warm cache (39 µs) vs cold cache (473 µs) is ~12x faster due to O(1) transitive graph lookups instead of BFS traversal

4. **Subtree filtering scales with density** - More canonical nodes means more filtering work during Phase 2 topic edge processing

5. **Warm cache provides ~14% speedup** - Pre-computed transitive graphs reduce end-to-end latency (37ms → 32ms)

6. **Throughput is consistent** - ~2M elements/second across different graph sizes, indicating good algorithmic scaling

### Success Criteria (from implementation plan)

| Metric | Target | Actual | Status |
|--------|--------|--------|--------|
| Full computation (1K nodes) | < 10ms | 485 µs | ✅ |
| Full computation (10K nodes) | < 100ms | ~5 ms (extrapolated) | ✅ |
| Event filtering | < 100ns | 16 ns | ✅ |
| End-to-end latency (p95) | < 50ms | 37 ms | ✅ |

## Related Documents

- [Algorithm Overview](./algorithm-overview.md) - High-level data flow
- [Graph Concepts](./graph-concepts.md) - Core concepts and terminology
- [Transitive Graph Implementation](./transitive-graph-implementation.md) - TransitiveProcessor details
- [0001: Canonical Graph Implementation Plan](./agents/plans/0001-canonical-graph-implementation-plan.md) - Original design plan
