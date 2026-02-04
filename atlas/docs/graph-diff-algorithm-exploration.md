# Graph Diff Algorithm Exploration

## Problem Statement

Atlas maintains a canonical tree representing trusted spaces reachable from a root. When events occur (edges added/removed, spaces created), we need to compute a **diff** between the old and new tree states to emit to Kafka.

## Tree Structure

```rust
struct TreeNode {
    space_id: SpaceId,        // [u8; 16]
    edge_type: EdgeType,      // Root, Verified, Related, Topic, Editor, Member
    topic_id: Option<TopicId>, // [u8; 16], present only for Topic edges
    children: Vec<TreeNode>,
}
```

Each node has:
- **space_id**: Unique identifier
- **distance**: Depth from root (implicit from tree position)
- **parent**: The node that reaches this one (implicit from tree structure)
- **edge_type**: How this node was reached from parent
- **topic_id**: For Topic edges only

## Diff Output Format

```rust
struct GraphDiff {
    changes: Vec<NodeChange>,
}

struct NodeChange {
    space_id: SpaceId,
    change_type: ChangeType,  // Added, Removed
    distance: Option<u32>,    // For Added
    parent_edge: Option<ParentEdge>, // For Added
}

struct ParentEdge {
    source: SpaceId,
    edge_type: EdgeType,
    topic_id: Option<TopicId>,
}
```

## What Constitutes a Change

A node appears in the diff if:
1. **Added**: In new tree but not old tree
2. **Removed**: In old tree but not new tree
3. **Moved**: In both trees but position changed (different parent, distance, or edge_type)

**Moves are encoded as REMOVED + ADDED** (no explicit MOVED type).

## Scale

- Trees can have **100k+ nodes**
- Events are frequent (blockchain blocks)
- Most events cause **small changes** (one edge add/remove affects a subtree)
- Need efficient diff computation

## Key Insight

For most events, **99%+ of nodes don't change**. A single edge modification typically affects:
- O(1) nodes for leaf changes
- O(subtree depth) for mid-tree changes
- O(n) worst case for root-adjacent changes (rare)

## Approaches to Explore

### Approach 1: Full Tree Comparison (Naive)

Compare every node in old tree vs new tree using HashMap.

```
old_positions: HashMap<SpaceId, NodePosition>
new_positions: HashMap<SpaceId, NodePosition>

for each node in old: check if exists/changed in new
for each node in new: check if newly added
```

**Complexity**: O(n) hash operations with random memory access
**Problem**: Poor cache locality, ~50-100ms for 100k nodes

### Approach 2: Sorted Vector Merge

Flatten both trees to sorted vectors, then merge-join.

```
old_vec: Vec<(SpaceId, NodePosition)> sorted by SpaceId
new_vec: Vec<(SpaceId, NodePosition)> sorted by SpaceId

merge-join to find differences
```

**Complexity**: O(n log n) sort + O(n) sequential merge
**Benefit**: Cache-friendly sequential access, ~5-10ms for 100k nodes

### Approach 3: RoaringBitmap + Position Hashes

Use RoaringBitmap for fast set operations, position hashes for change detection.

```
membership: RoaringBitmap (SpaceId -> u32 index)
position_hashes: Vec<u64>  // hash of (distance, parent, edge_type)

removed = old.membership - new.membership  // O(n/64) SIMD
added = new.membership - old.membership    // O(n/64) SIMD
maybe_moved = old.membership & new.membership

for idx in maybe_moved:
    if old.position_hashes[idx] != new.position_hashes[idx]:
        emit change
```

**Complexity**: O(n/64) set ops + O(intersection) hash comparison
**Benefit**: Very fast for pure adds/removes
**Problem**: Most nodes are in `maybe_moved`, still O(n) position checks

### Approach 4: Event-Driven Diff

Instead of comparing full trees, **know what the event affects**:

| Event | Affected Nodes |
|-------|---------------|
| SpaceCreated | Just the new node (if it becomes canonical) |
| Edge A→B added | B and B's descendants (might move closer to root) |
| Edge A→B removed | B and B's descendants (might be removed or reparented) |

```
fn diff_for_edge_added(source, target, old_tree, new_tree):
    old_target_pos = find_in_tree(old_tree, target)
    new_target_pos = find_in_tree(new_tree, target)

    if target newly canonical:
        return collect_subtree_as_added(new_tree, target)
    if target moved closer:
        return diff_subtree(old_tree, new_tree, target)
    return empty
```

**Complexity**: O(affected subtree) instead of O(n)
**Benefit**: Microseconds for typical events
**Challenge**: Need to correctly identify affected nodes for each event type

### Approach 5: Incremental Tree with Change Tracking

Modify the tree in-place and track changes as they happen.

```
struct TrackedTree {
    root: TreeNode,
    pending_changes: Vec<NodeChange>,
}

fn add_edge(&mut self, source, target, edge_type):
    // Modify tree
    // Record any nodes that moved/added/removed
    self.pending_changes.push(...)

fn take_diff(&mut self) -> GraphDiff:
    return GraphDiff { changes: self.pending_changes.drain(..).collect() }
```

**Complexity**: O(affected) amortized
**Benefit**: No diffing needed - changes recorded as they happen
**Challenge**: Complex to implement correctly, tree mutation logic

### Approach 6: Persistent Data Structure

Use a persistent (immutable) tree structure that shares unchanged subtrees.

```
Old tree and new tree share nodes that didn't change.
Only traverse/compare nodes that are different objects.
```

**Complexity**: O(changed nodes)
**Benefit**: Natural change detection via reference equality
**Challenge**: Requires different tree representation, memory overhead

## Constraints

1. **Correctness first**: Diff must be accurate
2. **Typical case matters**: Optimize for small changes to large trees
3. **Memory**: Can't keep unlimited history
4. **Simplicity**: Maintainable code preferred over micro-optimizations

## Questions to Explore

1. What's the best data structure for the tree to enable efficient diffing?
2. Should we diff the tree or track changes as we build the new tree?
3. How do we efficiently find a node's position in a tree (for event-driven)?
4. Is there a way to share structure between old/new trees?
5. What's the right trade-off between implementation complexity and performance?

## Current Implementation Context

- Trees are computed fresh on each event via BFS traversal
- Old tree is discarded after each event
- No incremental updates currently
- See `atlas/src/graph/canonical.rs` and `atlas/src/graph/transitive.rs`

## Success Criteria

- Diff computation for 100k node tree with small change: < 10ms
- Memory overhead: < 2x tree size
- Code complexity: Reasonable, maintainable

## Benchmark Summary (Mocked 100k nodes)

We benchmarked three diff strategies on mocked 100k-node topologies with ~1% add/remove/move by default:

- **Sorted vector merge (Approach #2)**: ~0.5–0.7ms diff-only; ~70–90ms including data construction.
- **BTreeMap merge**: ~0.75–1.0ms diff-only; similar build+diff costs.
- **RoaringTreemap + UUID→u64 mapping**: ~0.8–0.9ms diff-only; build+diff competitive once mapping is cached, but requires stable UUID→u64 mapping across events.

Change-rate sweeps (0.1% → 50% churn) show diff-only times scaling roughly linearly, staying <~1.3ms for sorted-merge at 50% churn. Build+diff costs are dominated by constructing positions and data structures.

**Decision:** use **sorted vector merge** for initial implementation due to simplicity, deterministic behavior, and competitive performance. Revisit if real-world profiling shows diff as a bottleneck or if we adopt a persistent ID mapping.
