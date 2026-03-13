//! In-memory canonical graph state.
//!
//! Thread-safe graph using `Arc<RwLock<CanonicalGraphInner>>` with `std::sync::RwLock`
//! (not tokio — lock is never held across `.await`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// Describes a change in canonical status for a space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalityChange {
    pub space_id: Uuid,
    /// `true` = space was ADDED to canonical graph, `false` = REMOVED
    pub in_canonical_graph: bool,
}

/// Type of change from a `CanonicalGraphDiff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Added,
    Removed,
    Moved,
}

/// A parsed node change from a diff.
#[derive(Debug, Clone)]
pub struct ParsedNodeChange {
    pub space_id: [u8; 16],
    pub change_type: ChangeType,
    pub distance: Option<u32>,
    pub parent_id: Option<[u8; 16]>,
}

/// Thread-safe canonical graph state.
#[derive(Debug, Clone)]
pub struct CanonicalGraphState {
    inner: Arc<RwLock<CanonicalGraphInner>>,
}

#[derive(Debug)]
struct CanonicalGraphInner {
    root_id: Option<[u8; 16]>,
    /// O(1) membership check
    canonical_spaces: HashSet<[u8; 16]>,
    /// Adjacency for BFS subspace traversal
    children: HashMap<[u8; 16], HashSet<[u8; 16]>>,
    /// For move/remove cleanup
    parents: HashMap<[u8; 16], [u8; 16]>,
    /// Distance from root per node
    distances: HashMap<[u8; 16], u32>,
}

impl CanonicalGraphState {
    /// Create a new empty canonical graph state.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(CanonicalGraphInner {
                root_id: None,
                canonical_spaces: HashSet::new(),
                children: HashMap::new(),
                parents: HashMap::new(),
                distances: HashMap::new(),
            })),
        }
    }

    /// Create from pre-loaded data (used by persistence layer on startup).
    pub fn from_snapshot(
        root_id: Option<[u8; 16]>,
        nodes: Vec<(
            /* space_id */ [u8; 16],
            /* parent_id */ [u8; 16],
            /* distance */ u32,
        )>,
    ) -> Self {
        let mut canonical_spaces = HashSet::with_capacity(nodes.len());
        let mut children: HashMap<[u8; 16], HashSet<[u8; 16]>> = HashMap::new();
        let mut parents = HashMap::with_capacity(nodes.len());
        let mut distances = HashMap::with_capacity(nodes.len());

        // Add root to canonical set
        if let Some(root) = root_id {
            canonical_spaces.insert(root);
            distances.insert(root, 0);
        }

        for (space_id, parent_id, distance) in nodes {
            canonical_spaces.insert(space_id);
            parents.insert(space_id, parent_id);
            distances.insert(space_id, distance);
            children.entry(parent_id).or_default().insert(space_id);
        }

        Self {
            inner: Arc::new(RwLock::new(CanonicalGraphInner {
                root_id,
                canonical_spaces,
                children,
                parents,
                distances,
            })),
        }
    }

    /// Get the root space ID, if set.
    pub fn root_id(&self) -> Option<Uuid> {
        let inner = self.inner.read().expect("lock poisoned");
        inner.root_id.map(Uuid::from_bytes)
    }

    /// Check if a space is in the canonical graph. O(1).
    pub fn is_canonical(&self, space_id: &[u8; 16]) -> bool {
        let inner = self.inner.read().expect("lock poisoned");
        inner.canonical_spaces.contains(space_id)
    }

    /// Get all subspaces (descendants + self) via BFS. O(subtree).
    /// Returns `None` if `space_id` is not in the canonical graph.
    pub fn get_subspaces(&self, space_id: &[u8; 16]) -> Option<Vec<Uuid>> {
        let inner = self.inner.read().expect("lock poisoned");
        if !inner.canonical_spaces.contains(space_id) {
            return None;
        }

        let mut result = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(*space_id);

        while let Some(current) = queue.pop_front() {
            result.push(Uuid::from_bytes(current));
            if let Some(kids) = inner.children.get(&current) {
                for child in kids {
                    queue.push_back(*child);
                }
            }
        }

        Some(result)
    }

    /// Get the distance from root for a space. O(1).
    pub fn get_distance(&self, space_id: &[u8; 16]) -> Option<u32> {
        let inner = self.inner.read().expect("lock poisoned");
        inner.distances.get(space_id).copied()
    }

    /// Apply a set of changes from a `CanonicalGraphDiff`.
    ///
    /// Returns a list of spaces whose canonical status flipped (ADDED/REMOVED).
    /// MOVED nodes do NOT produce a `CanonicalityChange` — they remain canonical
    /// but update parent/distance/children internally.
    pub fn apply_changes(
        &self,
        root_id: [u8; 16],
        changes: &[ParsedNodeChange],
    ) -> Vec<CanonicalityChange> {
        let mut inner = self.inner.write().expect("lock poisoned");
        let mut result = Vec::new();

        // Set root if not already set
        if inner.root_id.is_none() {
            inner.root_id = Some(root_id);
            if inner.canonical_spaces.insert(root_id) {
                inner.distances.insert(root_id, 0);
                result.push(CanonicalityChange {
                    space_id: Uuid::from_bytes(root_id),
                    in_canonical_graph: true,
                });
            }
        }

        for change in changes {
            match change.change_type {
                ChangeType::Added => {
                    let was_new = inner.canonical_spaces.insert(change.space_id);
                    if was_new {
                        // Set parent
                        if let Some(parent_id) = change.parent_id {
                            inner.parents.insert(change.space_id, parent_id);
                            inner
                                .children
                                .entry(parent_id)
                                .or_default()
                                .insert(change.space_id);
                        }
                        // Set distance
                        if let Some(distance) = change.distance {
                            inner.distances.insert(change.space_id, distance);
                        }
                        result.push(CanonicalityChange {
                            space_id: Uuid::from_bytes(change.space_id),
                            in_canonical_graph: true,
                        });
                    }
                }
                ChangeType::Removed => {
                    if inner.canonical_spaces.remove(&change.space_id) {
                        // Remove from old parent's children
                        if let Some(old_parent) = inner.parents.remove(&change.space_id) {
                            if let Some(siblings) = inner.children.get_mut(&old_parent) {
                                siblings.remove(&change.space_id);
                                if siblings.is_empty() {
                                    inner.children.remove(&old_parent);
                                }
                            }
                        }
                        // Clean up children map entry and distance
                        inner.children.remove(&change.space_id);
                        inner.distances.remove(&change.space_id);

                        result.push(CanonicalityChange {
                            space_id: Uuid::from_bytes(change.space_id),
                            in_canonical_graph: false,
                        });
                    }
                }
                ChangeType::Moved => {
                    // MOVED: node stays canonical, but parent/distance may change
                    if !inner.canonical_spaces.contains(&change.space_id) {
                        continue;
                    }
                    // Remove from old parent's children
                    if let Some(old_parent) = inner.parents.remove(&change.space_id) {
                        if let Some(siblings) = inner.children.get_mut(&old_parent) {
                            siblings.remove(&change.space_id);
                            if siblings.is_empty() {
                                inner.children.remove(&old_parent);
                            }
                        }
                    }
                    // Set new parent
                    if let Some(parent_id) = change.parent_id {
                        inner.parents.insert(change.space_id, parent_id);
                        inner
                            .children
                            .entry(parent_id)
                            .or_default()
                            .insert(change.space_id);
                    }
                    // Update distance
                    if let Some(distance) = change.distance {
                        inner.distances.insert(change.space_id, distance);
                    }
                    // No CanonicalityChange emitted for MOVED
                }
            }
        }

        result
    }

    /// Get a snapshot of the graph state for persistence.
    /// Returns (root_id, Vec<(space_id, parent_id, distance)>).
    #[allow(clippy::type_complexity)]
    pub fn snapshot(&self) -> (Option<[u8; 16]>, Vec<([u8; 16], [u8; 16], u32)>) {
        let inner = self.inner.read().expect("lock poisoned");
        let mut nodes = Vec::with_capacity(inner.canonical_spaces.len());

        for &space_id in &inner.canonical_spaces {
            // Skip root (it has no parent)
            if inner.root_id == Some(space_id) {
                continue;
            }
            let parent_id = inner.parents.get(&space_id).copied().unwrap_or([0u8; 16]);
            let distance = inner.distances.get(&space_id).copied().unwrap_or(0);
            nodes.push((space_id, parent_id, distance));
        }

        (inner.root_id, nodes)
    }

    /// Get the number of canonical spaces (for metrics/logging).
    pub fn len(&self) -> usize {
        let inner = self.inner.read().expect("lock poisoned");
        inner.canonical_spaces.len()
    }

    /// Check if the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for CanonicalGraphState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 16] {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    #[test]
    fn test_empty_state() {
        let state = CanonicalGraphState::new();
        assert!(state.is_empty());
        assert!(!state.is_canonical(&make_id(1)));
        assert!(state.get_subspaces(&make_id(1)).is_none());
        assert!(state.get_distance(&make_id(1)).is_none());
    }

    #[test]
    fn test_apply_added() {
        let state = CanonicalGraphState::new();
        let root = make_id(1);
        let child = make_id(2);

        let changes = vec![ParsedNodeChange {
            space_id: child,
            change_type: ChangeType::Added,
            distance: Some(1),
            parent_id: Some(root),
        }];

        let result = state.apply_changes(root, &changes);

        // Root gets added implicitly + child
        assert_eq!(result.len(), 2);
        assert!(result[0].in_canonical_graph); // root
        assert!(result[1].in_canonical_graph); // child

        assert!(state.is_canonical(&root));
        assert!(state.is_canonical(&child));
        assert_eq!(state.get_distance(&root), Some(0));
        assert_eq!(state.get_distance(&child), Some(1));
    }

    #[test]
    fn test_apply_removed() {
        let state = CanonicalGraphState::new();
        let root = make_id(1);
        let child = make_id(2);

        // Add first
        state.apply_changes(
            root,
            &[ParsedNodeChange {
                space_id: child,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            }],
        );

        // Now remove
        let result = state.apply_changes(
            root,
            &[ParsedNodeChange {
                space_id: child,
                change_type: ChangeType::Removed,
                distance: None,
                parent_id: None,
            }],
        );

        assert_eq!(result.len(), 1);
        assert!(!result[0].in_canonical_graph);
        assert!(!state.is_canonical(&child));
        assert!(state.is_canonical(&root));
    }

    #[test]
    fn test_apply_moved_no_canonicality_change() {
        let state = CanonicalGraphState::new();
        let root = make_id(1);
        let parent_a = make_id(2);
        let parent_b = make_id(3);
        let node = make_id(4);

        // Build initial tree: root -> parent_a -> node, root -> parent_b
        state.apply_changes(
            root,
            &[
                ParsedNodeChange {
                    space_id: parent_a,
                    change_type: ChangeType::Added,
                    distance: Some(1),
                    parent_id: Some(root),
                },
                ParsedNodeChange {
                    space_id: parent_b,
                    change_type: ChangeType::Added,
                    distance: Some(1),
                    parent_id: Some(root),
                },
                ParsedNodeChange {
                    space_id: node,
                    change_type: ChangeType::Added,
                    distance: Some(2),
                    parent_id: Some(parent_a),
                },
            ],
        );

        // Move node from parent_a to parent_b
        let result = state.apply_changes(
            root,
            &[ParsedNodeChange {
                space_id: node,
                change_type: ChangeType::Moved,
                distance: Some(2),
                parent_id: Some(parent_b),
            }],
        );

        // No canonicality change for MOVED
        assert!(result.is_empty());
        assert!(state.is_canonical(&node));

        // Verify subspaces reflect the move
        let parent_a_subs = state
            .get_subspaces(&parent_a)
            .expect("parent_a should be in canonical graph");
        assert_eq!(parent_a_subs.len(), 1); // just parent_a itself

        let parent_b_subs = state
            .get_subspaces(&parent_b)
            .expect("parent_b should be in canonical graph");
        assert_eq!(parent_b_subs.len(), 2); // parent_b + node
    }

    #[test]
    fn test_get_subspaces_bfs() {
        let state = CanonicalGraphState::new();
        let root = make_id(1);
        let child1 = make_id(2);
        let child2 = make_id(3);
        let grandchild = make_id(4);

        state.apply_changes(
            root,
            &[
                ParsedNodeChange {
                    space_id: child1,
                    change_type: ChangeType::Added,
                    distance: Some(1),
                    parent_id: Some(root),
                },
                ParsedNodeChange {
                    space_id: child2,
                    change_type: ChangeType::Added,
                    distance: Some(1),
                    parent_id: Some(root),
                },
                ParsedNodeChange {
                    space_id: grandchild,
                    change_type: ChangeType::Added,
                    distance: Some(2),
                    parent_id: Some(child1),
                },
            ],
        );

        // Root subtree = all 4 nodes
        let root_subs = state
            .get_subspaces(&root)
            .expect("Root should be in canonical graph");
        assert_eq!(root_subs.len(), 4);

        // child1 subtree = child1 + grandchild
        let child1_subs = state
            .get_subspaces(&child1)
            .expect("child1 should be in canonical graph");
        assert_eq!(child1_subs.len(), 2);

        // child2 subtree = just child2
        let child2_subs = state
            .get_subspaces(&child2)
            .expect("child2 should be in canonical graph");
        assert_eq!(child2_subs.len(), 1);

        // Non-canonical node returns None
        assert!(state.get_subspaces(&make_id(99)).is_none());
    }

    #[test]
    fn test_idempotent_add() {
        let state = CanonicalGraphState::new();
        let root = make_id(1);
        let child = make_id(2);

        let changes = vec![ParsedNodeChange {
            space_id: child,
            change_type: ChangeType::Added,
            distance: Some(1),
            parent_id: Some(root),
        }];

        let result1 = state.apply_changes(root, &changes);
        assert_eq!(result1.len(), 2); // root + child

        // Apply same changes again — should be idempotent
        let result2 = state.apply_changes(root, &changes);
        assert!(result2.is_empty()); // no new changes

        assert_eq!(state.len(), 2);
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let state = CanonicalGraphState::new();
        let root = make_id(1);

        state.apply_changes(
            root,
            &[
                ParsedNodeChange {
                    space_id: make_id(2),
                    change_type: ChangeType::Added,
                    distance: Some(1),
                    parent_id: Some(root),
                },
                ParsedNodeChange {
                    space_id: make_id(3),
                    change_type: ChangeType::Added,
                    distance: Some(2),
                    parent_id: Some(make_id(2)),
                },
            ],
        );

        let (snap_root, snap_nodes) = state.snapshot();
        let restored = CanonicalGraphState::from_snapshot(snap_root, snap_nodes);

        assert_eq!(restored.len(), state.len());
        assert!(restored.is_canonical(&root));
        assert!(restored.is_canonical(&make_id(2)));
        assert!(restored.is_canonical(&make_id(3)));
        assert_eq!(restored.get_distance(&make_id(3)), Some(2));

        let subs = restored
            .get_subspaces(&root)
            .expect("Root should have subspaces after snapshot roundtrip");
        assert_eq!(subs.len(), 3);
    }
}
