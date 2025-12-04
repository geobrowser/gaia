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

## Benchmarks

Benchmarks are located in `atlas/benches/transitive.rs`. Run with:

```bash
cargo bench -p atlas
```

### Benchmark Scenarios

| Benchmark | Description |
|-----------|-------------|
| `bfs_linear_chain` | Linear chain graph (0 → 1 → 2 → ... → n) |
| `bfs_wide_graph` | Wide/shallow graph (root → [1, 2, ..., n]) |
| `bfs_binary_tree` | Balanced binary tree |
| `bfs_random_graph` | Random graph with configurable density |
| `full_vs_explicit_only` | Comparison of traversal variants |
| `cache` | Cache hit vs miss performance |
| `tree_hashing` | Hash computation by tree size |
| `graph_state_events` | Event application overhead |
| `cache_invalidation` | Invalidation cost |
| `memory_sizes` | Memory usage measurements |

### Results

Measured on Apple M1 Pro:

#### BFS Computation

| Graph Type | Nodes | Time | Throughput |
|------------|-------|------|------------|
| Linear chain | 100 | 42 µs | 2.4 Melem/s |
| Linear chain | 1,000 | 489 µs | 2.0 Melem/s |
| Linear chain | 5,000 | 2.4 ms | 2.1 Melem/s |
| Binary tree | 127 | 52 µs | 2.4 Melem/s |
| Binary tree | 2,047 | 870 µs | 2.4 Melem/s |
| Binary tree | 8,191 | 3.5 ms | 2.3 Melem/s |
| Random (5000n/20000e) | 5,000 | 2.8 ms | 1.8 Melem/s |

#### Cache Performance

| Operation | Time |
|-----------|------|
| Cache miss (1000 nodes) | ~590 µs |
| Cache hit | ~39 ns |
| **Speedup** | **~15,000x** |

#### Tree Hashing

| Nodes | Time |
|-------|------|
| 100 | 2.3 µs |
| 1,000 | 23 µs |
| 5,000 | 115 µs |

#### Memory Usage

Memory calculations are provided by the `atlas::graph::memory` module.

##### GraphState Memory

| Nodes | Edges | Total | Per Node | Per Edge |
|-------|-------|-------|----------|----------|
| 100 | 195 | 39 KB | 401 B | 205 B |
| 1,000 | 4,000 | 564 KB | 577 B | 144 B |
| 5,000 | 20,000 | 2.4 MB | 500 B | 125 B |
| 10,000 | 40,000 | 4.8 MB | 500 B | 125 B |
| 50,000 | 200,000 | 21 MB | 438 B | 109 B |

##### TransitiveGraph Memory (single graph)

| Nodes | Total | Tree | FlatSet |
|-------|-------|------|---------|
| 100 | 28 KB | 25 KB | 3.5 KB |
| 1,000 | 306 KB | 250 KB | 56 KB |
| 5,000 | 1.4 MB | 1.2 MB | 224 KB |
| 10,000 | 2.9 MB | 2.4 MB | 448 KB |

##### Cache Memory (multiple cached graphs)

| Graphs | Nodes/Graph | Cache Total | Per Graph |
|--------|-------------|-------------|-----------|
| 10 | 100 | 1.6 MB | 163 KB |
| 100 | 100 | 155 MB | 1.6 MB |
| 10 | 1,000 | 14 MB | 1.4 MB |
| 100 | 1,000 | 1.6 GB | 16 MB |

**Note**: Cache memory grows significantly with the number of cached graphs due to the reverse dependency index tracking which spaces appear in which transitive graphs. See [Future Optimizations](#future-optimizations) for potential mitigation strategies.

### Key Observations

1. **Throughput is consistent** (~2M elements/second) across graph shapes, indicating O(V+E) scaling
2. **Cache hits are extremely fast** (~39ns) - just a HashMap lookup
3. **Tree hashing scales linearly** with node count
4. **The optimizations matter**: cache provides 15,000x speedup over recomputation
5. **Memory scales linearly**: ~500 bytes per node for GraphState, ~300 bytes per node for TransitiveGraph

### Performance Targets

From the implementation plan:

| Metric | Target | Actual |
|--------|--------|--------|
| Single transitive (1K nodes) | < 5ms | ~590 µs ✓ |
| Single transitive (10K nodes) | < 50ms | ~6 ms ✓ |
| Cache lookup | < 1ms | ~39 ns ✓ |

All targets are met with significant margin.

## Future Optimizations

The current cache implementation is simple but can consume significant memory when caching many graphs. The following optimizations could be considered if memory becomes a bottleneck.

### Cache Eviction Strategies

**Current limitation**: The `reverse_deps` index tracks which cached graphs contain each space, enabling efficient invalidation. This index grows with the number of cached graphs and creates a tradeoff between memory usage and invalidation efficiency.

**Option 1: LRU Eviction with On-Demand Recomputation**

Evict least-recently-used graphs when cache exceeds a size limit. Evicted graphs are recomputed on next access.

```rust
struct BoundedTransitiveCache {
    full: LruCache<SpaceId, TransitiveGraph>,
    explicit_only: LruCache<SpaceId, TransitiveGraph>,
    reverse_deps: HashMap<SpaceId, HashSet<SpaceId>>,
    max_entries: usize,
}
```

Tradeoffs:
- Pro: Bounded memory usage
- Con: Cache misses require full recomputation (~600µs for 1K nodes)
- Con: `reverse_deps` cleanup on eviction adds complexity

**Option 2: Canonical-Only Cache**

For canonical graph computation, only cache what's strictly needed:
- 1 explicit-only graph (the root's)
- N full graphs (canonical spaces that are targets of topic edges)

```rust
struct CanonicalCache {
    root_explicit_only: Option<TransitiveGraph>,
    canonical_member_graphs: HashMap<SpaceId, TransitiveGraph>,
}
```

Tradeoffs:
- Pro: Bounded by canonical set size, not total space count
- Pro: No wasted memory on graphs that won't be used
- Con: Requires knowledge of canonical set membership

**Option 3: Invalidation Without Reverse Deps**

Remove `reverse_deps` entirely. On any graph change, invalidate all cached graphs.

Tradeoffs:
- Pro: Dramatically reduces memory (no reverse index)
- Pro: Simpler implementation
- Con: Over-invalidates, causing unnecessary recomputation

**Option 4: Generation-Based Invalidation**

Track a global generation counter. Each cached graph stores its generation. On any change, bump the generation. Cache hits check generation match.

```rust
struct GenerationalCache {
    generation: u64,
    full: HashMap<SpaceId, (u64, TransitiveGraph)>,  // (gen, graph)
    explicit_only: HashMap<SpaceId, (u64, TransitiveGraph)>,
}
```

Tradeoffs:
- Pro: No reverse_deps memory overhead
- Pro: Simple invalidation (just increment counter)
- Con: Stale entries consume memory until accessed and evicted

### Recommendation

Start with the current implementation. If memory becomes a problem:

1. **First**: Profile actual usage patterns to understand which graphs are accessed
2. **If canonical-only**: Implement Option 2 (Canonical-Only Cache)
3. **If general purpose**: Implement Option 1 (LRU) or Option 4 (Generation-Based)

The ~15,000x speedup from caching means even occasional cache misses are acceptable, so aggressive caching may not be necessary.

## Related Documents

- [Algorithm Overview](./algorithm-overview.md)
- [0002: Transitive Graph Implementation Plan](./agents/plans/0002-transitive-graph-implementation-plan.md)
- [Graph Concepts](./graph-concepts.md)
