//! Graph diff computation for RFC 0002
//!
//! Computes incremental diffs between canonical graph states using ADDED/REMOVED/MOVED
//! semantics. The DiffTracker stores positions (not full graphs) to enable efficient
//! diff computation.

use super::{CanonicalGraph, EdgeType, TreeNode};
use crate::events::{SpaceId, TopicId};
use std::collections::HashMap;

/// A position in the canonical tree.
/// Stores the minimal information needed to detect changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    /// Distance from root (0 = root itself, which is not included in diffs)
    pub distance: u32,
    /// Parent node's space_id
    pub parent: SpaceId,
    /// Type of edge connecting to parent
    pub edge_type: EdgeType,
    /// Topic ID if this is a topic edge
    pub topic_id: Option<TopicId>,
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
/// Stores only positions (not full graphs) for memory efficiency.
/// Separated from CanonicalProcessor to maintain single responsibility.
#[derive(Debug, Default)]
pub struct DiffTracker {
    /// Previous position map (None = first computation, emit all as ADDED)
    last_positions: Option<HashMap<SpaceId, Position>>,
}

impl DiffTracker {
    /// Create a new diff tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Track a new graph and compute diff from previous state.
    ///
    /// On first call (bootstrap), returns a diff with all nodes as ADDED.
    /// On subsequent calls, returns changes between previous and current state.
    pub fn track(&mut self, graph: &CanonicalGraph) -> GraphDiff {
        let new_positions = build_position_map(&graph.tree);
        let diff = compute_diff(self.last_positions.as_ref(), &new_positions);
        self.last_positions = Some(new_positions);
        diff
    }

    /// Reset tracker state (useful for testing or reinitialization)
    pub fn reset(&mut self) {
        self.last_positions = None;
    }
}

/// Build a map of space_id -> Position from tree traversal
fn build_position_map(tree: &TreeNode) -> HashMap<SpaceId, Position> {
    let mut map = HashMap::new();
    // Root is at distance 0, but we use its own space_id as "parent" for consistency
    build_position_map_recursive(tree, &mut map, 0, tree.space_id);
    map
}

fn build_position_map_recursive(
    node: &TreeNode,
    map: &mut HashMap<SpaceId, Position>,
    distance: u32,
    parent: SpaceId,
) {
    // Don't include root in diff (it's implicit and never changes)
    if distance > 0 {
        map.insert(
            node.space_id,
            Position {
                distance,
                parent,
                edge_type: node.edge_type,
                topic_id: node.topic_id,
            },
        );
    }

    for child in &node.children {
        build_position_map_recursive(child, map, distance + 1, node.space_id);
    }
}

/// Compute diff between old and new position maps using sorted vector merge.
///
/// Returns changes sorted by space_id for deterministic output.
fn compute_diff(
    old: Option<&HashMap<SpaceId, Position>>,
    new: &HashMap<SpaceId, Position>,
) -> GraphDiff {
    let old_positions = old.cloned().unwrap_or_default();

    // Convert to sorted vectors for merge
    let mut old_vec: Vec<_> = old_positions.into_iter().collect();
    let mut new_vec: Vec<_> = new.iter().map(|(k, v)| (*k, *v)).collect();
    old_vec.sort_by_key(|(id, _)| *id);
    new_vec.sort_by_key(|(id, _)| *id);

    // Merge-join to find changes
    let mut changes = Vec::new();
    let mut old_iter = old_vec.into_iter().peekable();
    let mut new_iter = new_vec.into_iter().peekable();

    loop {
        match (old_iter.peek(), new_iter.peek()) {
            (None, None) => break,

            // Only in old -> REMOVED
            (Some(_), None) => {
                let (space_id, _) = old_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id,
                    change_type: ChangeType::Removed,
                    position: None,
                });
            }

            // Only in new -> ADDED
            (None, Some(_)) => {
                let (space_id, pos) = new_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id,
                    change_type: ChangeType::Added,
                    position: Some(pos),
                });
            }

            // In both -> check for MOVED
            (Some((old_id, _)), Some((new_id, _))) => match old_id.cmp(new_id) {
                std::cmp::Ordering::Less => {
                    // old_id < new_id: old_id was REMOVED
                    let (space_id, _) = old_iter.next().unwrap();
                    changes.push(NodeChange {
                        space_id,
                        change_type: ChangeType::Removed,
                        position: None,
                    });
                }
                std::cmp::Ordering::Greater => {
                    // old_id > new_id: new_id was ADDED
                    let (space_id, pos) = new_iter.next().unwrap();
                    changes.push(NodeChange {
                        space_id,
                        change_type: ChangeType::Added,
                        position: Some(pos),
                    });
                }
                std::cmp::Ordering::Equal => {
                    // Same space_id: check if position changed (MOVED)
                    let (space_id, old_pos) = old_iter.next().unwrap();
                    let (_, new_pos) = new_iter.next().unwrap();
                    if old_pos != new_pos {
                        changes.push(NodeChange {
                            space_id,
                            change_type: ChangeType::Moved,
                            position: Some(new_pos),
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

    /// Create a simple tree: root -> A -> B
    fn make_simple_tree() -> TreeNode {
        let mut root = TreeNode::new_root(make_space_id(0x01));
        let mut a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        let b = TreeNode::new(make_space_id(0x0B), EdgeType::Verified);
        a.add_child(b);
        root.add_child(a);
        root
    }

    #[test]
    fn test_build_position_map_excludes_root() {
        let tree = make_simple_tree();
        let map = build_position_map(&tree);

        // Root (0x01) should not be in the map
        assert!(!map.contains_key(&make_space_id(0x01)));
        // A and B should be in the map
        assert!(map.contains_key(&make_space_id(0x0A)));
        assert!(map.contains_key(&make_space_id(0x0B)));
    }

    #[test]
    fn test_build_position_map_distances() {
        let tree = make_simple_tree();
        let map = build_position_map(&tree);

        let pos_a = map.get(&make_space_id(0x0A)).unwrap();
        assert_eq!(pos_a.distance, 1);
        assert_eq!(pos_a.parent, make_space_id(0x01)); // root
        assert_eq!(pos_a.edge_type, EdgeType::Verified);

        let pos_b = map.get(&make_space_id(0x0B)).unwrap();
        assert_eq!(pos_b.distance, 2);
        assert_eq!(pos_b.parent, make_space_id(0x0A));
        assert_eq!(pos_b.edge_type, EdgeType::Verified);
    }

    #[test]
    fn test_diff_bootstrap_all_added() {
        let tree = make_simple_tree();
        let new_positions = build_position_map(&tree);
        let diff = compute_diff(None, &new_positions);

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
        let positions1 = build_position_map(&tree1);

        // Add node C under A
        let mut tree2 = make_simple_tree();
        let c = TreeNode::new(make_space_id(0x0C), EdgeType::Related);
        tree2.children[0].add_child(c); // Add C under A
        let positions2 = build_position_map(&tree2);

        let diff = compute_diff(Some(&positions1), &positions2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].space_id, make_space_id(0x0C));
        assert_eq!(diff.changes[0].change_type, ChangeType::Added);
    }

    #[test]
    fn test_diff_node_removed() {
        let tree1 = make_simple_tree();
        let positions1 = build_position_map(&tree1);

        // Remove B (just root -> A)
        let mut tree2 = TreeNode::new_root(make_space_id(0x01));
        let a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        tree2.add_child(a);
        let positions2 = build_position_map(&tree2);

        let diff = compute_diff(Some(&positions1), &positions2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].space_id, make_space_id(0x0B));
        assert_eq!(diff.changes[0].change_type, ChangeType::Removed);
        assert!(diff.changes[0].position.is_none());
    }

    #[test]
    fn test_diff_node_moved_different_parent() {
        // Tree1: root -> A -> B
        let tree1 = make_simple_tree();
        let positions1 = build_position_map(&tree1);

        // Tree2: root -> A, root -> B (B moved from A to root)
        let mut tree2 = TreeNode::new_root(make_space_id(0x01));
        let a = TreeNode::new(make_space_id(0x0A), EdgeType::Verified);
        let b = TreeNode::new(make_space_id(0x0B), EdgeType::Verified);
        tree2.add_child(a);
        tree2.add_child(b);
        let positions2 = build_position_map(&tree2);

        let diff = compute_diff(Some(&positions1), &positions2);
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
        let positions1 = build_position_map(&tree1);

        // Tree2: root -related-> A (same parent, different edge type)
        let mut tree2 = TreeNode::new_root(make_space_id(0x01));
        let a2 = TreeNode::new(make_space_id(0x0A), EdgeType::Related);
        tree2.add_child(a2);
        let positions2 = build_position_map(&tree2);

        let diff = compute_diff(Some(&positions1), &positions2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].change_type, ChangeType::Moved);
        assert_eq!(
            diff.changes[0].position.unwrap().edge_type,
            EdgeType::Related
        );
    }

    #[test]
    fn test_diff_sorted_by_space_id() {
        // Ensure output is deterministic regardless of HashMap iteration order
        let mut tree = TreeNode::new_root(make_space_id(0x01));
        // Add nodes in non-sorted order
        tree.add_child(TreeNode::new(make_space_id(0x0C), EdgeType::Verified));
        tree.add_child(TreeNode::new(make_space_id(0x0A), EdgeType::Verified));
        tree.add_child(TreeNode::new(make_space_id(0x0B), EdgeType::Verified));

        let positions = build_position_map(&tree);
        let diff = compute_diff(None, &positions);

        // Changes should be sorted by space_id
        let space_ids: Vec<_> = diff.changes.iter().map(|c| c.space_id[15]).collect();
        assert_eq!(space_ids, vec![0x0A, 0x0B, 0x0C]);
    }

    #[test]
    fn test_topic_edge_position() {
        let mut tree = TreeNode::new_root(make_space_id(0x01));
        let topic_node = TreeNode::new_with_topic(make_space_id(0x0A), make_topic_id(0x8A));
        tree.add_child(topic_node);

        let positions = build_position_map(&tree);
        let pos = positions.get(&make_space_id(0x0A)).unwrap();

        assert_eq!(pos.edge_type, EdgeType::Topic);
        assert_eq!(pos.topic_id, Some(make_topic_id(0x8A)));
    }
}
