//! Tree hashing for change detection
//!
//! Provides a trait-based interface for computing hashes of tree structures.
//! The hash is used to detect changes in the canonical graph.

use super::TreeNode;
use std::hash::{Hash, Hasher};

/// Trait for computing hashes of tree structures
pub trait TreeHasher {
    /// Compute a hash of the given tree
    fn hash_tree(&self, tree: &TreeNode) -> u64;
}

/// Default tree hasher using Rust's DefaultHasher
#[derive(Debug, Default, Clone)]
pub struct DefaultTreeHasher;

impl DefaultTreeHasher {
    pub fn new() -> Self {
        Self
    }
}

impl TreeHasher for DefaultTreeHasher {
    fn hash_tree(&self, tree: &TreeNode) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hash_node_recursive(tree, &mut hasher);
        hasher.finish()
    }
}

/// Recursively hash a tree node and its children
fn hash_node_recursive<H: Hasher>(node: &TreeNode, hasher: &mut H) {
    node.space_id.hash(hasher);
    node.edge_type.hash(hasher);
    node.topic_id.hash(hasher);
    node.children.len().hash(hasher);
    for child in &node.children {
        hash_node_recursive(child, hasher);
    }
}

/// Convenience function to hash a tree with the default hasher
pub fn hash_tree(tree: &TreeNode) -> u64 {
    DefaultTreeHasher::new().hash_tree(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SpaceId;
    use crate::graph::EdgeType;

    fn make_space_id(n: u8) -> SpaceId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

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

    #[test]
    fn test_hasher_trait_implementation() {
        let hasher = DefaultTreeHasher::new();
        let tree = TreeNode::new_root(make_space_id(1));

        let hash1 = hasher.hash_tree(&tree);
        let hash2 = hasher.hash_tree(&tree);

        assert_eq!(hash1, hash2);
    }
}
