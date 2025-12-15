//! Memory size calculation for graph data structures
//!
//! Provides functions for estimating heap memory usage of graph structures.
//! Useful for benchmarking and monitoring.
//!
//! # Example
//!
//! ```
//! use atlas::graph::{GraphState, TransitiveProcessor, memory};
//!
//! let state = GraphState::new();
//! let breakdown = memory::graph_state_size(&state);
//! println!("GraphState uses {} bytes", breakdown.total_bytes);
//! ```

use super::{EdgeType, GraphState, TransitiveGraph, TreeNode};
use crate::events::{SpaceId, TopicId};
use std::collections::{HashMap, HashSet};
use std::mem;

// ============================================================================
// GraphState memory calculation
// ============================================================================

/// Memory breakdown for GraphState
#[derive(Debug, Clone, Default)]
pub struct GraphStateMemory {
    pub total_bytes: usize,
    pub spaces_bytes: usize,
    pub space_topics_bytes: usize,
    pub topic_spaces_bytes: usize,
    pub explicit_edges_bytes: usize,
    pub topic_edges_bytes: usize,
    pub topic_edge_sources_bytes: usize,
}

/// Calculate memory usage of a GraphState
pub fn graph_state_size(state: &GraphState) -> GraphStateMemory {
    let spaces_bytes = hashset_size::<SpaceId>(&state.spaces);

    let space_topics_bytes = hashmap_simple_size::<SpaceId, TopicId>(&state.space_topics);

    let topic_spaces_bytes = hashmap_with_hashset_size::<TopicId, SpaceId>(&state.topic_spaces);

    let explicit_edges_bytes = hashmap_with_vec_size(&state.explicit_edges);

    let topic_edges_bytes = hashmap_with_hashset_size::<SpaceId, TopicId>(&state.topic_edges);

    let topic_edge_sources_bytes =
        hashmap_with_hashset_size::<TopicId, SpaceId>(&state.topic_edge_sources);

    GraphStateMemory {
        total_bytes: spaces_bytes
            + space_topics_bytes
            + topic_spaces_bytes
            + explicit_edges_bytes
            + topic_edges_bytes
            + topic_edge_sources_bytes,
        spaces_bytes,
        space_topics_bytes,
        topic_spaces_bytes,
        explicit_edges_bytes,
        topic_edges_bytes,
        topic_edge_sources_bytes,
    }
}

// ============================================================================
// TransitiveGraph memory calculation
// ============================================================================

/// Memory breakdown for TransitiveGraph
#[derive(Debug, Clone, Default)]
pub struct TransitiveGraphMemory {
    pub total_bytes: usize,
    pub tree_bytes: usize,
    pub flat_set_bytes: usize,
}

/// Calculate memory usage of a TransitiveGraph
pub fn transitive_graph_size(graph: &TransitiveGraph) -> TransitiveGraphMemory {
    let tree_bytes = tree_node_size(&graph.tree);
    let flat_set_bytes = hashset_size::<SpaceId>(&graph.flat);

    TransitiveGraphMemory {
        total_bytes: tree_bytes + flat_set_bytes,
        tree_bytes,
        flat_set_bytes,
    }
}

// ============================================================================
// TreeNode memory calculation
// ============================================================================

/// Calculate heap memory usage of a TreeNode (recursive)
pub fn tree_node_size(node: &TreeNode) -> usize {
    // Vec allocation for children
    let vec_alloc = node.children.capacity() * mem::size_of::<TreeNode>();
    // Recursive children heap allocations
    let children_heap: usize = node.children.iter().map(tree_node_size).sum();
    vec_alloc + children_heap
}

// ============================================================================
// Cache memory calculation
// ============================================================================

/// Memory breakdown for TransitiveProcessor cache
#[derive(Debug, Clone, Default)]
pub struct CacheMemory {
    pub total_bytes: usize,
    pub full_graphs_bytes: usize,
    pub explicit_only_graphs_bytes: usize,
    pub reverse_deps_bytes: usize,
}

/// Calculate memory usage of a TransitiveProcessor's cache
pub fn processor_cache_size(processor: &super::TransitiveProcessor) -> CacheMemory {
    // We need to compute this through the public API
    // The processor exposes cache_memory_bytes() but we want a breakdown
    // For now, return the total from the processor
    let total = processor.cache_memory_bytes();
    CacheMemory {
        total_bytes: total,
        full_graphs_bytes: 0,          // Would need internal access
        explicit_only_graphs_bytes: 0, // Would need internal access
        reverse_deps_bytes: 0,         // Would need internal access
    }
}

// ============================================================================
// Helper functions for collection sizes
// ============================================================================

/// Estimate heap size of a HashSet
fn hashset_size<T>(set: &HashSet<T>) -> usize {
    // HashSet uses a hash table with overhead per entry
    // Approximation: capacity * (size_of::<T> + 16 bytes metadata)
    set.capacity() * (mem::size_of::<T>() + 16)
}

/// Estimate heap size of a HashMap with simple value types (no nested heap)
fn hashmap_simple_size<K, V>(map: &HashMap<K, V>) -> usize {
    map.capacity() * (mem::size_of::<K>() + mem::size_of::<V>() + 16)
}

/// Estimate heap size of a HashMap<K, HashSet<V>>
fn hashmap_with_hashset_size<K, V>(map: &HashMap<K, HashSet<V>>) -> usize {
    let table_size = map.capacity() * (mem::size_of::<K>() + mem::size_of::<HashSet<V>>() + 16);
    let sets_size: usize = map.values().map(|s| hashset_size(s)).sum();
    table_size + sets_size
}

/// Estimate heap size of HashMap<SpaceId, Vec<(SpaceId, EdgeType)>>
fn hashmap_with_vec_size(map: &HashMap<SpaceId, Vec<(SpaceId, EdgeType)>>) -> usize {
    let table_size = map.capacity()
        * (mem::size_of::<SpaceId>() + mem::size_of::<Vec<(SpaceId, EdgeType)>>() + 16);
    let vecs_size: usize = map
        .values()
        .map(|v| v.capacity() * mem::size_of::<(SpaceId, EdgeType)>())
        .sum();
    table_size + vecs_size
}

// ============================================================================
// Convenience functions
// ============================================================================

/// Format bytes as human-readable string
pub fn format_bytes(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * KB;
    const GB: usize = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Calculate bytes per node for a graph
pub fn bytes_per_node(state: &GraphState) -> f64 {
    let mem = graph_state_size(state);
    let nodes = state.space_count();
    if nodes == 0 {
        0.0
    } else {
        mem.total_bytes as f64 / nodes as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{BlockMetadata, SpaceType};
    use crate::graph::GraphState;

    fn make_space_id(n: u8) -> SpaceId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    fn make_topic_id(n: u8) -> TopicId {
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
        use crate::events::{SpaceCreated, SpaceTopologyEvent, SpaceTopologyPayload};

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

    #[test]
    fn test_empty_graph_state_size() {
        let state = GraphState::new();
        let mem = graph_state_size(&state);
        // Empty state should have zero or minimal allocation
        assert_eq!(mem.total_bytes, 0);
    }

    #[test]
    fn test_graph_state_size_grows() {
        let mut state = GraphState::new();
        let mem1 = graph_state_size(&state);

        for i in 0..10 {
            create_space(&mut state, i);
        }

        let mem2 = graph_state_size(&state);
        assert!(mem2.total_bytes > mem1.total_bytes);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_tree_node_size() {
        use crate::graph::{EdgeType, TreeNode};

        let mut root = TreeNode::new_root(make_space_id(1));
        let size1 = tree_node_size(&root);

        // Add children
        for i in 2..10 {
            root.add_child(TreeNode::new(make_space_id(i), EdgeType::Verified));
        }

        let size2 = tree_node_size(&root);
        assert!(size2 > size1);
    }
}
