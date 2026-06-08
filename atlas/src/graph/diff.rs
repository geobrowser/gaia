//! Graph diff computation for RFC 0002
//!
//! Computes incremental diffs between canonical graph states using ADDED/REMOVED/MOVED
//! semantics. The DiffTracker stores positions as sorted vectors for efficient
//! diff computation via merge-join.
//!
//! Important implementation semantics:
//! - The root node is implicit protocol state and is never emitted as a diff change.
//! - If a SpaceId appears multiple times in the tree, one position is chosen for
//!   diffing: the shortest distance from root (closest-to-root wins).
//! - Output is deterministic: changes are sorted by SpaceId.
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
    /// Set by [`DiffTracker::from_baseline`] and consumed by the first
    /// [`DiffTracker::track`] call. Carries the persisted emission state
    /// (`(space_id, distance, parent)` — no `EdgeType`, by design) so
    /// the first diff after restart can be computed against what we last
    /// told consumers rather than against an empty bootstrap baseline.
    ///
    /// Sorted by `space_id`, unique by `space_id` (closest-to-root wins on
    /// duplicates — same rule [`track`] applies to in-memory positions).
    pending_baseline: Option<Vec<(SpaceId, BaselinePos)>>,
}

/// Subset of [`Position`] that survives schema bumps. Used internally for the
/// one-shot diff against a restored baseline; not exposed in the public diff
/// output. Notably omits `edge_type` — see [`PersistedEmissionBaseline`].
///
/// [`PersistedEmissionBaseline`]: crate::persistence::PersistedEmissionBaseline
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BaselinePos {
    distance: u32,
    parent: SpaceId,
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
            pending_baseline: None,
        }
    }

    /// Prime the tracker from a persisted emission baseline.
    ///
    /// Used on startup so the first `track()` call produces the correct diff
    /// against what was last emitted to consumers — typically across a deploy
    /// that changes canonical rules and discards `GraphState`. The baseline
    /// carries `(space_id, distance, parent)` only; `edge_type` is *not*
    /// compared in the first diff, because the persisted shape intentionally
    /// drops it to remain schema-stable (see
    /// [`PersistedEmissionBaseline`]).
    ///
    /// Implications for the first track:
    /// - REMOVED is emitted for any space in the baseline but not in the new
    ///   canonical graph.
    /// - MOVED is emitted only when `distance` or `parent` changes (an
    ///   `edge_type`-only change is not enough to qualify, since we do not
    ///   know the old `edge_type` — assuming "unchanged" is the conservative
    ///   choice that minimises spurious events).
    /// - ADDED is emitted for any space in the new graph but not in the
    ///   baseline.
    ///
    /// Subsequent `track()` calls behave normally (full `Position` equality).
    ///
    /// [`PersistedEmissionBaseline`]: crate::persistence::PersistedEmissionBaseline
    pub fn from_baseline(baseline: &crate::persistence::PersistedEmissionBaseline) -> Self {
        let mut entries: Vec<(SpaceId, BaselinePos)> = baseline
            .nodes()
            .iter()
            .map(|n| {
                (
                    n.space_id,
                    BaselinePos {
                        distance: n.distance,
                        parent: n.parent,
                    },
                )
            })
            .collect();
        // `PersistedEmissionBaseline::from_nodes` already sorts and dedups,
        // but a hand-built baseline (or a future shape that doesn't enforce
        // the invariant) might not — so normalise defensively. The merge-join
        // diff below relies on unique-and-sorted-by-SpaceId on both sides.
        entries.sort_unstable_by(|(id_a, pos_a), (id_b, pos_b)| {
            id_a.cmp(id_b)
                .then_with(|| pos_a.distance.cmp(&pos_b.distance))
        });
        entries.dedup_by_key(|(id, _)| *id);

        let capacity = entries.len();
        Self {
            last_positions: Vec::with_capacity(capacity),
            scratch: Vec::with_capacity(capacity),
            // Treat the tracker as initialized so the next `track()` doesn't
            // fall into the bootstrap-all-ADDED branch.
            initialized: true,
            pending_baseline: Some(entries),
        }
    }

    /// Track a new graph and compute diff from previous state.
    ///
    /// On first call (bootstrap), returns a diff with all nodes as ADDED —
    /// unless the tracker was constructed via [`DiffTracker::from_baseline`],
    /// in which case the first call diffs against the restored baseline.
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

        let diff = if let Some(baseline) = self.pending_baseline.take() {
            // One-shot path: compare against the restored baseline, which has
            // no edge_type. After this call, last_positions carries the full
            // current Position (including edge_type), so subsequent diffs go
            // through the regular compute_diff path.
            compute_diff_against_baseline(&baseline, &self.scratch)
        } else if self.initialized {
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
        self.pending_baseline = None;
    }

    /// Returns the number of positions currently tracked
    pub fn position_count(&self) -> usize {
        self.last_positions.len()
    }

    /// Iterate the tracker's current emission state as
    /// `(space_id, distance, parent)` triples — the persistable shape of a
    /// [`PersistedEmissionBaseline`]. Order matches `last_positions`
    /// (sorted by `space_id`, unique).
    ///
    /// Returns `None` when nothing has been tracked yet *and* no baseline has
    /// been primed (i.e. there is no contract with consumers to persist).
    /// When a baseline has been primed but `track()` hasn't been called yet,
    /// returns the pending baseline so the force-write path on startup can
    /// re-persist what was already on disk without losing data.
    ///
    /// [`PersistedEmissionBaseline`]: crate::persistence::PersistedEmissionBaseline
    pub fn iter_emission_state(&self) -> Option<EmissionStateIter<'_>> {
        if let Some(baseline) = self.pending_baseline.as_deref() {
            Some(EmissionStateIter {
                inner: EmissionStateIterInner::Pending(baseline.iter()),
            })
        } else if self.initialized {
            Some(EmissionStateIter {
                inner: EmissionStateIterInner::Live(self.last_positions.iter()),
            })
        } else {
            None
        }
    }
}

/// Iterator over a [`DiffTracker`]'s current emission state. The internal
/// variant is intentionally opaque so the in-memory `BaselinePos` shape (and
/// `Position`'s `edge_type`) stay implementation details.
pub struct EmissionStateIter<'a> {
    inner: EmissionStateIterInner<'a>,
}

enum EmissionStateIterInner<'a> {
    Pending(std::slice::Iter<'a, (SpaceId, BaselinePos)>),
    Live(std::slice::Iter<'a, (SpaceId, Position)>),
}

impl<'a> Iterator for EmissionStateIter<'a> {
    type Item = (SpaceId, u32, SpaceId);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            EmissionStateIterInner::Pending(it) => {
                it.next().map(|(id, pos)| (*id, pos.distance, pos.parent))
            }
            EmissionStateIterInner::Live(it) => {
                it.next().map(|(id, pos)| (*id, pos.distance, pos.parent))
            }
        }
    }
}

/// Build position vec by traversing tree and appending to existing vec
fn build_position_vec_into(tree: &TreeNode, vec: &mut Vec<(SpaceId, Position)>) {
    // Stack items: (node, distance, parent_space_id)
    let mut stack: Vec<(&TreeNode, u32, SpaceId)> = vec![(tree, 0, tree.space_id)];

    while let Some((node, distance, parent)) = stack.pop() {
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

        // Push children in reverse so leftmost is processed first
        for child in node.children.iter().rev() {
            stack.push((child, distance + 1, node.space_id));
        }
    }
}

/// One-shot merge-join diff between a restored baseline (no `edge_type`) and
/// the current canonical positions. Used by [`DiffTracker::track`] on the
/// first call when the tracker was constructed with
/// [`DiffTracker::from_baseline`].
///
/// Differs from [`compute_diff`] in exactly one place: the MOVED check
/// compares only `distance` + `parent`, never `edge_type`. The persisted
/// baseline drops `edge_type` for schema stability, so we can't know what it
/// was previously — and "edge_type-only change" would otherwise be flagged
/// as MOVED for every restored node, which is exactly the spurious-event
/// storm we are trying to avoid.
fn compute_diff_against_baseline(
    old: &[(SpaceId, BaselinePos)],
    new: &[(SpaceId, Position)],
) -> GraphDiff {
    let mut changes = Vec::new();
    let mut old_iter = old.iter().peekable();
    let mut new_iter = new.iter().peekable();

    loop {
        match (old_iter.peek(), new_iter.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                let (space_id, _) = old_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id: *space_id,
                    change_type: ChangeType::Removed,
                    position: None,
                });
            }
            (None, Some(_)) => {
                let (space_id, pos) = new_iter.next().unwrap();
                changes.push(NodeChange {
                    space_id: *space_id,
                    change_type: ChangeType::Added,
                    position: Some(*pos),
                });
            }
            (Some((old_id, _)), Some((new_id, _))) => match old_id.cmp(new_id) {
                std::cmp::Ordering::Less => {
                    let (space_id, _) = old_iter.next().unwrap();
                    changes.push(NodeChange {
                        space_id: *space_id,
                        change_type: ChangeType::Removed,
                        position: None,
                    });
                }
                std::cmp::Ordering::Greater => {
                    let (space_id, pos) = new_iter.next().unwrap();
                    changes.push(NodeChange {
                        space_id: *space_id,
                        change_type: ChangeType::Added,
                        position: Some(*pos),
                    });
                }
                std::cmp::Ordering::Equal => {
                    let (space_id, old_pos) = old_iter.next().unwrap();
                    let (_, new_pos) = new_iter.next().unwrap();
                    if old_pos.distance != new_pos.distance || old_pos.parent != new_pos.parent {
                        changes.push(NodeChange {
                            space_id: *space_id,
                            change_type: ChangeType::Moved,
                            position: Some(*new_pos),
                        });
                    }
                }
            },
        }
    }

    GraphDiff { changes }
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

    // ------------------------------------------------------------------
    // from_baseline / pending_baseline behaviour (GEO-645)
    // ------------------------------------------------------------------

    use crate::persistence::{BaselineNode, PersistedEmissionBaseline};

    fn baseline_with(nodes: &[(u8, u32, u8)]) -> PersistedEmissionBaseline {
        PersistedEmissionBaseline::from_nodes(nodes.iter().map(|(s, d, p)| BaselineNode {
            space_id: make_space_id(*s),
            distance: *d,
            parent: make_space_id(*p),
        }))
    }

    fn one_node_graph(child: u8, edge: EdgeType) -> CanonicalGraph {
        let mut root = TreeNode::new_root(make_space_id(0x01));
        root.add_child(TreeNode::new(make_space_id(child), edge));
        CanonicalGraph::new(
            make_space_id(0x01),
            root,
            [make_space_id(0x01), make_space_id(child)]
                .into_iter()
                .collect(),
        )
    }

    #[test]
    fn from_baseline_first_track_emits_removed_for_orphans() {
        // Models the GEO-645 happy path: baseline contains a space that the
        // new rules no longer treat as canonical (e.g. previously reachable
        // only via a Member edge, which v2 removes). The first track() must
        // emit REMOVED for that orphan, not silently drop it.
        let baseline = baseline_with(&[
            (0x0A, 1, 0x01), // still canonical
            (0x0B, 1, 0x01), // orphaned in the new rules
        ]);
        let mut tracker = DiffTracker::from_baseline(&baseline);

        let graph = one_node_graph(0x0A, EdgeType::Verified);
        let diff = tracker.track(&graph);

        // Only B should appear in the diff, and as REMOVED.
        assert_eq!(diff.len(), 1);
        let removed = &diff.changes[0];
        assert_eq!(removed.space_id, make_space_id(0x0B));
        assert_eq!(removed.change_type, ChangeType::Removed);
        assert!(removed.position.is_none());
    }

    #[test]
    fn from_baseline_first_track_emits_added_for_new() {
        let baseline = baseline_with(&[(0x0A, 1, 0x01)]);
        let mut tracker = DiffTracker::from_baseline(&baseline);

        // Graph now has A + a brand-new C (was not in baseline).
        let mut root = TreeNode::new_root(make_space_id(0x01));
        root.add_child(TreeNode::new(make_space_id(0x0A), EdgeType::Verified));
        root.add_child(TreeNode::new(make_space_id(0x0C), EdgeType::Verified));
        let graph = CanonicalGraph::new(
            make_space_id(0x01),
            root,
            [
                make_space_id(0x01),
                make_space_id(0x0A),
                make_space_id(0x0C),
            ]
            .into_iter()
            .collect(),
        );

        let diff = tracker.track(&graph);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].space_id, make_space_id(0x0C));
        assert_eq!(diff.changes[0].change_type, ChangeType::Added);
    }

    #[test]
    fn from_baseline_emits_moved_only_on_distance_or_parent_change() {
        // Baseline says A is at distance 1, parent = root.
        let baseline = baseline_with(&[(0x0A, 1, 0x01)]);

        // Case 1: distance/parent unchanged, edge_type differs — must NOT
        // emit MOVED. The baseline has no edge_type by design (schema
        // stability), so we cannot know whether edge_type changed. The
        // conservative choice is to suppress the event.
        {
            let mut tracker = DiffTracker::from_baseline(&baseline);
            let graph = one_node_graph(0x0A, EdgeType::Related); // edge_type differs
            let diff = tracker.track(&graph);
            assert!(
                diff.is_empty(),
                "edge_type-only delta against a baseline must not produce MOVED, got: {:?}",
                diff.changes
            );
        }

        // Case 2: distance changes — MOVED.
        {
            let mut tracker = DiffTracker::from_baseline(&baseline);
            // Build graph where A is now at distance 2 (root -> B -> A).
            let mut root = TreeNode::new_root(make_space_id(0x01));
            let mut b = TreeNode::new(make_space_id(0x0B), EdgeType::Verified);
            b.add_child(TreeNode::new(make_space_id(0x0A), EdgeType::Verified));
            root.add_child(b);
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
            let diff = tracker.track(&graph);
            let a_change = diff
                .changes
                .iter()
                .find(|c| c.space_id == make_space_id(0x0A))
                .expect("A must appear in diff after distance change");
            assert_eq!(a_change.change_type, ChangeType::Moved);
            assert_eq!(a_change.position.unwrap().distance, 2);
        }

        // Case 3: parent changes (distance same).
        {
            // Build graph where A is still distance 1 but parent is now B.
            // (Forced by making B the root via a sibling — simulate by having
            // A attached to a new parent with distance 1. To do this with
            // distance 1, the parent must be root, so this case overlaps
            // with the baseline. Instead simulate by changing the baseline
            // parent and showing parent change at the same distance.)
            let baseline2 = baseline_with(&[(0x0A, 1, 0x0B)]); // claim A had parent=B
            let mut tracker2 = DiffTracker::from_baseline(&baseline2);
            let graph = one_node_graph(0x0A, EdgeType::Verified); // actual parent = root (0x01)
            let diff = tracker2.track(&graph);
            let a_change = diff
                .changes
                .iter()
                .find(|c| c.space_id == make_space_id(0x0A))
                .expect("A must appear in diff after parent change");
            assert_eq!(a_change.change_type, ChangeType::Moved);
            assert_eq!(a_change.position.unwrap().parent, make_space_id(0x01));
        }
    }

    #[test]
    fn from_baseline_second_track_uses_full_position_equality() {
        // After the one-shot baseline-aware diff, subsequent track() calls
        // must include edge_type in MOVED detection (regular compute_diff
        // semantics). This guards against accidentally leaving the
        // edge_type-blind comparison in place forever.
        let baseline = baseline_with(&[(0x0A, 1, 0x01)]);
        let mut tracker = DiffTracker::from_baseline(&baseline);

        // First track: edge_type-only change is suppressed (as established
        // above).
        let graph_v1 = one_node_graph(0x0A, EdgeType::Verified);
        let diff = tracker.track(&graph_v1);
        assert!(diff.is_empty());

        // Second track: change edge_type only. compute_diff (not the
        // baseline variant) should now flag this as MOVED.
        let graph_v2 = one_node_graph(0x0A, EdgeType::Related);
        let diff = tracker.track(&graph_v2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changes[0].change_type, ChangeType::Moved);
        assert_eq!(
            diff.changes[0].position.unwrap().edge_type,
            EdgeType::Related
        );
    }

    #[test]
    fn iter_emission_state_returns_pending_baseline_before_first_track() {
        // Force-write-on-startup path: when a tracker is freshly primed but
        // no track() has run, iter_emission_state must still expose the
        // baseline so it can be re-persisted (idempotent rewrite).
        let baseline = baseline_with(&[(0x0A, 1, 0x01), (0x0B, 2, 0x0A)]);
        let tracker = DiffTracker::from_baseline(&baseline);
        let collected: Vec<_> = tracker.iter_emission_state().unwrap().collect();
        assert_eq!(collected.len(), 2);
        assert!(collected.contains(&(make_space_id(0x0A), 1, make_space_id(0x01))));
        assert!(collected.contains(&(make_space_id(0x0B), 2, make_space_id(0x0A))));
    }

    #[test]
    fn iter_emission_state_returns_none_when_uninitialized() {
        // A brand-new tracker (no baseline, no track) has no contract with
        // consumers to persist — iter_emission_state must signal this so the
        // caller doesn't write an empty baseline by accident.
        let tracker = DiffTracker::new();
        assert!(tracker.iter_emission_state().is_none());
    }

    #[test]
    fn iter_emission_state_reflects_live_positions_after_track() {
        let mut tracker = DiffTracker::new();
        let graph = one_node_graph(0x0A, EdgeType::Verified);
        let _ = tracker.track(&graph);

        let collected: Vec<_> = tracker.iter_emission_state().unwrap().collect();
        assert_eq!(
            collected,
            vec![(make_space_id(0x0A), 1, make_space_id(0x01))]
        );
    }

    #[test]
    fn from_baseline_no_op_when_baseline_matches_current() {
        // Sanity: priming from a baseline that exactly matches the next
        // canonical graph must produce zero events. This is the steady-state
        // restart case (Phase 1 restart, no rules change between deploys).
        let baseline = baseline_with(&[(0x0A, 1, 0x01)]);
        let mut tracker = DiffTracker::from_baseline(&baseline);

        let graph = one_node_graph(0x0A, EdgeType::Verified);
        let diff = tracker.track(&graph);
        assert!(
            diff.is_empty(),
            "baseline matching current must emit no events, got: {:?}",
            diff.changes
        );
    }
}
