//! Tree node representation for graph traversals
//!
//! Trees are used to represent the result of BFS traversals,
//! preserving the parent-child relationships and edge metadata.

use crate::events::{SpaceId, TopicId};
use std::hash::{Hash, Hasher};

/// The type of edge connecting a node to its parent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeType {
    /// Root node has no incoming edge
    Root,
    /// Verified trust relationship
    Verified,
    /// Related trust relationship
    Related,
    /// Topic-based membership
    Topic,
}

/// A node in the traversal tree
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// The space this node represents
    pub space_id: SpaceId,

    /// How this node was reached from its parent
    pub edge_type: EdgeType,

    /// If reached via topic edge, which topic
    pub topic_id: Option<TopicId>,

    /// Children of this node in the traversal
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    /// Create a new root node
    pub fn new_root(space_id: SpaceId) -> Self {
        Self {
            space_id,
            edge_type: EdgeType::Root,
            topic_id: None,
            children: Vec::new(),
        }
    }

    /// Create a new node with the given edge type
    pub fn new(space_id: SpaceId, edge_type: EdgeType) -> Self {
        Self {
            space_id,
            edge_type,
            topic_id: None,
            children: Vec::new(),
        }
    }

    /// Create a new node reached via a topic edge
    pub fn new_with_topic(space_id: SpaceId, topic_id: TopicId) -> Self {
        Self {
            space_id,
            edge_type: EdgeType::Topic,
            topic_id: Some(topic_id),
            children: Vec::new(),
        }
    }

    /// Add a child node
    pub fn add_child(&mut self, child: TreeNode) {
        self.children.push(child);
    }

    /// Count total nodes in this subtree (including self)
    pub fn node_count(&self) -> usize {
        1 + self.children.iter().map(|c| c.node_count()).sum::<usize>()
    }

    /// Compute a hash of the tree structure for change detection
    pub fn compute_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash_into(&mut hasher);
        hasher.finish()
    }

    /// Recursively hash tree structure
    fn hash_into<H: Hasher>(&self, hasher: &mut H) {
        self.space_id.hash(hasher);
        self.edge_type.hash(hasher);
        self.topic_id.hash(hasher);
        self.children.len().hash(hasher);
        for child in &self.children {
            child.hash_into(hasher);
        }
    }
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

    #[test]
    fn test_new_root() {
        let space = make_space_id(1);
        let node = TreeNode::new_root(space);

        assert_eq!(node.space_id, space);
        assert_eq!(node.edge_type, EdgeType::Root);
        assert!(node.topic_id.is_none());
        assert!(node.children.is_empty());
    }

    #[test]
    fn test_new_with_topic() {
        let space = make_space_id(1);
        let topic = make_topic_id(2);
        let node = TreeNode::new_with_topic(space, topic);

        assert_eq!(node.space_id, space);
        assert_eq!(node.edge_type, EdgeType::Topic);
        assert_eq!(node.topic_id, Some(topic));
    }

    #[test]
    fn test_add_child() {
        let mut root = TreeNode::new_root(make_space_id(1));
        let child = TreeNode::new(make_space_id(2), EdgeType::Verified);

        root.add_child(child);

        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].space_id, make_space_id(2));
    }

    #[test]
    fn test_node_count() {
        let mut root = TreeNode::new_root(make_space_id(1));
        let mut child1 = TreeNode::new(make_space_id(2), EdgeType::Verified);
        let child2 = TreeNode::new(make_space_id(3), EdgeType::Related);
        let grandchild = TreeNode::new(make_space_id(4), EdgeType::Verified);

        child1.add_child(grandchild);
        root.add_child(child1);
        root.add_child(child2);

        assert_eq!(root.node_count(), 4);
    }

    #[test]
    fn test_compute_hash_deterministic() {
        let mut root1 = TreeNode::new_root(make_space_id(1));
        root1.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));

        let mut root2 = TreeNode::new_root(make_space_id(1));
        root2.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));

        assert_eq!(root1.compute_hash(), root2.compute_hash());
    }

    #[test]
    fn test_compute_hash_different_structures() {
        let root1 = TreeNode::new_root(make_space_id(1));

        let mut root2 = TreeNode::new_root(make_space_id(1));
        root2.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));

        assert_ne!(root1.compute_hash(), root2.compute_hash());
    }
}
