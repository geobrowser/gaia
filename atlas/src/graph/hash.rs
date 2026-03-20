//! Tree hashing for change detection
//!
//! Computes deterministic hashes of tree structures for detecting changes
//! in the canonical graph.

use super::TreeNode;
use std::hash::{Hash, Hasher};

/// Hash a tree using iterative pre-order DFS with SipHash-2-4 (64-bit).
///
/// This hash is used for **change detection**, not content addressing. A
/// collision means a changed tree is silently treated as unchanged for one
/// block (the next event will likely re-trigger computation).
///
/// Birthday bound: ~2^32 distinct trees before 50% collision probability.
/// At realistic throughput (millions of distinct trees, not billions) the
/// risk is negligible. If the system scales to very high throughput,
/// consider migrating to a 128-bit hash (e.g., blake3).
pub fn hash_tree(tree: &TreeNode) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    // Pre-order DFS: visit node, then children left-to-right.
    // Push children in reverse so leftmost is popped first.
    let mut stack = vec![tree];
    while let Some(node) = stack.pop() {
        node.space_id.hash(&mut hasher);
        node.edge_type.hash(&mut hasher);
        node.children.len().hash(&mut hasher);
        for child in node.children.iter().rev() {
            stack.push(child);
        }
    }

    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeType;
    use crate::test_utils::make_space_id;

    #[test]
    fn test_hash_tree_deterministic() {
        let mut root1 = TreeNode::new_root(make_space_id(1));
        root1.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));

        let mut root2 = TreeNode::new_root(make_space_id(1));
        root2.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));

        assert_eq!(hash_tree(&root1), hash_tree(&root2));
    }

    #[test]
    fn test_hash_tree_different_structures() {
        let root1 = TreeNode::new_root(make_space_id(1));

        let mut root2 = TreeNode::new_root(make_space_id(1));
        root2.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));

        assert_ne!(hash_tree(&root1), hash_tree(&root2));
    }

    #[test]
    fn test_hash_tree_different_edge_types() {
        let mut root1 = TreeNode::new_root(make_space_id(1));
        root1.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));

        let mut root2 = TreeNode::new_root(make_space_id(1));
        root2.add_child(TreeNode::new(make_space_id(2), EdgeType::Related));

        assert_ne!(hash_tree(&root1), hash_tree(&root2));
    }
}
