//! Graph diff computation for RFC 0002
//!
//! Computes incremental diffs between canonical graph states using ADDED/REMOVED/MOVED
//! semantics. The DiffTracker stores positions as sorted vectors for efficient
//! diff computation via merge-join.
//!
//! ## Performance Characteristics
//!
//! - **Time complexity**: O(n log n) per diff (dominated by sort)
//! - **Space complexity**: O(n) for position storage
//! - **Allocations**: Near-zero after warmup (buffers are reused)
//! - **Cache locality**: Excellent (contiguous sorted vectors for merge scan)

use super::{CanonicalGraph, EdgeType, TreeNode};
use crate::events::SpaceId;

/// A position in the canonical tree.
/// Stores the minimal information needed to detect changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    /// Distance from root (0 = root itself, which is not included in diffs)
    pub distance: u32,
    /// Parent node's space_id
    pub parent: SpaceId,
    /// Type of edge connecting to parent (includes topic_id for Topic edges)
    pub edge_type: EdgeType,
}

/// A single change in a graph diff
#[derive(Debug, Clone)]
pub struct NodeChange {
    /// The space that changed
    pub space_id: SpaceId,
    /// Type of change
    pub change_type: ChangeType,
    /// Position in the new tree (present for ADDED/MOVED)
    pub position: Option<Position>,
}

/// Type of change
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    /// Node was added to the canonical graph
    Added,
    /// Node was removed from the canonical graph
    Removed,
    /// Node moved to a different position (different parent, edge type, or distance)
    Moved,
}

/// Diff between two graph states
#[derive(Debug, Clone, Default)]
pub struct GraphDiff {
    /// List of changes, sorted by space_id for determinism
    pub changes: Vec<NodeChange>,
}

impl GraphDiff {
    /// Create an empty diff
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the diff has no changes
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Number of changes in the diff
    pub fn len(&self) -> usize {
        self.changes.len()
    }
}

/// Tracks previous graph state and computes diffs.
///
/// Uses sorted vectors instead of HashMaps for better performance:
/// - No hashing overhead on insert
/// - No clone needed for diff computation (iterate borrowed slices)
/// - Better cache locality during merge-join scan
/// - Reuses allocations across calls (near-zero allocation after warmup)
#[derive(Debug, Default)]
pub struct DiffTracker {
    /// Previous positions as a sorted Vec (sorted by SpaceId)
    last_positions: Vec<(SpaceId, Position)>,
    /// Scratch buffer for building new positions (avoids allocation)
    scratch: Vec<(SpaceId, Position)>,
    /// Whether we've seen the first graph (for bootstrap detection)
    initialized: bool,
}

impl DiffTracker {
    /// Create a new diff tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new diff tracker with pre-allocated capacity
    ///
    /// Use this when you know the approximate number of nodes to avoid
    /// reallocations during the first few calls.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            last_positions: Vec::with_capacity(capacity),
            scratch: Vec::with_capacity(capacity),
            initialized: false,
        }
    }

    /// Track a new graph and compute diff from previous state.
    ///
    /// On first call (bootstrap), returns a diff with all nodes as ADDED.
    /// On subsequent calls, returns changes between previous and current state.
    ///
    /// When a SpaceId appears at multiple positions in the tree (e.g., via
    /// both explicit and topic edges), only the position closest to the root
    /// is tracked. This matches the spec's "first path wins" semantics.
    ///
    /// ## Performance
    ///
    /// After the first call, this method performs near-zero heap allocations
    /// (only the `changes` Vec in the returned diff allocates, and only when
    /// there are actual changes).
    pub fn track(&mut self, graph: &CanonicalGraph) -> GraphDiff {
        // Reuse scratch buffer - clear but keep capacity
        self.scratch.clear();
        build_position_vec_into(&graph.tree, &mut self.scratch);

        // Sort by (SpaceId, distance) so that for duplicates, the shortest
        // distance comes first. This ensures "closest to root wins" when a
        // node appears multiple times in the tree (e.g., via both explicit
        // and topic edges).
        self.scratch
            .sort_unstable_by(|(id_a, pos_a), (id_b, pos_b)| {
                id_a.cmp(id_b)
                    .then_with(|| pos_a.distance.cmp(&pos_b.distance))
            });

        // Dedup by SpaceId, keeping the first entry (shortest distance due to sort above).
        // The merge-join diff requires unique SpaceIds.
        self.scratch.dedup_by_key(|(id, _)| *id);

        let diff = if self.initialized {
            compute_diff(&self.last_positions, &self.scratch)
        } else {
            self.initialized = true;
            // Bootstrap: all nodes are ADDED
            compute_diff(&[], &self.scratch)
        };

        // Swap buffers instead of allocating - last_positions gets the new data,
        // scratch gets the old capacity for reuse next call
        std::mem::swap(&mut self.last_positions, &mut self.scratch);

        diff
    }

    /// Reset tracker state (useful for testing or reinitialization)
    pub fn reset(&mut self) {
        self.last_positions.clear();
        self.scratch.clear();
        self.initialized = false;
    }

    /// Returns the number of positions currently tracked
    pub fn position_count(&self) -> usize {
        self.last_positions.len()
    }
}

/// Build position vec by traversing tree and appending to existing vec
fn build_position_vec_into(tree: &TreeNode, vec: &mut Vec<(SpaceId, Position)>) {
    // Root is at distance 0, but we use its own space_id as "parent" for consistency
    build_position_vec_recursive(tree, vec, 0, tree.space_id);
}

fn build_position_vec_recursive(
    node: &TreeNode,
    vec: &mut Vec<(SpaceId, Position)>,
    distance: u32,
    parent: SpaceId,
) {
    // Don't include root in diff (it's implicit and never changes)
    if distance > 0 {
        vec.push((
            node.space_id,
            Position {
                distance,
                parent,
                edge_type: node.edge_type,
            },
        ));
    }

    for child in &node.children {
        build_position_vec_recursive(child, vec, distance + 1, node.space_id);
    }
}

/// Compute diff between old and new position slices using merge-join.
///
/// Both slices must be sorted by SpaceId. Returns changes sorted by space_id
/// for deterministic output.
///
/// ## Performance
///
/// - Time: O(n) where n = max(old.len(), new.len())
/// - Space: O(changes) - only allocates for actual changes
/// - No cloning of input data
fn compute_diff(old: &[(SpaceId, Position)], new: &[(SpaceId, Position)]) -> GraphDiff {
    let mut changes = Vec::new();
    let mut old_iter = old.iter().peekable();
    let mut new_iter = new.iter().peekable();

    loop {
        match (old_iter.peek(), new_iter.peek()) {
            (None, None) => break,

            // Only in old -> REMOVED
            (Some(_), None) => {
                let (space_id, _) = old_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id: *space_id,
                    change_type: ChangeType::Removed,
                    position: None,
                });
            }

            // Only in new -> ADDED
            (None, Some(_)) => {
                let (space_id, pos) = new_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id: *space_id,
                    change_type: ChangeType::Added,
                    position: Some(*pos),
                });
            }

            // In both -> check for MOVED
            (Some((old_id, _)), Some((new_id, _))) => match old_id.cmp(new_id) {
                std::cmp::Ordering::Less => {
                    // old_id < new_id: old_id was REMOVED
                    let (space_id, _) = old_iter.next().unwrap();
                    changes.push(NodeChange {
                        space_id: *space_id,
                        change_type: ChangeType::Removed,
                        position: None,
                    });
                }
                std::cmp::Ordering::Greater => {
                    // old_id > new_id: new_id was ADDED
                    let (space_id, pos) = new_iter.next().unwrap();
                    changes.push(NodeChange {
                        space_id: *space_id,
                        change_type: ChangeType::Added,
                        position: Some(*pos),
                    });
                }
                std::cmp::Ordering::Equal => {
                    // Same space_id: check if position changed (MOVED)
                    let (space_id, old_pos) = old_iter.next().unwrap();
                    let (_, new_pos) = new_iter.next().unwrap();
                    if old_pos != new_pos {
                        changes.push(NodeChange {
                            space_id: *space_id,
                            change_type: ChangeType::Moved,
                            position: Some(*new_pos),
                        });
                    }
                    // If positions are equal, no change to report
                }
            },
        }
    }

    GraphDiff { changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{make_space_id, make_topic_id};

    /// Create a simple tree: root -> A -> B
    fn make_simple_tree() -> TreeNode {
        let mut root = TreeNode::new_root(make_space_id(0x01));
        let mut a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        let b = TreeNode::new(make_space_id(0x0B), EdgeType::Verified);
        a.add_child(b);
        root.add_child(a);
        root
    }

    /// Helper to build a sorted position vec from a tree (for tests)
    fn build_position_vec(tree: &TreeNode) -> Vec<(SpaceId, Position)> {
        let mut vec = Vec::new();
        build_position_vec_into(tree, &mut vec);
        vec.sort_unstable_by_key(|(id, _)| *id);
        vec
    }

    #[test]
    fn test_build_position_vec_excludes_root() {
        let tree = make_simple_tree();
        let vec = build_position_vec(&tree);

        // Root (0x01) should not be in the vec
        assert!(!vec.iter().any(|(id, _)| *id == make_space_id(0x01)));
        // A and B should be in the vec
        assert!(vec.iter().any(|(id, _)| *id == make_space_id(0x0A)));
        assert!(vec.iter().any(|(id, _)| *id == make_space_id(0x0B)));
    }

    #[test]
    fn test_build_position_vec_distances() {
        let tree = make_simple_tree();
        let vec = build_position_vec(&tree);

        let pos_a = vec
            .iter()
            .find(|(id, _)| *id == make_space_id(0x0A))
            .map(|(_, p)| p)
            .unwrap();
        assert_eq!(pos_a.distance, 1);
        assert_eq!(pos_a.parent, make_space_id(0x01)); // root
        assert_eq!(pos_a.edge_type, EdgeType::Verified);

        let pos_b = vec
            .iter()
            .find(|(id, _)| *id == make_space_id(0x0B))
            .map(|(_, p)| p)
            .unwrap();
        assert_eq!(pos_b.distance, 2);
        assert_eq!(pos_b.parent, make_space_id(0x0A));
        assert_eq!(pos_b.edge_type, EdgeType::Verified);
    }

    #[test]
    fn test_diff_bootstrap_all_added() {
        let tree = make_simple_tree();
        let new_positions = build_position_vec(&tree);
        let diff = compute_diff(&[], &new_positions);

        // Bootstrap: all nodes should be ADDED
        assert_eq!(diff.len(), 2); // A and B (root excluded)

        let added: Vec<_> = diff
            .changes
            .iter()
            .filter(|c| c.change_type == ChangeType::Added)
            .collect();
        assert_eq!(added.len(), 2);
    }

    #[test]
    fn test_diff_tracker_bootstrap() {
        let tree = make_simple_tree();
        let graph = CanonicalGraph::new(
            make_space_id(0x01),
            tree,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0B),
            ]
            .into_iter()
            .collect(),
        );

        let mut tracker = DiffTracker::new();
        let diff = tracker.track(&graph);

        // First track: all ADDED
        assert_eq!(diff.len(), 2);
        assert!(diff
            .changes
            .iter()
            .all(|c| c.change_type == ChangeType::Added));
    }

    #[test]
    fn test_diff_tracker_subsequent_no_change() {
        let tree = make_simple_tree();
        let graph = CanonicalGraph::new(
            make_space_id(0x01),
            tree,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0B),
            ]
            .into_iter()
            .collect(),
        );

        let mut tracker = DiffTracker::new();
        let _ = tracker.track(&graph.clone());
        let diff = tracker.track(&graph);

        // Same graph: no changes
        assert!(diff.is_empty());
    }

    #[test]
    fn test_diff_node_added() {
        let tree1 = make_simple_tree();
        let positions1 = build_position_vec(&tree1);

        // Add node C under A
        let mut tree2 = make_simple_tree();
        let c = TreeNode::new(make_space_id(0x0C), EdgeType::Related);
        tree2.children[0].add_child(c); // Add C under A
        let positions2 = build_position_vec(&tree2);

        let diff = compute_diff(&positions1, &positions2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].space_id, make_space_id(0x0C));
        assert_eq!(diff.changes[0].change_type, ChangeType::Added);
    }

    #[test]
    fn test_diff_node_removed() {
        let tree1 = make_simple_tree();
        let positions1 = build_position_vec(&tree1);

        // Remove B (just root -> A)
        let mut tree2 = TreeNode::new_root(make_space_id(0x01));
        let a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        tree2.add_child(a);
        let positions2 = build_position_vec(&tree2);

        let diff = compute_diff(&positions1, &positions2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].space_id, make_space_id(0x0B));
        assert_eq!(diff.changes[0].change_type, ChangeType::Removed);
        assert!(diff.changes[0].position.is_none());
    }

    #[test]
    fn test_diff_node_moved_different_parent() {
        // Tree1: root -> A -> B
        let tree1 = make_simple_tree();
        let positions1 = build_position_vec(&tree1);

        // Tree2: root -> A, root -> B (B moved from A to root)
        let mut tree2 = TreeNode::new_root(make_space_id(0x01));
        let a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        let b = TreeNode::new(make_space_id(0x0B), EdgeType::Verified);
        tree2.add_child(a);
        tree2.add_child(b);
        let positions2 = build_position_vec(&tree2);

        let diff = compute_diff(&positions1, &positions2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].space_id, make_space_id(0x0B));
        assert_eq!(diff.changes[0].change_type, ChangeType::Moved);

        let new_pos = diff.changes[0].position.unwrap();
        assert_eq!(new_pos.distance, 1); // Now directly under root
        assert_eq!(new_pos.parent, make_space_id(0x01)); // Root is parent
    }

    #[test]
    fn test_diff_node_moved_different_edge_type() {
        // Tree1: root -verified-> A
        let mut tree1 = TreeNode::new_root(make_space_id(0x01));
        let a1 = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        tree1.add_child(a1);
        let positions1 = build_position_vec(&tree1);

        // Tree2: root -related-> A (same parent, different edge type)
        let mut tree2 = TreeNode::new_root(make_space_id(0x01));
        let a2 = TreeNode::new(make_space_id(0x0A), EdgeType::Related);
        tree2.add_child(a2);
        let positions2 = build_position_vec(&tree2);

        let diff = compute_diff(&positions1, &positions2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].change_type, ChangeType::Moved);
        assert_eq!(
            diff.changes[0].position.unwrap().edge_type,
            EdgeType::Related
        );
    }

    #[test]
    fn test_diff_sorted_by_space_id() {
        // Ensure output is deterministic regardless of tree traversal order
        let mut tree = TreeNode::new_root(make_space_id(0x01));
        // Add nodes in non-sorted order
        tree.add_child(TreeNode::new(make_space_id(0x0C), EdgeType::Verified));
        tree.add_child(TreeNode::new(make_space_id(0x0A), EdgeType::Verified));
        tree.add_child(TreeNode::new(make_space_id(0x0B), EdgeType::Verified));

        let positions = build_position_vec(&tree);
        let diff = compute_diff(&[], &positions);

        // Changes should be sorted by space_id
        let space_ids: Vec<_> = diff.changes.iter().map(|c| c.space_id[15]).collect();
        assert_eq!(space_ids, vec![0x0A, 0x0B, 0x0C]);
    }

    #[test]
    fn test_topic_edge_position() {
        let mut tree = TreeNode::new_root(make_space_id(0x01));
        let topic_node = TreeNode::new_with_topic(make_space_id(0x0A), make_topic_id(0x8A));
        tree.add_child(topic_node);

        let positions = build_position_vec(&tree);
        let pos = positions
            .iter()
            .find(|(id, _)| *id == make_space_id(0x0A))
            .map(|(_, p)| p)
            .unwrap();

        assert_eq!(
            pos.edge_type,
            EdgeType::Topic {
                topic_id: make_topic_id(0x8A)
            }
        );
    }

    #[test]
    fn test_diff_tracker_with_capacity() {
        let tracker = DiffTracker::with_capacity(1000);
        assert_eq!(tracker.position_count(), 0);
        // Capacity is pre-allocated but not observable without unsafe
    }

    #[test]
    fn test_diff_tracker_reset() {
        let tree = make_simple_tree();
        let graph = CanonicalGraph::new(
            make_space_id(0x01),
            tree,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0B),
            ]
            .into_iter()
            .collect(),
        );

        let mut tracker = DiffTracker::new();
        let _ = tracker.track(&graph);
        assert_eq!(tracker.position_count(), 2);

        tracker.reset();
        assert_eq!(tracker.position_count(), 0);

        // After reset, next track should be a bootstrap (all ADDED)
        let diff = tracker.track(&graph);
        assert_eq!(diff.len(), 2);
        assert!(diff
            .changes
            .iter()
            .all(|c| c.change_type == ChangeType::Added));
    }

    #[test]
    fn test_allocation_reuse() {
        // This test verifies the buffer swap logic works correctly
        let mut tracker = DiffTracker::new();

        // First graph: A, B
        let tree1 = make_simple_tree();
        let graph1 = CanonicalGraph::new(
            make_space_id(0x01),
            tree1,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0B),
            ]
            .into_iter()
            .collect(),
        );
        let _ = tracker.track(&graph1);

        // Second graph: A, B, C (add C)
        let mut tree2 = make_simple_tree();
        tree2.add_child(TreeNode::new(make_space_id(0x0C), EdgeType::Verified));
        let graph2 = CanonicalGraph::new(
            make_space_id(0x01),
            tree2,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0B),
                make_space_id(0x0C),
            ]
            .into_iter()
            .collect(),
        );
        let diff = tracker.track(&graph2);

        // Should detect C was added
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].space_id, make_space_id(0x0C));
        assert_eq!(diff.changes[0].change_type, ChangeType::Added);

        // Third graph: same as second (no changes)
        let diff = tracker.track(&graph2);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_duplicate_spaceid_keeps_shortest_distance() {
        // Tree where B appears twice:
        //   Root -> A (explicit, distance 1)
        //     A -> B (explicit, distance 2)
        //   Root -> B (topic, distance 1)
        //
        // B should be emitted once at distance 1 (the topic path, closer to root)
        let mut root = TreeNode::new_root(make_space_id(0x01));
        let mut a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        let b_explicit = TreeNode::new(make_space_id(0x0B), EdgeType::Verified);
        a.add_child(b_explicit);
        root.add_child(a);
        // B again via topic edge, directly under root (distance 1)
        let b_topic = TreeNode::new_with_topic(make_space_id(0x0B), make_topic_id(0x8B));
        root.add_child(b_topic);

        let graph = CanonicalGraph::new(
            make_space_id(0x01),
            root,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0B),
            ]
            .into_iter()
            .collect(),
        );

        let mut tracker = DiffTracker::new();
        let diff = tracker.track(&graph);

        // A and B should each appear exactly once
        assert_eq!(diff.len(), 2);

        let b_change = diff
            .changes
            .iter()
            .find(|c| c.space_id == make_space_id(0x0B))
            .expect("B should be in diff");
        assert_eq!(b_change.change_type, ChangeType::Added);

        // B's position should be at distance 1 (topic path, closer to root)
        let pos = b_change.position.unwrap();
        assert_eq!(pos.distance, 1);
        assert_eq!(pos.parent, make_space_id(0x01)); // root is parent
        assert_eq!(
            pos.edge_type,
            EdgeType::Topic {
                topic_id: make_topic_id(0x8B)
            }
        );

        // Tracker should only track 2 positions (A and B, not 3)
        assert_eq!(tracker.position_count(), 2);
    }

    #[test]
    fn test_duplicate_spaceid_explicit_closer_than_topic() {
        // Tree where B appears twice, but explicit is closer:
        //   Root -> B (explicit, distance 1)
        //   Root -> A (explicit, distance 1)
        //     A -> B (topic, distance 2)
        //
        // B should be emitted once at distance 1 (the explicit path)
        let mut root = TreeNode::new_root(make_space_id(0x01));
        let b_explicit = TreeNode::new(make_space_id(0x0B), EdgeType::Verified);
        root.add_child(b_explicit);
        let mut a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        let b_topic = TreeNode::new_with_topic(make_space_id(0x0B), make_topic_id(0x8B));
        a.add_child(b_topic);
        root.add_child(a);

        let graph = CanonicalGraph::new(
            make_space_id(0x01),
            root,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0B),
            ]
            .into_iter()
            .collect(),
        );

        let mut tracker = DiffTracker::new();
        let diff = tracker.track(&graph);

        assert_eq!(diff.len(), 2);

        let b_change = diff
            .changes
            .iter()
            .find(|c| c.space_id == make_space_id(0x0B))
            .expect("B should be in diff");

        // B's position should be at distance 1 (explicit path, closer to root)
        let pos = b_change.position.unwrap();
        assert_eq!(pos.distance, 1);
        assert_eq!(pos.parent, make_space_id(0x01));
        assert_eq!(pos.edge_type, EdgeType::Verified);

        assert_eq!(tracker.position_count(), 2);
    }
}
