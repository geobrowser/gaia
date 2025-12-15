//! Canonical graph computation
//!
//! Computes the canonical graph from a designated root node using a two-phase algorithm:
//! - Phase 1: Traverse explicit edges only to establish the canonical (trusted) node set
//! - Phase 2: Add topic edges, but only connecting nodes already in the canonical set
//!
//! The canonical graph represents the "trusted" portion of the topology graph,
//! where trust flows only through explicit edges (Verified, Related).

use super::{hash_tree, GraphState, TransitiveProcessor, TreeNode};
use crate::events::{SpaceId, SpaceTopologyEvent, SpaceTopologyPayload, TopicId};
use std::collections::HashSet;

/// Result of canonical graph computation
#[derive(Debug, Clone)]
pub struct CanonicalGraph {
    /// Root space this graph was computed from
    pub root: SpaceId,

    /// Tree representation with edge metadata
    /// The tree structure preserves distance from root
    pub tree: TreeNode,

    /// Flat set of all canonical spaces
    pub flat: HashSet<SpaceId>,
}

impl CanonicalGraph {
    /// Create a new canonical graph
    pub fn new(root: SpaceId, tree: TreeNode, flat: HashSet<SpaceId>) -> Self {
        Self { root, tree, flat }
    }

    /// Check if a space is in the canonical set
    pub fn contains(&self, space_id: &SpaceId) -> bool {
        self.flat.contains(space_id)
    }

    /// Get the number of canonical spaces
    pub fn len(&self) -> usize {
        self.flat.len()
    }

    /// Check if the graph is empty (only contains root)
    pub fn is_empty(&self) -> bool {
        self.flat.len() <= 1
    }
}

/// Processor for computing canonical graphs
///
/// Uses `TransitiveProcessor` to leverage pre-computed transitive graphs
/// for efficient canonical graph computation.
#[derive(Debug)]
pub struct CanonicalProcessor {
    /// The root space for canonical graph computation
    root: SpaceId,

    /// Hash of the last computed tree structure
    /// Used to detect changes in tree structure (not just canonical set)
    last_hash: Option<u64>,
}

impl CanonicalProcessor {
    /// Create a new canonical processor with the given root
    pub fn new(root: SpaceId) -> Self {
        Self {
            root,
            last_hash: None,
        }
    }

    /// Get the root space ID
    pub fn root(&self) -> SpaceId {
        self.root
    }

    /// Check if an event can affect the canonical graph
    ///
    /// This is an optimization to skip recomputation for events that
    /// cannot possibly change the canonical graph.
    pub fn affects_canonical(
        &self,
        event: &SpaceTopologyEvent,
        canonical_set: &HashSet<SpaceId>,
    ) -> bool {
        match &event.payload {
            // New spaces are not canonical until reached via explicit edges from root
            SpaceTopologyPayload::SpaceCreated(_) => false,

            SpaceTopologyPayload::TrustExtended(extended) => {
                // Only events from canonical sources can affect the canonical graph
                canonical_set.contains(&extended.source_space_id)
            }
        }
    }

    /// Compute the canonical graph
    ///
    /// Returns `Some(CanonicalGraph)` if the tree structure changed since the last
    /// computation, `None` if the tree is identical.
    ///
    /// The algorithm has two phases:
    /// 1. Get the canonical set from root's explicit-only transitive graph
    /// 2. Add topic edges, attaching filtered subtrees for canonical members
    ///
    /// Use `affects_canonical` to check if an event could possibly require
    /// recomputation before calling this method.
    ///
    /// Note: Even if `affects_canonical` returns true, the tree structure may
    /// not actually change (e.g., adding a duplicate edge). The hash comparison
    /// detects this case.
    pub fn compute(
        &mut self,
        state: &GraphState,
        transitive: &mut TransitiveProcessor,
    ) -> Option<CanonicalGraph> {
        // Phase 1: Get canonical set from root's explicit-only transitive graph
        // This gives us all nodes reachable via explicit edges (Verified, Related)
        let root_transitive = transitive.get_explicit_only(self.root, state);
        let canonical_set = root_transitive.flat.clone();
        let mut tree = root_transitive.tree.clone();

        // Phase 2: Add topic edges with filtered subtrees
        // Collect all topic edges from canonical nodes
        let topic_edges = self.collect_topic_edges(&canonical_set, state);

        // Process each topic edge
        for (source, topic_id) in topic_edges {
            self.process_topic_edge(
                source,
                topic_id,
                &canonical_set,
                state,
                transitive,
                &mut tree,
            );
        }

        let graph = CanonicalGraph::new(self.root, tree, canonical_set);

        // Check if tree structure changed
        let new_hash = hash_tree(&graph.tree);
        if self.last_hash == Some(new_hash) {
            return None;
        }

        self.last_hash = Some(new_hash);
        Some(graph)
    }

    /// Collect all topic edges from canonical nodes
    ///
    /// Returns a sorted list of (source, topic_id) pairs for deterministic processing.
    fn collect_topic_edges(
        &self,
        canonical_set: &HashSet<SpaceId>,
        state: &GraphState,
    ) -> Vec<(SpaceId, TopicId)> {
        let mut topic_edges: Vec<(SpaceId, TopicId)> = Vec::new();

        for source in canonical_set {
            if let Some(topics) = state.get_topic_edges(source) {
                for topic_id in topics {
                    topic_edges.push((*source, *topic_id));
                }
            }
        }

        // Sort for deterministic ordering
        topic_edges.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        topic_edges
    }

    /// Process a single topic edge
    ///
    /// For each topic edge (source -> topic_id):
    /// 1. Resolve topic to spaces that announced it
    /// 2. Filter to spaces in the canonical set
    /// 3. For each canonical member, attach its filtered subtree
    fn process_topic_edge(
        &self,
        source: SpaceId,
        topic_id: TopicId,
        canonical_set: &HashSet<SpaceId>,
        state: &GraphState,
        transitive: &mut TransitiveProcessor,
        tree: &mut TreeNode,
    ) {
        // Get all spaces that announced this topic
        let members = match state.get_topic_members(&topic_id) {
            Some(m) => m,
            None => return,
        };

        // Filter to canonical members and sort for deterministic ordering
        let mut canonical_members: Vec<SpaceId> = members
            .iter()
            .filter(|m| canonical_set.contains(*m))
            .copied()
            .collect();
        canonical_members.sort();

        // For each canonical member, attach its filtered subtree
        for member in canonical_members {
            // Get the member's full transitive graph
            let member_transitive = transitive.get_full(member, state);

            // Filter the subtree to only include canonical nodes
            let filtered_subtree =
                self.filter_to_canonical(&member_transitive.tree, canonical_set, topic_id);

            // Attach the filtered subtree to the source node in the tree
            attach_subtree(tree, source, filtered_subtree);
        }
    }

    /// Filter a transitive tree to only include canonical nodes
    ///
    /// Creates a new tree containing only nodes that are in the canonical set.
    /// The root of the subtree is marked as a topic edge with the given topic_id.
    fn filter_to_canonical(
        &self,
        subtree: &TreeNode,
        canonical_set: &HashSet<SpaceId>,
        topic_id: TopicId,
    ) -> TreeNode {
        // Create the root of the filtered subtree as a topic edge
        let mut filtered = TreeNode::new_with_topic(subtree.space_id, topic_id);

        // Recursively filter children
        for child in &subtree.children {
            if canonical_set.contains(&child.space_id) {
                filtered
                    .children
                    .push(filter_child_recursive(child, canonical_set));
            }
        }

        filtered
    }
}

/// Recursively filter a child node and its descendants
///
/// Unlike `filter_to_canonical`, this preserves the original edge type
/// since we're not at the root of the topic edge attachment.
fn filter_child_recursive(node: &TreeNode, canonical_set: &HashSet<SpaceId>) -> TreeNode {
    let mut filtered = TreeNode::new(node.space_id, node.edge_type);
    filtered.topic_id = node.topic_id;

    for child in &node.children {
        if canonical_set.contains(&child.space_id) {
            filtered
                .children
                .push(filter_child_recursive(child, canonical_set));
        }
    }

    filtered
}

/// Attach a subtree to a source node in the tree
///
/// Finds the source node in the tree and adds the subtree as a child.
fn attach_subtree(tree: &mut TreeNode, source: SpaceId, subtree: TreeNode) {
    if tree.space_id == source {
        tree.children.push(subtree);
        return;
    }

    for child in &mut tree.children {
        attach_subtree(child, source, subtree.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        BlockMetadata, SpaceCreated, SpaceTopologyPayload, SpaceType, TrustExtended, TrustExtension,
    };

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

    fn create_space_with_topic(state: &mut GraphState, n: u8, topic_n: u8) -> SpaceId {
        let space = make_space_id(n);
        let topic = make_topic_id(topic_n);
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

    fn add_topic_edge(state: &mut GraphState, source: SpaceId, topic: TopicId) {
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
    fn test_single_space_canonical() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor.compute(&state, &mut transitive).unwrap();

        assert_eq!(graph.root, root);
        assert_eq!(graph.len(), 1);
        assert!(graph.contains(&root));
    }

    #[test]
    fn test_explicit_edges_only() {
        // Root -> A -> B
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let a = create_space(&mut state, 2);
        let b = create_space(&mut state, 3);

        add_verified_edge(&mut state, root, a);
        add_verified_edge(&mut state, a, b);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor.compute(&state, &mut transitive).unwrap();

        assert_eq!(graph.len(), 3);
        assert!(graph.contains(&root));
        assert!(graph.contains(&a));
        assert!(graph.contains(&b));
    }

    #[test]
    fn test_topic_edge_to_canonical_member() {
        // Root -> A (explicit)
        // Root -> topic(B) where B is canonical via explicit path
        // Root -> B (explicit)
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let a = create_space(&mut state, 2);
        let b = create_space(&mut state, 3);
        let topic_b = make_topic_id(3); // B announces topic 3

        add_verified_edge(&mut state, root, a);
        add_verified_edge(&mut state, root, b);
        add_topic_edge(&mut state, root, topic_b);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor.compute(&state, &mut transitive).unwrap();

        // All three should be canonical
        assert_eq!(graph.len(), 3);
        assert!(graph.contains(&root));
        assert!(graph.contains(&a));
        assert!(graph.contains(&b));

        // B should appear twice in the tree: once via explicit edge, once via topic
        // (the tree preserves both paths)
        assert!(graph.tree.node_count() >= 3);
    }

    #[test]
    fn test_topic_edge_to_non_canonical_member() {
        // Root -> A (explicit)
        // Root -> topic(C) where C is NOT canonical (no explicit path)
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let a = create_space(&mut state, 2);
        let c = create_space(&mut state, 3);
        let topic_c = make_topic_id(3); // C announces topic 3

        add_verified_edge(&mut state, root, a);
        add_topic_edge(&mut state, root, topic_c);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor.compute(&state, &mut transitive).unwrap();

        // C should NOT be canonical (only reachable via topic edge)
        assert_eq!(graph.len(), 2);
        assert!(graph.contains(&root));
        assert!(graph.contains(&a));
        assert!(!graph.contains(&c));
    }

    #[test]
    fn test_topic_edge_includes_transitive_subtree() {
        // Root -> A (explicit)
        // A -> topic(B) where B has explicit children C, D
        // B -> C -> D (explicit edges)
        // Root -> B (explicit, making B canonical)
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let a = create_space(&mut state, 2);
        let b = create_space(&mut state, 3);
        let c = create_space(&mut state, 4);
        let d = create_space(&mut state, 5);
        let topic_b = make_topic_id(3); // B announces topic 3

        // Explicit edges to make B, C, D canonical
        add_verified_edge(&mut state, root, a);
        add_verified_edge(&mut state, root, b);
        add_verified_edge(&mut state, b, c);
        add_verified_edge(&mut state, c, d);

        // Topic edge from A to B's topic
        add_topic_edge(&mut state, a, topic_b);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor.compute(&state, &mut transitive).unwrap();

        // All should be canonical
        assert_eq!(graph.len(), 5);
        assert!(graph.contains(&root));
        assert!(graph.contains(&a));
        assert!(graph.contains(&b));
        assert!(graph.contains(&c));
        assert!(graph.contains(&d));

        // Tree should have B's subtree attached under A via topic edge
        // as well as under Root via explicit edge
    }

    #[test]
    fn test_affects_canonical_space_created() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let canonical_set: HashSet<SpaceId> = [root].into_iter().collect();

        let processor = CanonicalProcessor::new(root);

        // SpaceCreated events don't affect canonical
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
                space_id: make_space_id(99),
                topic_id: make_topic_id(99),
                space_type: SpaceType::Dao {
                    initial_editors: vec![],
                    initial_members: vec![],
                },
            }),
        };

        assert!(!processor.affects_canonical(&event, &canonical_set));
    }

    #[test]
    fn test_affects_canonical_from_canonical_source() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let a = create_space(&mut state, 2);
        add_verified_edge(&mut state, root, a);

        let canonical_set: HashSet<SpaceId> = [root, a].into_iter().collect();
        let processor = CanonicalProcessor::new(root);

        // Edge from canonical source should affect canonical
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: a,
                extension: TrustExtension::Verified {
                    target_space_id: make_space_id(99),
                },
            }),
        };

        assert!(processor.affects_canonical(&event, &canonical_set));
    }

    #[test]
    fn test_affects_canonical_from_non_canonical_source() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let non_canonical = create_space(&mut state, 99);

        let canonical_set: HashSet<SpaceId> = [root].into_iter().collect();
        let processor = CanonicalProcessor::new(root);

        // Edge from non-canonical source should NOT affect canonical
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: non_canonical,
                extension: TrustExtension::Verified {
                    target_space_id: make_space_id(100),
                },
            }),
        };

        assert!(!processor.affects_canonical(&event, &canonical_set));
    }

    #[test]
    fn test_change_detection() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        // First computation should return a graph
        let graph1 = processor.compute(&state, &mut transitive);
        assert!(graph1.is_some());
        let graph1 = graph1.unwrap();

        // Second computation with no changes should return None
        let graph2 = processor.compute(&state, &mut transitive);
        assert!(graph2.is_none());

        // Add a new edge
        let a = create_space(&mut state, 2);
        add_verified_edge(&mut state, root, a);

        // Need to invalidate transitive cache
        transitive.handle_event(
            &SpaceTopologyEvent {
                meta: make_block_meta(),
                payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                    source_space_id: root,
                    extension: TrustExtension::Verified { target_space_id: a },
                }),
            },
            &state,
        );

        // Third computation should return a new graph (tree structure changed)
        let graph3 = processor.compute(&state, &mut transitive);
        assert!(graph3.is_some());
        let graph3 = graph3.unwrap();

        // Verify the graphs are different
        assert_eq!(graph1.len(), 1); // Just root
        assert_eq!(graph3.len(), 2); // Root + A
    }

    #[test]
    fn test_multiple_spaces_same_topic() {
        // Multiple spaces announce the same topic
        // Only canonical members should be included via topic edge
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let shared_topic = make_topic_id(100);

        // A and B both announce shared_topic, both canonical
        let a = create_space_with_topic(&mut state, 2, 100);
        let b = create_space_with_topic(&mut state, 3, 100);

        // C announces shared_topic but is NOT canonical
        let _c = create_space_with_topic(&mut state, 4, 100);

        // Make A and B canonical via explicit edges
        add_verified_edge(&mut state, root, a);
        add_verified_edge(&mut state, root, b);

        // Root has topic edge to shared_topic
        add_topic_edge(&mut state, root, shared_topic);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor.compute(&state, &mut transitive).unwrap();

        // Should have Root, A, B (not C)
        assert_eq!(graph.len(), 3);
        assert!(graph.contains(&root));
        assert!(graph.contains(&a));
        assert!(graph.contains(&b));
    }

    #[test]
    fn test_filtered_subtree_preserves_canonical_only() {
        // B has children C (canonical) and D (non-canonical)
        // Topic edge should include C but not D in subtree
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let a = create_space(&mut state, 2);
        let b = create_space(&mut state, 3);
        let c = create_space(&mut state, 4);
        let d = create_space(&mut state, 5);

        // Make Root -> A, Root -> B, B -> C, B -> D canonical
        add_verified_edge(&mut state, root, a);
        add_verified_edge(&mut state, root, b);
        add_verified_edge(&mut state, b, c);
        add_verified_edge(&mut state, b, d);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor.compute(&state, &mut transitive).unwrap();

        // All explicitly connected nodes are canonical
        assert_eq!(graph.len(), 5);
    }
}
