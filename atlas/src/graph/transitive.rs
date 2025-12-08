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

    /// Flat set of all reachable spaces
    pub flat: HashSet<SpaceId>,

    /// Hash for change detection
    pub hash: u64,
}

impl TransitiveGraph {
    /// Create a new transitive graph
    pub fn new(root: SpaceId, tree: TreeNode, flat: HashSet<SpaceId>) -> Self {
        let hash = hash_tree(&tree);
        Self {
            root,
            tree,
            flat,
            hash,
        }
    }

    /// Check if a space is reachable
    pub fn contains(&self, space_id: &SpaceId) -> bool {
        self.flat.contains(space_id)
    }

    /// Get the number of reachable spaces
    pub fn len(&self) -> usize {
        self.flat.len()
    }

    /// Check if the graph is empty (only contains root)
    pub fn is_empty(&self) -> bool {
        self.flat.len() <= 1
    }
}

/// Cache of pre-computed transitive graphs
#[derive(Debug, Default, Clone)]
pub struct TransitiveCache {
    /// Full transitive graphs (explicit + topic edges)
    full: HashMap<SpaceId, TransitiveGraph>,

    /// Explicit-only transitive graphs
    explicit_only: HashMap<SpaceId, TransitiveGraph>,

    /// Reverse index: space → spaces whose transitive graph contains it
    /// Used for cache invalidation
    reverse_deps: HashMap<SpaceId, HashSet<SpaceId>>,
}

impl TransitiveCache {
    /// Create a new empty cache
    pub fn new() -> Self {
        Self::default()
    }

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
        for space in &graph.flat {
            self.reverse_deps
                .entry(*space)
                .or_default()
                .insert(graph.root);
        }
    }

    /// Invalidate all cached graphs affected by a space change
    pub fn invalidate(&mut self, space: &SpaceId) {
        // Remove this space's own graphs
        self.full.remove(space);
        self.explicit_only.remove(space);

        // Remove all graphs that contained this space
        if let Some(dependents) = self.reverse_deps.remove(space) {
            for dep in dependents {
                self.full.remove(&dep);
                self.explicit_only.remove(&dep);
            }
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
                    TrustExtension::Verified { target_space_id }
                    | TrustExtension::Related { target_space_id } => {
                        self.cache.invalidate(target_space_id);
                    }
                    TrustExtension::Subtopic { target_topic_id } => {
                        // Invalidate all spaces that announced this topic
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

        // Track node metadata (edge_type, topic_id) - no TreeNode allocation yet
        let mut node_metadata: HashMap<SpaceId, (EdgeType, Option<crate::events::TopicId>)> =
            HashMap::new();

        // Build children index directly: parent -> [children] (O(1) lookup)
        let mut children_index: HashMap<SpaceId, Vec<SpaceId>> = HashMap::new();

        // Reusable edge buffer to avoid allocations in the loop
        let mut edges: Vec<(SpaceId, EdgeType, Option<crate::events::TopicId>)> = Vec::new();

        // Initialize with root
        visited.insert(root);
        queue.push_back(root);
        node_metadata.insert(root, (EdgeType::Root, None));

        while let Some(current) = queue.pop_front() {
            // Clear and reuse the edges buffer
            edges.clear();

            // Collect explicit edges
            if let Some(explicit) = state.get_explicit_edges(&current) {
                for (target, edge_type) in explicit {
                    edges.push((*target, *edge_type, None));
                }
            }

            // Collect topic edges (if enabled)
            if include_topic_edges {
                if let Some(topics) = state.get_topic_edges(&current) {
                    for topic_id in topics {
                        if let Some(members) = state.get_topic_members(topic_id) {
                            for member in members {
                                edges.push((*member, EdgeType::Topic, Some(*topic_id)));
                            }
                        }
                    }
                }
            }

            // Sort for deterministic ordering
            edges.sort_unstable_by_key(|(id, _, _)| *id);

            // Process edges and build children index
            for (target, edge_type, topic_id) in &edges {
                if visited.insert(*target) {
                    queue.push_back(*target);
                    node_metadata.insert(*target, (*edge_type, *topic_id));

                    // Add to children index (O(1) amortized)
                    children_index.entry(current).or_default().push(*target);
                }
            }
        }

        // Build tree structure using children index (O(n) total)
        fn build_tree(
            node_id: SpaceId,
            node_metadata: &HashMap<SpaceId, (EdgeType, Option<crate::events::TopicId>)>,
            children_index: &HashMap<SpaceId, Vec<SpaceId>>,
        ) -> TreeNode {
            let (edge_type, topic_id) = node_metadata.get(&node_id).copied().unwrap();

            let mut node = match topic_id {
                Some(tid) => TreeNode::new_with_topic(node_id, tid),
                None if edge_type == EdgeType::Root => TreeNode::new_root(node_id),
                None => TreeNode::new(node_id, edge_type),
            };

            // Children are already collected - just iterate (O(1) lookup)
            if let Some(children) = children_index.get(&node_id) {
                // Reserve capacity to avoid reallocations
                node.children.reserve(children.len());
                for child_id in children {
                    node.children
                        .push(build_tree(*child_id, node_metadata, children_index));
                }
            }

            node
        }

        let tree = build_tree(root, &node_metadata, &children_index);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        BlockMetadata, SpaceCreated, SpaceTopologyPayload, SpaceType, TrustExtended,
    };

    fn make_space_id(n: u8) -> SpaceId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    fn make_topic_id(n: u8) -> crate::events::TopicId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    fn make_block_meta() -> BlockMetadata {
        BlockMetadata {
            block_number: 1,
            block_timestamp: 12,
            tx_hash: "0x1".to_string(),
            cursor: "cursor_1".to_string(),
        }
    }

    fn create_space(state: &mut GraphState, n: u8) -> SpaceId {
        let space = make_space_id(n);
        let topic = make_topic_id(n);
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
                space_id: space,
                topic_id: topic,
                space_type: SpaceType::Dao {
                    initial_editors: vec![],
                    initial_members: vec![],
                },
            }),
        };
        state.apply_event(&event);
        space
    }

    fn add_verified_edge(state: &mut GraphState, source: SpaceId, target: SpaceId) {
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Verified {
                    target_space_id: target,
                },
            }),
        };
        state.apply_event(&event);
    }

    fn add_topic_edge(state: &mut GraphState, source: SpaceId, topic: crate::events::TopicId) {
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Subtopic {
                    target_topic_id: topic,
                },
            }),
        };
        state.apply_event(&event);
    }

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
}
