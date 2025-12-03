# 0002: Transitive Graph Implementation Plan

## Summary

This document outlines the plan to implement transitive graph computation for Atlas. Transitive graphs are computed per-space and represent all nodes reachable from that space. They serve as a foundation for efficient canonical graph computation.

## Relationship to Canonical Graph

The `TransitiveProcessor` is designed to support the `CanonicalProcessor`:

1. **Phase 1 optimization**: Root's explicit-only transitive graph provides the canonical set directly
2. **Phase 2 optimization**: Pre-computed transitive graphs are cloned when adding members via topic edges

This means `TransitiveProcessor` should be implemented first or in parallel with `CanonicalProcessor`.

## Understanding

### Transitive Graph Definition

For a given space S, its transitive graph contains:
- S itself (the root)
- All spaces reachable by following edges from S
- Represented as both a tree structure and a flat set

### Two Variants

We need two types of transitive graphs:

1. **Full Transitive Graph**: Follows both explicit edges and topic edges
   - Used for general reachability queries
   - Used in Phase 2 of canonical computation (attaching subtrees)

2. **Explicit-Only Transitive Graph**: Follows only explicit edges
   - Used in Phase 1 of canonical computation (establishing canonical set)
   - Represents "trust reachability"

### Computation Rules

**Full Transitive (explicit + topic edges):**
```
BFS from space S:
  - Follow explicit edges (Verified, Related)
  - Follow topic edges: resolve topic → find all spaces that announced it → follow
  - First visit wins (cycle breaking)
  - Result: tree + flat set
```

**Explicit-Only Transitive:**
```
BFS from space S:
  - Follow only explicit edges (Verified, Related)
  - Ignore topic edges
  - First visit wins (cycle breaking)
  - Result: tree + flat set
```

## Implementation Plan

### 1. Data Structures

```rust
/// Result of transitive graph computation
#[derive(Clone)]
pub struct TransitiveGraph {
    /// Root space this graph was computed from
    pub root: SpaceId,

    /// Tree representation with edge metadata
    pub tree: TreeNode,

    /// Flat set of all reachable spaces
    pub flat: HashSet<SpaceId>,

    /// Hash for change detection
    pub hash: u64,
}

/// Cache of pre-computed transitive graphs
pub struct TransitiveCache {
    /// Full transitive graphs (explicit + topic edges)
    pub full: HashMap<SpaceId, TransitiveGraph>,

    /// Explicit-only transitive graphs
    pub explicit_only: HashMap<SpaceId, TransitiveGraph>,

    /// Reverse index: space → spaces whose transitive graph contains it
    /// Used for invalidation
    pub reverse_deps: HashMap<SpaceId, HashSet<SpaceId>>,
}
```

### 2. TransitiveProcessor

```rust
pub struct TransitiveProcessor {
    cache: TransitiveCache,
}

impl TransitiveProcessor {
    /// Compute or retrieve full transitive graph for a space
    pub fn get_full(&mut self, space: SpaceId, state: &GraphState) -> &TransitiveGraph {
        if !self.cache.full.contains_key(&space) {
            let graph = self.compute_full(space, state);
            self.update_reverse_deps(&graph);
            self.cache.full.insert(space, graph);
        }
        self.cache.full.get(&space).unwrap()
    }

    /// Compute or retrieve explicit-only transitive graph for a space
    pub fn get_explicit_only(&mut self, space: SpaceId, state: &GraphState) -> &TransitiveGraph {
        if !self.cache.explicit_only.contains_key(&space) {
            let graph = self.compute_explicit_only(space, state);
            self.update_reverse_deps(&graph);
            self.cache.explicit_only.insert(space, graph);
        }
        self.cache.explicit_only.get(&space).unwrap()
    }

    /// Invalidate caches when graph changes
    pub fn handle_event(&mut self, event: &TopologyEvent) {
        match event {
            // Explicit edge added/removed
            TopologyEvent::ExplicitEdge { source, target, .. } => {
                self.invalidate_affected(source);
                self.invalidate_affected(target);
            }

            // Topic edge added/removed
            TopologyEvent::TopicEdge { source, topic_id, .. } => {
                self.invalidate_affected(source);
                // All spaces that announced this topic may be affected
                // (handled via reverse_deps when those graphs are accessed)
            }

            // Space created with topic
            TopologyEvent::SpaceCreated { space_id, topic_id } => {
                // New space doesn't invalidate existing graphs
                // But spaces with topic edges to this topic may need recomputation
                self.invalidate_topic_edge_sources(topic_id);
            }
        }
    }

    fn invalidate_affected(&mut self, space: &SpaceId) {
        // Invalidate this space's graphs
        self.cache.full.remove(space);
        self.cache.explicit_only.remove(space);

        // Invalidate all spaces that had this space in their transitive graph
        if let Some(dependents) = self.cache.reverse_deps.get(space) {
            for dep in dependents.clone() {
                self.cache.full.remove(&dep);
                self.cache.explicit_only.remove(&dep);
            }
        }
    }
}
```

### 3. Transitive Graph Algorithm

```rust
impl TransitiveProcessor {
    fn compute_full(&self, root: SpaceId, state: &GraphState) -> TransitiveGraph {
        self.compute_transitive(root, state, true)
    }

    fn compute_explicit_only(&self, root: SpaceId, state: &GraphState) -> TransitiveGraph {
        self.compute_transitive(root, state, false)
    }

    fn compute_transitive(
        &self,
        root: SpaceId,
        state: &GraphState,
        include_topic_edges: bool,
    ) -> TransitiveGraph {
        let mut visited: HashSet<SpaceId> = HashSet::new();
        let mut queue: VecDeque<SpaceId> = VecDeque::new();
        let mut tree_nodes: HashMap<SpaceId, TreeNode> = HashMap::new();

        // Initialize with root
        visited.insert(root);
        queue.push_back(root);
        tree_nodes.insert(root, TreeNode::new(root, EdgeType::Unspecified));

        while let Some(current) = queue.pop_front() {
            let mut children: Vec<(SpaceId, EdgeType, Option<TopicId>)> = Vec::new();

            // Collect explicit edges
            if let Some(edges) = state.explicit_edges.get(&current) {
                for (target, edge_type) in edges {
                    children.push((*target, *edge_type, None));
                }
            }

            // Collect topic edges (if enabled)
            if include_topic_edges {
                if let Some(topics) = state.topic_edges.get(&current) {
                    for topic_id in topics {
                        if let Some(members) = state.topic_spaces.get(topic_id) {
                            for member in members {
                                children.push((*member, EdgeType::Topic, Some(*topic_id)));
                            }
                        }
                    }
                }
            }

            // Sort for deterministic ordering
            children.sort_by_key(|(id, _, _)| *id);

            // Process children
            for (child_id, edge_type, topic_id) in children {
                if visited.insert(child_id) {
                    queue.push_back(child_id);

                    let child_node = TreeNode::new_with_edge(child_id, edge_type, topic_id);
                    tree_nodes.insert(child_id, child_node.clone());

                    if let Some(parent) = tree_nodes.get_mut(&current) {
                        parent.children.push(child_node);
                    }
                }
            }
        }

        let tree = tree_nodes.remove(&root).unwrap();
        let hash = hash_tree(&tree);

        TransitiveGraph {
            root,
            tree,
            flat: visited,
            hash,
        }
    }
}
```

### 4. Integration with CanonicalProcessor

```rust
impl CanonicalProcessor {
    pub fn compute(
        &self,
        state: &GraphState,
        transitive: &mut TransitiveProcessor,
    ) -> Option<CanonicalGraph> {
        // Phase 1: Get canonical set from root's explicit-only transitive graph
        let root_transitive = transitive.get_explicit_only(self.root, state);
        let canonical_set = root_transitive.flat.clone();
        let mut tree = root_transitive.tree.clone();

        // Phase 2: Add topic edges
        let deferred_topic_edges = self.collect_topic_edges(&canonical_set, state);

        for (source, topic_id) in deferred_topic_edges {
            if let Some(members) = state.topic_spaces.get(&topic_id) {
                for member in members.iter().sorted() {
                    if canonical_set.contains(member) {
                        // Get pre-computed transitive graph for this member
                        let member_transitive = transitive.get_full(*member, state);

                        // Filter to only canonical nodes and attach
                        let filtered_subtree = self.filter_to_canonical(
                            &member_transitive.tree,
                            &canonical_set,
                        );

                        self.attach_subtree(&mut tree, source, filtered_subtree, topic_id);
                    }
                }
            }
        }

        // Check if changed
        let new_hash = hash_tree(&tree);
        if Some(new_hash) == self.current_hash {
            return None;
        }

        self.current_hash = Some(new_hash);
        Some(CanonicalGraph {
            root: self.root,
            tree,
            flat: canonical_set,
        })
    }

    /// Filter a transitive tree to only include canonical nodes
    fn filter_to_canonical(&self, tree: &TreeNode, canonical_set: &HashSet<SpaceId>) -> TreeNode {
        let mut filtered = TreeNode::new_with_edge(tree.space_id, tree.edge_type, tree.topic_id);

        for child in &tree.children {
            if canonical_set.contains(&child.space_id) {
                filtered.children.push(self.filter_to_canonical(child, canonical_set));
            }
        }

        filtered
    }
}
```

### 5. Persistence

Transitive graphs can be large. Persistence strategy:

**Option A: Compute on demand, no persistence**
- Graphs are recomputed when needed
- Fast for small graphs, expensive for large ones
- No storage overhead

**Option B: Persist all transitive graphs**
- Store in PostgreSQL as JSONB
- Fast reads, but high storage cost
- Need to keep in sync with graph changes

**Option C: Persist only frequently-accessed graphs (LRU)**
- Cache hot graphs in PostgreSQL
- Compute cold graphs on demand
- Balance between storage and compute

**Recommended: Option A initially, Option C if needed**

```sql
-- Optional: persist hot transitive graphs
CREATE TABLE transitive_graph_cache (
    space_id BYTEA PRIMARY KEY,
    graph_type TEXT NOT NULL,  -- 'full' or 'explicit_only'
    tree JSONB NOT NULL,
    flat_space_ids BYTEA[] NOT NULL,
    hash BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_transitive_updated ON transitive_graph_cache(updated_at);
```

## File Structure

```
atlas/
├── src/
│   ├── processor/
│   │   ├── mod.rs
│   │   ├── canonical.rs
│   │   └── transitive.rs      # TransitiveProcessor implementation
│   ├── cache/
│   │   ├── mod.rs
│   │   └── transitive.rs      # TransitiveCache implementation
│   └── ...
├── benches/
│   ├── canonical.rs
│   ├── transitive.rs          # Transitive graph benchmarks
│   └── ...
```

## Benchmarking

Add to existing benchmark suite:

| Scenario | Nodes | Edges | Description |
|----------|-------|-------|-------------|
| Small | 100 | 200 | Single space transitive |
| Medium | 1,000 | 5,000 | Moderate graph |
| Large | 10,000 | 50,000 | Large graph |
| Deep | 1,000 | 999 | Linear chain (worst case depth) |
| Wide | 1,000 | 10,000 | Highly connected (worst case breadth) |

**Metrics:**
- Computation time (full vs explicit-only)
- Memory usage per cached graph
- Invalidation cascade size
- Cache hit rate

**Success criteria:**
| Metric | Target |
|--------|--------|
| Single transitive (1K nodes) | < 5ms |
| Single transitive (10K nodes) | < 50ms |
| Cache lookup | < 1ms |
| Invalidation (per space) | < 1ms |

## Implementation Order

1. **TreeNode and TransitiveGraph structs**
2. **Basic transitive computation (no caching)**
3. **TransitiveCache with invalidation**
4. **Integration with CanonicalProcessor**
5. **Benchmarks**
6. **Optional: PostgreSQL persistence for hot graphs**

## Open Questions

1. **Eagerness**: Should we compute all transitive graphs eagerly on startup, or lazily on demand?
2. **Granular invalidation**: Can we update transitive graphs incrementally instead of full recomputation?
3. **Memory limits**: Should we cap the number of cached transitive graphs?
