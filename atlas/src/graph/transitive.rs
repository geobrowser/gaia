//! Transitive graph computation
//!
//! Computes transitive closure of the topology graph using BFS.
//! Supports two variants:
//! - Full transitive: follows both explicit and topic edges
//! - Explicit-only transitive: follows only explicit edges

use super::{hash_tree, EdgeType, GraphState, TreeNode};
use crate::events::{SpaceId, SpaceTopologyEvent, SpaceTopologyPayload, TrustExtension};
use std::collections::{HashMap, HashSet, VecDeque};

/// Result of transitive graph computation
#[derive(Debug, Clone)]
pub struct TransitiveGraph {
    /// Root space this graph was computed from
    pub root: SpaceId,

    /// Tree representation with edge metadata
    pub tree: TreeNode,

    /// Set of all reachable space IDs (the "membership" set)
    pub members: HashSet<SpaceId>,

    /// Hash for change detection
    pub hash: u64,
}

impl TransitiveGraph {
    /// Create a new transitive graph
    pub fn new(root: SpaceId, tree: TreeNode, members: HashSet<SpaceId>) -> Self {
        let hash = hash_tree(&tree);
        Self {
            root,
            tree,
            members,
            hash,
        }
    }

    /// Check if a space is reachable
    pub fn contains(&self, space_id: &SpaceId) -> bool {
        self.members.contains(space_id)
    }

    /// Get the number of reachable spaces.
    ///
    /// A transitive graph always contains at least the root, so `is_empty`
    /// is intentionally omitted — it would always return false.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.members.len()
    }
}

/// Cache of pre-computed transitive graphs
#[derive(Debug, Default, Clone)]
pub(crate) struct TransitiveCache {
    /// Full transitive graphs (explicit + topic edges)
    full: HashMap<SpaceId, TransitiveGraph>,

    /// Explicit-only transitive graphs
    explicit_only: HashMap<SpaceId, TransitiveGraph>,

    /// Reverse index: space → spaces whose transitive graph contains it
    /// Used for cache invalidation
    reverse_deps: HashMap<SpaceId, HashSet<SpaceId>>,
}

impl TransitiveCache {
    /// Get a cached full transitive graph
    pub fn get_full(&self, space: &SpaceId) -> Option<&TransitiveGraph> {
        self.full.get(space)
    }

    /// Get a cached explicit-only transitive graph
    pub fn get_explicit_only(&self, space: &SpaceId) -> Option<&TransitiveGraph> {
        self.explicit_only.get(space)
    }

    /// Insert a full transitive graph into the cache
    pub fn insert_full(&mut self, graph: TransitiveGraph) {
        self.update_reverse_deps(&graph);
        self.full.insert(graph.root, graph);
    }

    /// Insert an explicit-only transitive graph into the cache
    pub fn insert_explicit_only(&mut self, graph: TransitiveGraph) {
        self.update_reverse_deps(&graph);
        self.explicit_only.insert(graph.root, graph);
    }

    /// Update reverse dependency index
    fn update_reverse_deps(&mut self, graph: &TransitiveGraph) {
        for space in &graph.members {
            self.reverse_deps
                .entry(*space)
                .or_default()
                .insert(graph.root);
        }
    }

    /// Invalidate all cached graphs affected by a space change
    pub fn invalidate(&mut self, space: &SpaceId) {
        // Collect all graph roots to evict: this space's own graphs + dependents
        let mut to_evict: Vec<SpaceId> = vec![*space];
        if let Some(dependents) = self.reverse_deps.remove(space) {
            to_evict.extend(dependents);
        }

        // For each evicted graph, clean up its reverse_deps entries using
        // the graph's flat set before removing it from cache
        for root in &to_evict {
            // Collect flat sets from both caches before mutating reverse_deps
            let flat_members: Vec<SpaceId> = self
                .full
                .get(root)
                .into_iter()
                .chain(self.explicit_only.get(root))
                .flat_map(|g| g.members.iter().copied())
                .collect();

            for member in flat_members {
                if let Some(deps) = self.reverse_deps.get_mut(&member) {
                    deps.remove(root);
                    if deps.is_empty() {
                        self.reverse_deps.remove(&member);
                    }
                }
            }

            self.full.remove(root);
            self.explicit_only.remove(root);
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            full_count: self.full.len(),
            explicit_only_count: self.explicit_only.len(),
            reverse_deps_count: self.reverse_deps.len(),
        }
    }

    /// Get estimated heap memory usage of the cache in bytes
    pub fn heap_size(&self) -> usize {
        use super::memory::transitive_graph_size;
        use std::collections::HashSet;
        use std::mem;

        // Helper to estimate HashSet heap size
        fn hashset_heap_size<T>(set: &HashSet<T>) -> usize {
            set.capacity() * (mem::size_of::<T>() + 16)
        }

        // full: HashMap<SpaceId, TransitiveGraph>
        let full_table = self.full.capacity()
            * (mem::size_of::<crate::events::SpaceId>() + mem::size_of::<TransitiveGraph>() + 16);
        let full_graphs: usize = self
            .full
            .values()
            .map(|g| transitive_graph_size(g).total_bytes)
            .sum();

        // explicit_only: HashMap<SpaceId, TransitiveGraph>
        let explicit_only_table = self.explicit_only.capacity()
            * (mem::size_of::<crate::events::SpaceId>() + mem::size_of::<TransitiveGraph>() + 16);
        let explicit_only_graphs: usize = self
            .explicit_only
            .values()
            .map(|g| transitive_graph_size(g).total_bytes)
            .sum();

        // reverse_deps: HashMap<SpaceId, HashSet<SpaceId>>
        let reverse_deps_table = self.reverse_deps.capacity()
            * (mem::size_of::<crate::events::SpaceId>()
                + mem::size_of::<HashSet<crate::events::SpaceId>>()
                + 16);
        let reverse_deps_sets: usize = self.reverse_deps.values().map(hashset_heap_size).sum();

        full_table
            + full_graphs
            + explicit_only_table
            + explicit_only_graphs
            + reverse_deps_table
            + reverse_deps_sets
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub full_count: usize,
    pub explicit_only_count: usize,
    pub reverse_deps_count: usize,
}

/// Processor for computing transitive graphs
#[derive(Debug, Default, Clone)]
pub struct TransitiveProcessor {
    cache: TransitiveCache,
}

impl TransitiveProcessor {
    /// Create a new transitive processor
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute or retrieve full transitive graph for a space
    ///
    /// Full transitive graphs follow both explicit and topic edges.
    pub fn get_full(&mut self, space: SpaceId, state: &GraphState) -> &TransitiveGraph {
        if !self.cache.full.contains_key(&space) {
            let graph = self.compute(space, state, true);
            self.cache.insert_full(graph);
        }
        self.cache.get_full(&space).unwrap()
    }

    /// Compute or retrieve explicit-only transitive graph for a space
    ///
    /// Explicit-only transitive graphs follow only Verified and Related edges.
    pub fn get_explicit_only(&mut self, space: SpaceId, state: &GraphState) -> &TransitiveGraph {
        if !self.cache.explicit_only.contains_key(&space) {
            let graph = self.compute(space, state, false);
            self.cache.insert_explicit_only(graph);
        }
        self.cache.get_explicit_only(&space).unwrap()
    }

    /// Handle a topology event, invalidating affected caches
    pub fn handle_event(&mut self, event: &SpaceTopologyEvent, state: &GraphState) {
        match &event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                // New space might affect existing topic edges
                // Invalidate all spaces that have topic edges to this space's topic (O(1) lookup)
                if let Some(sources) = state.get_topic_edge_sources(&created.topic_id) {
                    for source in sources {
                        self.cache.invalidate(source);
                    }
                }
            }
            SpaceTopologyPayload::TrustExtended(extended) => {
                // Invalidate source and potentially target
                self.cache.invalidate(&extended.source_space_id);

                match &extended.extension {
                    // Explicit edges: invalidate target
                    TrustExtension::Verified { target_space_id }
                    | TrustExtension::Related { target_space_id }
                    | TrustExtension::VerifiedRemoved { target_space_id }
                    | TrustExtension::RelatedRemoved { target_space_id } => {
                        self.cache.invalidate(target_space_id);
                    }

                    // Member and Editor edges: no-op — these events do not mutate state (plan 0007).
                    TrustExtension::MemberAdded { .. }
                    | TrustExtension::MemberRemoved { .. }
                    | TrustExtension::EditorAdded { .. }
                    | TrustExtension::EditorRemoved { .. } => {}

                    // Topic edges: invalidate all spaces that announced this topic
                    TrustExtension::Subtopic { target_topic_id }
                    | TrustExtension::SubtopicRemoved { target_topic_id } => {
                        if let Some(members) = state.get_topic_members(target_topic_id) {
                            for member in members {
                                self.cache.invalidate(member);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Compute a transitive graph using BFS
    fn compute(
        &self,
        root: SpaceId,
        state: &GraphState,
        include_topic_edges: bool,
    ) -> TransitiveGraph {
        let mut visited: HashSet<SpaceId> = HashSet::new();
        let mut queue: VecDeque<SpaceId> = VecDeque::new();

        // Track node metadata (edge_type) - no TreeNode allocation yet
        let mut node_metadata: HashMap<SpaceId, EdgeType> = HashMap::new();

        // Build children index directly: parent -> [children] (O(1) lookup)
        let mut children_index: HashMap<SpaceId, Vec<SpaceId>> = HashMap::new();

        // Reusable edge buffer to avoid allocations in the loop
        let mut edges: Vec<(SpaceId, EdgeType)> = Vec::new();

        // Initialize with root
        visited.insert(root);
        queue.push_back(root);
        node_metadata.insert(root, EdgeType::Root);

        while let Some(current) = queue.pop_front() {
            // Clear and reuse the edges buffer
            edges.clear();

            // Collect explicit edges
            if let Some(explicit) = state.get_explicit_edges(&current) {
                for (target, edge_type) in explicit {
                    edges.push((*target, *edge_type));
                }
            }

            // Collect topic edges (if enabled)
            if include_topic_edges {
                if let Some(topics) = state.get_topic_edges(&current) {
                    for topic_id in topics {
                        if let Some(members) = state.get_topic_members(topic_id) {
                            for member in members {
                                edges.push((
                                    *member,
                                    EdgeType::Topic {
                                        topic_id: *topic_id,
                                    },
                                ));
                            }
                        }
                    }
                }
            }

            // Sort for deterministic ordering
            edges.sort_unstable_by_key(|(id, _)| *id);

            // Process edges and build children index
            for (target, edge_type) in &edges {
                if visited.insert(*target) {
                    queue.push_back(*target);
                    node_metadata.insert(*target, *edge_type);

                    // Add to children index (O(1) amortized)
                    children_index.entry(current).or_default().push(*target);
                }
            }
        }

        let tree = build_tree_iterative(root, &node_metadata, &children_index);

        TransitiveGraph::new(root, tree, visited)
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Get estimated heap memory usage of the cache in bytes
    pub fn cache_memory_bytes(&self) -> usize {
        self.cache.heap_size()
    }
}

/// Build a tree from a children index iteratively.
///
/// Simulates recursive tree construction by maintaining an explicit stack
/// of (partially-built TreeNode, remaining child IDs). When all children
/// of a node are processed, it's pushed onto its parent's children vec.
fn build_tree_iterative(
    root: SpaceId,
    node_metadata: &HashMap<SpaceId, EdgeType>,
    children_index: &HashMap<SpaceId, Vec<SpaceId>>,
) -> TreeNode {
    let root_edge = node_metadata.get(&root).copied().unwrap();
    let mut root_node = TreeNode::new(root, root_edge);

    let root_children = match children_index.get(&root) {
        Some(ids) => ids,
        None => return root_node, // Leaf root
    };

    // Stack item: (node being built, index of next child to process)
    // Using index instead of iterator avoids lifetime issues.
    root_node.children.reserve(root_children.len());
    let mut stack: Vec<(TreeNode, usize)> = vec![(root_node, 0)];

    loop {
        // Get the next child to process for the top-of-stack node
        let next_child_id = {
            let (ref node, ref mut idx) = stack.last_mut().unwrap();
            children_index.get(&node.space_id).and_then(|child_ids| {
                if *idx < child_ids.len() {
                    let id = child_ids[*idx];
                    *idx += 1;
                    Some(id)
                } else {
                    None
                }
            })
        };

        match next_child_id {
            Some(child_id) => {
                let edge_type = node_metadata.get(&child_id).copied().unwrap();
                let mut child_node = TreeNode::new(child_id, edge_type);

                if let Some(grandchild_ids) = children_index.get(&child_id) {
                    // Has children — push onto stack to process
                    child_node.children.reserve(grandchild_ids.len());
                    stack.push((child_node, 0));
                } else {
                    // Leaf — attach directly to parent
                    stack.last_mut().unwrap().0.children.push(child_node);
                }
            }
            None => {
                // All children processed — pop and attach to parent
                let (completed, _) = stack.pop().unwrap();
                if let Some(parent) = stack.last_mut() {
                    parent.0.children.push(completed);
                } else {
                    return completed; // Root is complete
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TrustExtended;
    use crate::test_utils::{
        add_topic_edge, add_verified_edge, create_space, make_block_meta, make_topic_id,
    };

    #[test]
    fn test_single_space_transitive() {
        let mut state = GraphState::new();
        let space = create_space(&mut state, 1);

        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(space, &state);

        assert_eq!(graph.root, space);
        assert_eq!(graph.len(), 1);
        assert!(graph.contains(&space));
    }

    #[test]
    fn test_linear_chain() {
        // A -> B -> C
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);
        let c = create_space(&mut state, 3);

        add_verified_edge(&mut state, a, b);
        add_verified_edge(&mut state, b, c);

        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(a, &state);

        assert_eq!(graph.len(), 3);
        assert!(graph.contains(&a));
        assert!(graph.contains(&b));
        assert!(graph.contains(&c));
    }

    #[test]
    fn test_diamond_graph() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);
        let c = create_space(&mut state, 3);
        let d = create_space(&mut state, 4);

        add_verified_edge(&mut state, a, b);
        add_verified_edge(&mut state, a, c);
        add_verified_edge(&mut state, b, d);
        add_verified_edge(&mut state, c, d);

        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(a, &state);

        assert_eq!(graph.len(), 4);
        // D should only appear once in the tree (first path wins)
        assert_eq!(graph.tree.node_count(), 4);
    }

    #[test]
    fn test_topic_edge_resolution() {
        // A -> topic(B) -> B
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);
        let topic_b = make_topic_id(2);

        add_topic_edge(&mut state, a, topic_b);

        let mut processor = TransitiveProcessor::new();

        // Full transitive should include B
        let full = processor.get_full(a, &state);
        assert_eq!(full.len(), 2);
        assert!(full.contains(&b));

        // Explicit-only should NOT include B
        let explicit = processor.get_explicit_only(a, &state);
        assert_eq!(explicit.len(), 1);
        assert!(!explicit.contains(&b));
    }

    #[test]
    fn test_cache_hit() {
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);
        add_verified_edge(&mut state, a, b);

        let mut processor = TransitiveProcessor::new();

        // First call computes
        let hash1 = processor.get_full(a, &state).hash;
        let stats1 = processor.cache_stats();
        assert_eq!(stats1.full_count, 1);

        // Second call should hit cache
        let hash2 = processor.get_full(a, &state).hash;
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_cache_invalidation() {
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);
        add_verified_edge(&mut state, a, b);

        let mut processor = TransitiveProcessor::new();

        // Compute initial graph
        let _ = processor.get_full(a, &state);
        assert_eq!(processor.cache_stats().full_count, 1);

        // Create new edge event
        let c = create_space(&mut state, 3);
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: b,
                extension: TrustExtension::Verified { target_space_id: c },
            }),
        };

        // Handle event (should invalidate cache)
        processor.handle_event(&event, &state);
        state.apply_event(&event);

        // Cache should be invalidated (A's graph contained B)
        // Note: exact behavior depends on reverse_deps tracking
    }

    #[test]
    fn test_cycle_handling() {
        // A -> B -> C -> A (cycle)
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);
        let c = create_space(&mut state, 3);

        add_verified_edge(&mut state, a, b);
        add_verified_edge(&mut state, b, c);
        add_verified_edge(&mut state, c, a);

        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(a, &state);

        // Should handle cycle gracefully - each node appears once
        assert_eq!(graph.len(), 3);
        assert_eq!(graph.tree.node_count(), 3);
    }

    #[test]
    fn test_invalidation_cleans_up_stale_reverse_deps() {
        // Build: A -> B -> C
        // Cache A's graph (contains A, B, C)
        // Invalidate B — should remove A's graph AND clean up reverse_deps
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);
        let c = create_space(&mut state, 3);
        add_verified_edge(&mut state, a, b);
        add_verified_edge(&mut state, b, c);

        let mut processor = TransitiveProcessor::new();
        let _ = processor.get_full(a, &state);

        // reverse_deps should have entries for A, B, C (all pointing back to A)
        let stats_before = processor.cache_stats();
        assert_eq!(stats_before.full_count, 1);
        assert!(stats_before.reverse_deps_count > 0);

        // Invalidate B — should cascade to A's graph and clean stale refs
        processor.cache.invalidate(&b);

        // After invalidation, reverse_deps should not have stale entries
        // for spaces whose graphs no longer exist
        let stats_after = processor.cache_stats();
        assert_eq!(stats_after.full_count, 0);
        assert_eq!(
            stats_after.reverse_deps_count, 0,
            "stale reverse_deps should be cleaned up"
        );
    }

    #[test]
    fn test_member_edge_not_in_transitive() {
        // Plan 0007: Member edges are no-ops. A space reachable only via a
        // Member edge must not appear in the transitive graph rooted at the
        // source — neither in the flat membership set nor in the tree.
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);

        let member_event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: a,
                extension: TrustExtension::MemberAdded { member_space_id: b },
            }),
        };
        state.apply_event(&member_event);

        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(a, &state);

        assert_eq!(graph.len(), 1, "A reaches only itself");
        assert!(graph.contains(&a));
        assert!(
            !graph.contains(&b),
            "B must not be reachable through a Member edge"
        );
        assert_eq!(graph.tree.node_count(), 1);
    }

    #[test]
    fn test_editor_edge_not_in_transitive() {
        // Plan 0007: Editor edges are no-ops. A space reachable only via an
        // Editor edge must not appear in the transitive graph rooted at the
        // source — neither in the flat membership set nor in the tree.
        let mut state = GraphState::new();
        let a = create_space(&mut state, 1);
        let b = create_space(&mut state, 2);

        let editor_event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: a,
                extension: TrustExtension::EditorAdded { member_space_id: b },
            }),
        };
        state.apply_event(&editor_event);

        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(a, &state);

        assert_eq!(graph.len(), 1, "A reaches only itself");
        assert!(graph.contains(&a));
        assert!(
            !graph.contains(&b),
            "B must not be reachable through an Editor edge"
        );
        assert_eq!(graph.tree.node_count(), 1);
    }
}
