# Transitive Graph Implementation

This document describes the implementation of transitive graph computation in Atlas.

## Overview

The transitive graph processor computes the transitive closure of the topology graph using BFS. It supports two variants:

- **Full transitive**: follows both explicit edges (Verified, Related) and topic edges
- **Explicit-only transitive**: follows only explicit edges

## Module Structure

```
atlas/src/graph/
├── mod.rs          # Module exports
├── state.rs        # GraphState - topology storage
├── tree.rs         # TreeNode - tree representation
├── hash.rs         # TreeHasher - change detection
└── transitive.rs   # TransitiveProcessor - BFS computation
```

## Core Data Structures

### GraphState

In-memory representation of the topology graph:

```rust
pub struct GraphState {
    spaces: HashSet<SpaceId>,
    space_topics: HashMap<SpaceId, TopicId>,        // space -> announced topic
    topic_spaces: HashMap<TopicId, HashSet<SpaceId>>, // topic -> announcing spaces
    explicit_edges: HashMap<SpaceId, Vec<(SpaceId, EdgeType)>>,
    topic_edges: HashMap<SpaceId, HashSet<TopicId>>,
    topic_edge_sources: HashMap<TopicId, HashSet<SpaceId>>, // reverse index
}
```

The `topic_edge_sources` reverse index enables O(1) lookup of which spaces have edges TO a given topic, used for cache invalidation.

### TreeNode

Tree representation of a BFS traversal:

```rust
pub struct TreeNode {
    space_id: SpaceId,
    edge_type: EdgeType,      // Root, Verified, Related, Topic
    topic_id: Option<TopicId>, // Set if reached via topic edge
    children: Vec<TreeNode>,
}
```

### TransitiveGraph

Result of transitive computation:

```rust
pub struct TransitiveGraph {
    root: SpaceId,
    tree: TreeNode,           // Tree structure with edge metadata
    flat: HashSet<SpaceId>,   // Flat set for O(1) membership checks
    hash: u64,                // For change detection
}
```

## Algorithm

### BFS Computation

The `compute()` function performs BFS traversal:

```
1. Initialize visited set and queue with root
2. For each node in queue:
   a. Collect outgoing edges (explicit + topic if enabled)
   b. Sort edges by target SpaceId for determinism
   c. For each unvisited target:
      - Mark visited, add to queue
      - Record metadata (edge_type, topic_id)
      - Add to parent's children list
3. Build tree recursively from children index
4. Return TransitiveGraph with tree, flat set, and hash
```

### Caching Strategy

```rust
pub struct TransitiveCache {
    full: HashMap<SpaceId, TransitiveGraph>,
    explicit_only: HashMap<SpaceId, TransitiveGraph>,
    reverse_deps: HashMap<SpaceId, HashSet<SpaceId>>,
}
```

- **Lazy computation**: graphs computed on first access
- **Reverse dependency tracking**: when space X is in space Y's transitive graph, we record Y in `reverse_deps[X]`
- **Invalidation**: when X changes, invalidate X's graphs AND all graphs in `reverse_deps[X]`

## Performance Optimizations

### 1. O(n) Tree Building

**Problem**: Original implementation was O(n²) - for each node, iterated all parents to find children.

**Solution**: Build children index during BFS:

```rust
// During BFS, build children index directly
children_index: HashMap<SpaceId, Vec<SpaceId>>

// When visiting edge current -> target:
children_index.entry(current).or_default().push(target);

// Tree building is now O(n) - just iterate children
```

### 2. Reverse Topic Edge Index

**Problem**: Finding spaces with topic edges to a given topic was O(n).

**Solution**: Maintain reverse index in GraphState:

```rust
// When adding topic edge source -> topic:
topic_edge_sources.entry(topic).or_default().insert(source);

// Lookup is now O(1):
state.get_topic_edge_sources(&topic_id)
```

### 3. Minimal Allocations

- **Reuse edge buffer**: `edges.clear()` instead of new Vec per iteration
- **Deferred TreeNode creation**: store only `(EdgeType, Option<TopicId>)` during BFS, create TreeNode once at end
- **Reserve capacity**: `node.children.reserve(children.len())` before adding children

### 4. Efficient Sorting

Use `sort_unstable_by_key` instead of `sort_by_key` - faster when stability isn't needed.

## Complexity Analysis

| Operation | Complexity |
|-----------|------------|
| BFS traversal | O(V + E) |
| Tree building | O(V) |
| Cache lookup | O(1) |
| Cache invalidation | O(reverse_deps) |
| Topic edge source lookup | O(1) |

Where V = nodes visited, E = edges traversed.

## Usage Example

```rust
let mut state = GraphState::new();
let mut processor = TransitiveProcessor::new();

// Apply events to state
for event in events {
    processor.handle_event(&event, &state); // Invalidate cache
    state.apply_event(&event);              // Update state
}

// Get transitive graphs (computed lazily, cached)
let full = processor.get_full(root_space, &state);
let explicit = processor.get_explicit_only(root_space, &state);

println!("Reachable nodes: {}", full.len());
println!("Tree hash: {:016x}", full.hash);
```

## Tree Hashing

The `hash` module provides a separate interface for tree hashing:

```rust
pub trait TreeHasher {
    fn hash_tree(&self, tree: &TreeNode) -> u64;
}

// Convenience function
pub fn hash_tree(tree: &TreeNode) -> u64
```

Hashing traverses the tree recursively, incorporating:
- `space_id`
- `edge_type`
- `topic_id`
- `children.len()`
- Recursive child hashes

This produces a deterministic hash for change detection.

## Related Documents

- [Algorithm Overview](./algorithm-overview.md)
- [0002: Transitive Graph Implementation Plan](./agents/plans/0002-transitive-graph-implementation-plan.md)
- [Graph Concepts](./graph-concepts.md)
