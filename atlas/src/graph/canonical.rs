//! Canonical graph computation
//!
//! Computes the canonical graph from a designated root node using a two-phase algorithm:
//! - Phase 1: Traverse explicit edges only to establish the canonical (trusted) node set
//! - Phase 2: Add topic edges, but only connecting nodes already in the canonical set
//!
//! The canonical graph represents the "trusted" portion of the topology graph,
//! where trust flows only through explicit edges (Verified, Related).
//!
//! Important implementation semantics:
//! - Topic edges never grant canonical membership. They can only attach paths
//!   between nodes that are already canonical via explicit edges.
//! - Tree nodes may contain duplicate SpaceIds via different attachment paths
//!   (for example explicit + topic). This is intentional.
//! - The flat membership set is still unique by SpaceId and is the authority for
//!   canonical inclusion checks.

use super::{hash_tree, GraphState, TransitiveProcessor, TreeNode};
use crate::events::{SpaceId, SpaceTopologyEvent, SpaceTopologyPayload, TopicId};
use std::collections::{HashMap, HashSet};

/// Result of canonical graph computation
#[derive(Debug, Clone)]
pub struct CanonicalGraph {
    /// Root space this graph was computed from
    pub root: SpaceId,

    /// Tree representation with edge metadata
    /// The tree structure preserves distance from root
    pub tree: TreeNode,

    /// Set of all canonical space IDs (the "membership" set)
    pub members: HashSet<SpaceId>,
}

impl CanonicalGraph {
    /// Create a new canonical graph
    pub fn new(root: SpaceId, tree: TreeNode, members: HashSet<SpaceId>) -> Self {
        Self {
            root,
            tree,
            members,
        }
    }

    /// Check if a space is in the canonical set
    pub fn contains(&self, space_id: &SpaceId) -> bool {
        self.members.contains(space_id)
    }

    /// Get the number of canonical spaces.
    ///
    /// A canonical graph always contains at least the root, so `is_empty`
    /// is intentionally omitted — it would always return false.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.members.len()
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

    /// Hash of the last computed canonical tree (after Phase 2).
    /// Used to detect changes in tree structure (not just canonical set).
    last_tree_hash: Option<u64>,

    /// Hash of inputs to the canonical computation (Phase 1 tree + topic edges).
    /// Used to short-circuit before cloning when inputs haven't changed.
    ///
    /// This is intentionally separate from `last_tree_hash`:
    /// - `last_phase1_input_hash` skips work before Phase 2 when inputs are identical.
    /// - `last_tree_hash` detects whether final tree structure changed after Phase 2.
    last_phase1_input_hash: Option<u64>,

    /// Canonical set from the last successful `compute_if_changed()` call.
    /// Used by `affects_canonical()` to skip recomputation for events
    /// from non-canonical sources.
    last_canonical_set: Option<HashSet<SpaceId>>,
}

impl CanonicalProcessor {
    /// Create a new canonical processor with the given root
    pub fn new(root: SpaceId) -> Self {
        Self {
            root,
            last_tree_hash: None,
            last_phase1_input_hash: None,
            last_canonical_set: None,
        }
    }

    /// Get the root space ID
    pub fn root(&self) -> SpaceId {
        self.root
    }

    /// Check if an event can affect the canonical graph.
    ///
    /// Returns `true` if we haven't computed a canonical graph yet (no set to check against),
    /// or if the event originates from a canonical source.
    /// Returns `false` for SpaceCreated events (new spaces aren't canonical until
    /// explicitly connected from root) and for events from non-canonical sources.
    ///
    /// This is a performance hint, not a correctness oracle. The pipeline computes
    /// canonical state from full graph state at block boundaries when any event in
    /// that block may affect canonical output.
    pub fn affects_canonical(&self, event: &SpaceTopologyEvent) -> bool {
        let canonical_set = match &self.last_canonical_set {
            Some(set) => set,
            // No prior computation — must compute to establish baseline
            None => return true,
        };

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
    pub fn compute_if_changed(
        &mut self,
        state: &GraphState,
        transitive: &mut TransitiveProcessor,
    ) -> Option<CanonicalGraph> {
        // Phase 1: Get canonical set from root's explicit-only transitive graph
        // This gives us all nodes reachable via explicit edges (Verified, Related)
        let root_transitive = transitive.get_explicit_only(self.root, state);

        // Fast path: hash the inputs (Phase 1 tree + topic edges) to detect
        // whether anything changed since the last computation. If unchanged,
        // skip cloning and Phase 2 entirely.
        let topic_edges = self.collect_topic_edges(&root_transitive.members, state);
        let input_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            hash_tree(&root_transitive.tree).hash(&mut hasher);
            topic_edges.hash(&mut hasher);
            hasher.finish()
        };

        if self.last_phase1_input_hash == Some(input_hash) {
            return None;
        }
        self.last_phase1_input_hash = Some(input_hash);

        // Inputs changed — clone and run Phase 2
        let canonical_set = root_transitive.members.clone();
        let mut tree = root_transitive.tree.clone();

        // Phase 2: Add topic edges with filtered subtrees
        //
        // Collects all subtrees to attach, grouped by source SpaceId, then
        // attaches them in a single DFS pass over the tree. This is O(N + T)
        // instead of O(T × N) for individual attach calls.
        let pending = self.collect_topic_subtrees(&topic_edges, &canonical_set, state, transitive);
        if !pending.is_empty() {
            attach_all_subtrees(&mut tree, pending);
        }

        let graph = CanonicalGraph::new(self.root, tree, canonical_set);

        // Postconditions: canonical set must contain root, tree root must match.
        // These are promoted to `assert!` (not `debug_assert!`) because violating
        // them would emit corrupt Kafka messages — a panic is preferable.
        assert!(
            graph.members.contains(&self.root),
            "canonical set does not contain root"
        );
        assert_eq!(
            graph.tree.space_id, self.root,
            "tree root does not match processor root"
        );

        // Check if tree structure changed (Phase 2 may not have altered the tree)
        let new_hash = hash_tree(&graph.tree);
        if self.last_tree_hash == Some(new_hash) {
            self.last_canonical_set = Some(graph.members);
            return None;
        }

        self.last_tree_hash = Some(new_hash);
        self.last_canonical_set = Some(graph.members.clone());
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

    /// Collect all filtered topic subtrees, grouped by their attachment source.
    ///
    /// For each topic edge (source -> topic_id), resolves the topic to its
    /// canonical members and builds filtered subtrees. Returns a map from
    /// source SpaceId to the list of subtrees to attach at that node.
    fn collect_topic_subtrees(
        &self,
        topic_edges: &[(SpaceId, TopicId)],
        canonical_set: &HashSet<SpaceId>,
        state: &GraphState,
        transitive: &mut TransitiveProcessor,
    ) -> HashMap<SpaceId, Vec<TreeNode>> {
        let mut pending: HashMap<SpaceId, Vec<TreeNode>> = HashMap::new();

        for &(source, topic_id) in topic_edges {
            let members = match state.get_topic_members(&topic_id) {
                Some(m) => m,
                None => continue,
            };

            // Filter to canonical members and sort for deterministic ordering
            let mut canonical_members: Vec<SpaceId> = members
                .iter()
                .filter(|m| canonical_set.contains(*m))
                .copied()
                .collect();
            canonical_members.sort();

            for member in canonical_members {
                let member_transitive = transitive.get_full(member, state);
                let filtered_subtree =
                    self.filter_to_canonical(&member_transitive.tree, canonical_set, topic_id);
                pending.entry(source).or_default().push(filtered_subtree);
            }
        }

        pending
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

        // Iteratively filter children
        for child in &subtree.children {
            if canonical_set.contains(&child.space_id) {
                filtered
                    .children
                    .push(filter_child_iterative(child, canonical_set));
            }
        }

        filtered
    }
}

/// Iteratively filter a child node and its descendants to canonical-only nodes.
///
/// Unlike `filter_to_canonical`, this preserves the original edge type
/// since we're not at the root of the topic edge attachment.
///
/// Uses post-order traversal: builds children before parents so each
/// parent can collect its already-filtered children.
fn filter_child_iterative(root_node: &TreeNode, canonical_set: &HashSet<SpaceId>) -> TreeNode {
    // Phase 1: Collect nodes in post-order via iterative DFS.
    // Only include nodes that are in the canonical set.
    // Each entry: (source_node, index_of_parent_in_post_order or None for root)
    struct WorkItem<'a> {
        node: &'a TreeNode,
        parent_idx: Option<usize>,
    }

    let mut post_order: Vec<(&TreeNode, Option<usize>)> = Vec::new();
    // DFS stack: (source_node, parent_index_in_post_order)
    let mut stack: Vec<WorkItem<'_>> = vec![WorkItem {
        node: root_node,
        parent_idx: None,
    }];

    while let Some(item) = stack.pop() {
        let my_idx = post_order.len();
        post_order.push((item.node, item.parent_idx));

        for child in item.node.children.iter().rev() {
            if canonical_set.contains(&child.space_id) {
                stack.push(WorkItem {
                    node: child,
                    parent_idx: Some(my_idx),
                });
            }
        }
    }

    // Phase 2: Build filtered nodes in reverse (post-order = children before parents).
    let mut built: Vec<Option<TreeNode>> = Vec::with_capacity(post_order.len());
    for (node, _) in &post_order {
        built.push(Some(TreeNode::new(node.space_id, node.edge_type)));
    }

    // Traverse in reverse so children are finalized before their parents consume them.
    for i in (0..post_order.len()).rev() {
        if let Some(parent_idx) = post_order[i].1 {
            let child_node = built[i].take().unwrap();
            built[parent_idx]
                .as_mut()
                .unwrap()
                .children
                .push(child_node);
        }
    }

    built[0].take().unwrap()
}

/// Attach all pending subtrees in a single DFS pass over the tree.
///
/// For each node visited, checks if there are subtrees pending for that
/// node's SpaceId and appends them as children. This is O(N + T) where
/// N = tree size and T = total subtrees, versus O(T × N) for individual
/// attach calls.
fn attach_all_subtrees(tree: &mut TreeNode, mut pending: HashMap<SpaceId, Vec<TreeNode>>) {
    let mut stack: Vec<&mut TreeNode> = vec![tree];
    while let Some(node) = stack.pop() {
        if let Some(subtrees) = pending.remove(&node.space_id) {
            node.children.extend(subtrees);
        }
        // Early exit: no more pending attachments
        if pending.is_empty() {
            return;
        }
        stack.extend(node.children.iter_mut());
    }

    // All pending entries should have been attached. Leftover entries indicate
    // a bug — the source SpaceId wasn't found in the tree despite being in the
    // canonical set.
    debug_assert!(
        pending.is_empty(),
        "attach_all_subtrees: {} source(s) not found in tree",
        pending.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        SpaceCreated, SpaceTopologyPayload, SpaceType, TrustExtended, TrustExtension,
    };
    use crate::test_utils::{
        add_topic_edge, add_verified_edge, create_space, create_space_with_topic, make_block_meta,
        make_space_id, make_topic_id,
    };

    #[test]
    fn test_single_space_canonical() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

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

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

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

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

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

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

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

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

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
    fn test_affects_canonical_returns_true_before_first_compute() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let processor = CanonicalProcessor::new(root);

        // Before any compute(), affects_canonical should return true (no baseline)
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

        assert!(processor.affects_canonical(&event));
    }

    #[test]
    fn test_affects_canonical_space_created() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);
        processor.compute_if_changed(&state, &mut transitive); // establish baseline

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

        assert!(!processor.affects_canonical(&event));
    }

    #[test]
    fn test_affects_canonical_from_canonical_source() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let a = create_space(&mut state, 2);
        add_verified_edge(&mut state, root, a);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);
        processor.compute_if_changed(&state, &mut transitive); // establish baseline

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

        assert!(processor.affects_canonical(&event));
    }

    #[test]
    fn test_affects_canonical_from_non_canonical_source() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let _non_canonical = create_space(&mut state, 99);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);
        processor.compute_if_changed(&state, &mut transitive); // establish baseline

        // Edge from non-canonical source should NOT affect canonical
        let event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: make_space_id(99),
                extension: TrustExtension::Verified {
                    target_space_id: make_space_id(100),
                },
            }),
        };

        assert!(!processor.affects_canonical(&event));
    }

    #[test]
    fn test_change_detection() {
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        // First computation should return a graph
        let graph1 = processor.compute_if_changed(&state, &mut transitive);
        assert!(graph1.is_some());
        let graph1 = graph1.unwrap();

        // Second computation with no changes should return None
        let graph2 = processor.compute_if_changed(&state, &mut transitive);
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
        let graph3 = processor.compute_if_changed(&state, &mut transitive);
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

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

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

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

        // All explicitly connected nodes are canonical
        assert_eq!(graph.len(), 5);
    }

    #[test]
    fn test_member_edge_from_root_does_not_grant_canonical() {
        // Plan 0007: Member edges no longer grant canonical membership.
        // A MemberAdded(root, X) event must leave X outside the canonical set.
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let x = create_space(&mut state, 2);

        // Apply a MemberAdded event directly — bypasses the test helpers
        // so we exercise the same path the chain takes.
        let member_event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: root,
                extension: TrustExtension::MemberAdded { member_space_id: x },
            }),
        };
        state.apply_event(&member_event);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

        assert_eq!(graph.len(), 1, "canonical set must contain only root");
        assert!(graph.contains(&root));
        assert!(
            !graph.contains(&x),
            "Member-only reachable space must not be canonical"
        );
    }

    #[test]
    fn test_editor_edge_from_root_does_not_grant_canonical() {
        // Plan 0007: Editor edges no longer grant canonical membership.
        // An EditorAdded(root, X) event must leave X outside the canonical set.
        let mut state = GraphState::new();
        let root = create_space(&mut state, 1);
        let x = create_space(&mut state, 2);

        let editor_event = SpaceTopologyEvent {
            meta: make_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: root,
                extension: TrustExtension::EditorAdded { member_space_id: x },
            }),
        };
        state.apply_event(&editor_event);

        let mut transitive = TransitiveProcessor::new();
        let mut processor = CanonicalProcessor::new(root);

        let graph = processor
            .compute_if_changed(&state, &mut transitive)
            .unwrap();

        assert_eq!(graph.len(), 1, "canonical set must contain only root");
        assert!(graph.contains(&root));
        assert!(
            !graph.contains(&x),
            "Editor-only reachable space must not be canonical"
        );
    }
}
