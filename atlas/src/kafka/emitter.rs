//! Canonical graph emitter
//!
//! Emits canonical graph updates to Kafka when the graph changes.
//! Supports both full snapshots (CanonicalGraphUpdated) and incremental diffs (CanonicalGraphDiff).
//!
//! Implementation notes:
//! - Tree -> protobuf conversion is iterative to avoid stack overflow on deep trees.
//! - Diff emission treats Root edge positions as invalid defensive input and drops
//!   them with a warning rather than emitting malformed edge metadata.
//!
//! # Example
//!
//! ```ignore
//! use atlas::kafka::{AtlasProducer, CanonicalGraphEmitter};
//! use atlas::graph::{CanonicalProcessor, DiffTracker, GraphState, TransitiveProcessor};
//!
//! // Set up Kafka producer and emitter
//! let producer = AtlasProducer::new("localhost:9092", "topology.canonical")?;
//! let emitter = CanonicalGraphEmitter::new(producer);
//! let mut diff_tracker = DiffTracker::new();
//!
//! // Process a block: apply all events, then compute and emit once.
//! // Block-level batching avoids emitting intermediate states.
//! for event in block_events {
//!     // Order matters: transitive reads pre-mutation state for cache invalidation,
//!     // then graph state is updated with the new event.
//!     transitive.handle_event(&event, &state);
//!     state.apply_event(&event);
//! }
//!
//! // Compute canonical graph once per block and emit a single diff
//! if let Some(graph) = canonical.compute_if_changed(&state, &mut transitive) {
//!     let diff = diff_tracker.track(&graph);
//!     emitter.emit_diff(&graph.root, &diff, &block_meta)?;
//! }
//! ```

use crate::events::{BlockMetadata, SpaceId};
use crate::graph::{
    CanonicalGraph, ChangeType, EdgeType, GraphDiff, NodeChange, Position, TreeNode,
};
use crate::kafka::{AtlasProducer, ProducerError};
use hermes_instrumentation::{debug_span, warn};
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata as ProtoBlockchainMetadata;
use hermes_schema::pb::topology::{
    canonical_tree_node::Edge, edge_info, CanonicalGraphDiff, CanonicalGraphUpdated,
    CanonicalTreeNode, ChangeType as ProtoChangeType, EdgeInfo,
    NodeChange as ProtoNodeChange, RelatedEdge, RootEdge, TopicEdge, VerifiedEdge,
};
use prost::Message;

/// Emits canonical graph updates to Kafka
pub struct CanonicalGraphEmitter {
    producer: AtlasProducer,
}

impl CanonicalGraphEmitter {
    /// Create a new emitter with the given producer
    pub fn new(producer: AtlasProducer) -> Self {
        Self { producer }
    }

    /// Emit a canonical graph update to Kafka (full snapshot)
    ///
    /// Converts the graph to protobuf, encodes it, and sends to Kafka.
    ///
    /// NOTE: Prefer `emit_diff` for production use - it's more efficient.
    pub fn emit(&self, graph: &CanonicalGraph, meta: &BlockMetadata) -> Result<(), ProducerError> {
        let update = CanonicalGraphUpdated {
            root_id: graph.root.to_vec(),
            tree: Some(tree_node_to_proto(&graph.tree)),
            canonical_space_ids: graph.members.iter().map(|id| id.to_vec()).collect(),
            meta: Some(block_meta_to_proto(meta)),
        };

        let mut payload = Vec::with_capacity(update.encoded_len());
        update
            .encode(&mut payload)
            .expect("Vec<u8> provides sufficient buffer capacity");

        debug_span!(
            "kafka.send.snapshot",
            payload_size = payload.len(),
            node_count = graph.len()
        )
        .in_scope(|| self.producer.send_and_flush(&graph.root, &payload))
    }

    /// Emit a canonical graph diff to Kafka (incremental update)
    ///
    /// Converts the diff to protobuf, encodes it, and sends to Kafka.
    /// Empty diffs are skipped (returns Ok immediately).
    pub fn emit_diff(
        &self,
        root_id: &SpaceId,
        diff: &GraphDiff,
        meta: &BlockMetadata,
    ) -> Result<(), ProducerError> {
        // Skip empty diffs
        if diff.is_empty() {
            return Ok(());
        }

        let proto_diff = CanonicalGraphDiff {
            root_id: root_id.to_vec(),
            changes: diff.changes.iter().map(node_change_to_proto).collect(),
            meta: Some(block_meta_to_proto(meta)),
        };

        let mut payload = Vec::with_capacity(proto_diff.encoded_len());
        proto_diff
            .encode(&mut payload)
            .expect("Vec<u8> provides sufficient buffer capacity");

        debug_span!(
            "kafka.send.diff",
            payload_size = payload.len(),
            change_count = diff.len()
        )
        .in_scope(|| self.producer.send_and_flush(root_id, &payload))
    }
}

/// Convert a tree node to protobuf representation.
///
/// Uses an iterative post-order traversal with an explicit stack to avoid
/// stack overflow on deep graphs (~80K linear chains).
fn tree_node_to_proto(root: &TreeNode) -> CanonicalTreeNode {
    // Phase 1: Collect nodes in pre-order, tracking parent indices.
    // Each entry: (source_node, parent_index_in_order or None for root)
    let mut order: Vec<(&TreeNode, Option<usize>)> = Vec::new();
    let mut stack: Vec<(&TreeNode, Option<usize>)> = vec![(root, None)];

    while let Some((node, parent_idx)) = stack.pop() {
        let my_idx = order.len();
        order.push((node, parent_idx));

        // Push children in reverse so left children are processed first
        for child in node.children.iter().rev() {
            stack.push((child, Some(my_idx)));
        }
    }

    // Phase 2: Build proto nodes, then assemble in reverse order
    // (children finalized before their parents consume them).
    let mut built: Vec<Option<CanonicalTreeNode>> = order
        .iter()
        .map(|(node, _)| {
            Some(CanonicalTreeNode {
                space_id: node.space_id.to_vec(),
                edge: Some(edge_type_to_proto_edge(node.edge_type)),
                children: Vec::new(),
            })
        })
        .collect();

    for i in (0..order.len()).rev() {
        if let Some(parent_idx) = order[i].1 {
            let child_node = built[i].take().unwrap();
            built[parent_idx]
                .as_mut()
                .unwrap()
                .children
                .push(child_node);
        }
    }

    // Reverse-order assembly produces children in reverse; restore original order.
    // Only nodes that actually have children need reversal.
    for node_opt in &mut built {
        if let Some(node) = node_opt.as_mut() {
            if node.children.len() > 1 {
                node.children.reverse();
            }
        }
    }

    built[0].take().unwrap()
}

fn edge_type_to_proto_edge(edge_type: EdgeType) -> Edge {
    match edge_type {
        EdgeType::Root => Edge::Root(RootEdge {}),
        EdgeType::Verified => Edge::Verified(VerifiedEdge {}),
        EdgeType::Related => Edge::Related(RelatedEdge {}),
        EdgeType::Topic { topic_id } => Edge::Topic(TopicEdge {
            topic_id: topic_id.to_vec(),
        }),
    }
}

fn block_meta_to_proto(meta: &BlockMetadata) -> ProtoBlockchainMetadata {
    ProtoBlockchainMetadata {
        created_at: meta.block_timestamp,
        created_by: Vec::new(),
        block_number: meta.block_number,
        cursor: meta.cursor.clone(),
        sequence: 0,
        is_last: false,
    }
}

fn node_change_to_proto(change: &NodeChange) -> ProtoNodeChange {
    ProtoNodeChange {
        space_id: change.space_id.to_vec(),
        change_type: match change.change_type {
            ChangeType::Added => ProtoChangeType::Added as i32,
            ChangeType::Removed => ProtoChangeType::Removed as i32,
            ChangeType::Moved => ProtoChangeType::Moved as i32,
        },
        distance: change.position.as_ref().map(|p| p.distance),
        parent_edge: change.position.as_ref().and_then(position_to_edge_info),
    }
}

fn position_to_edge_info(pos: &Position) -> Option<EdgeInfo> {
    let edge_type = match pos.edge_type {
        EdgeType::Verified => edge_info::EdgeType::Verified(VerifiedEdge {}),
        EdgeType::Related => edge_info::EdgeType::Related(RelatedEdge {}),
        EdgeType::Topic { topic_id } => edge_info::EdgeType::Topic(TopicEdge {
            topic_id: topic_id.to_vec(),
        }),
        EdgeType::Root => {
            warn!(
                parent = ?pos.parent,
                "Root edge reached diff emitter — this should not happen, skipping"
            );
            return None;
        }
    };

    Some(EdgeInfo {
        parent_id: pos.parent.to_vec(),
        edge_type: Some(edge_type),
    })
}

impl std::fmt::Debug for CanonicalGraphEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanonicalGraphEmitter")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{make_block_meta, make_space_id, make_topic_id};

    #[test]
    fn test_tree_node_to_proto_single_node() {
        let root = TreeNode::new_root(make_space_id(1));
        let proto = tree_node_to_proto(&root);

        assert_eq!(proto.space_id, make_space_id(1).to_vec());
        assert!(matches!(proto.edge, Some(Edge::Root(RootEdge {}))));
        assert!(proto.children.is_empty());
    }

    #[test]
    fn test_tree_node_to_proto_preserves_structure() {
        // Root -> A (verified) -> B (related)
        let mut root = TreeNode::new_root(make_space_id(1));
        let mut a = TreeNode::new(make_space_id(2), EdgeType::Verified);
        let b = TreeNode::new(make_space_id(3), EdgeType::Related);
        a.add_child(b);
        root.add_child(a);

        let proto = tree_node_to_proto(&root);

        assert_eq!(proto.children.len(), 1);
        let proto_a = &proto.children[0];
        assert_eq!(proto_a.space_id, make_space_id(2).to_vec());
        assert!(matches!(
            proto_a.edge,
            Some(Edge::Verified(VerifiedEdge {}))
        ));

        assert_eq!(proto_a.children.len(), 1);
        let proto_b = &proto_a.children[0];
        assert_eq!(proto_b.space_id, make_space_id(3).to_vec());
        assert!(matches!(proto_b.edge, Some(Edge::Related(RelatedEdge {}))));
        assert!(proto_b.children.is_empty());
    }

    #[test]
    fn test_tree_node_to_proto_all_edge_types() {
        let mut root = TreeNode::new_root(make_space_id(1));
        root.add_child(TreeNode::new(make_space_id(2), EdgeType::Verified));
        root.add_child(TreeNode::new(make_space_id(3), EdgeType::Related));
        root.add_child(TreeNode::new_with_topic(
            make_space_id(4),
            make_topic_id(0x8A),
        ));

        let proto = tree_node_to_proto(&root);
        assert_eq!(proto.children.len(), 3);

        assert!(matches!(proto.children[0].edge, Some(Edge::Verified(_))));
        assert!(matches!(proto.children[1].edge, Some(Edge::Related(_))));

        match &proto.children[2].edge {
            Some(Edge::Topic(TopicEdge { topic_id })) => {
                assert_eq!(*topic_id, make_topic_id(0x8A).to_vec());
            }
            other => panic!("expected Topic edge, got {:?}", other),
        }
    }

    #[test]
    fn test_tree_node_to_proto_wide_tree() {
        let mut root = TreeNode::new_root(make_space_id(1));
        for i in 2..=10 {
            root.add_child(TreeNode::new(make_space_id(i), EdgeType::Verified));
        }

        let proto = tree_node_to_proto(&root);
        assert_eq!(proto.children.len(), 9);
    }

    #[test]
    fn test_node_change_to_proto_added() {
        let change = NodeChange {
            space_id: make_space_id(5),
            change_type: ChangeType::Added,
            position: Some(Position {
                distance: 2,
                parent: make_space_id(1),
                edge_type: EdgeType::Verified,
            }),
        };

        let proto = node_change_to_proto(&change);
        assert_eq!(proto.space_id, make_space_id(5).to_vec());
        assert_eq!(proto.change_type, ProtoChangeType::Added as i32);
        assert_eq!(proto.distance, Some(2));
        assert!(proto.parent_edge.is_some());

        let edge_info = proto.parent_edge.unwrap();
        assert_eq!(edge_info.parent_id, make_space_id(1).to_vec());
        assert!(matches!(
            edge_info.edge_type,
            Some(edge_info::EdgeType::Verified(_))
        ));
    }

    #[test]
    fn test_node_change_to_proto_removed() {
        let change = NodeChange {
            space_id: make_space_id(3),
            change_type: ChangeType::Removed,
            position: None,
        };

        let proto = node_change_to_proto(&change);
        assert_eq!(proto.change_type, ProtoChangeType::Removed as i32);
        assert_eq!(proto.distance, None);
        assert!(proto.parent_edge.is_none());
    }

    #[test]
    fn test_node_change_to_proto_moved() {
        let change = NodeChange {
            space_id: make_space_id(4),
            change_type: ChangeType::Moved,
            position: Some(Position {
                distance: 3,
                parent: make_space_id(2),
                edge_type: EdgeType::Related,
            }),
        };

        let proto = node_change_to_proto(&change);
        assert_eq!(proto.change_type, ProtoChangeType::Moved as i32);
        assert_eq!(proto.distance, Some(3));
    }

    #[test]
    fn test_position_to_edge_info_root_returns_none() {
        let pos = Position {
            distance: 0,
            parent: make_space_id(1),
            edge_type: EdgeType::Root,
        };

        // Root edge should return None (this is a defensive edge case)
        assert!(position_to_edge_info(&pos).is_none());
    }

    #[test]
    fn test_position_to_edge_info_topic_carries_topic_id() {
        let topic_id = make_topic_id(0x8B);
        let pos = Position {
            distance: 1,
            parent: make_space_id(1),
            edge_type: EdgeType::Topic { topic_id },
        };

        let info = position_to_edge_info(&pos).unwrap();
        assert_eq!(info.parent_id, make_space_id(1).to_vec());

        match info.edge_type {
            Some(edge_info::EdgeType::Topic(TopicEdge { topic_id: tid })) => {
                assert_eq!(tid, topic_id.to_vec());
            }
            other => panic!("expected Topic edge, got {:?}", other),
        }
    }

    #[test]
    fn test_block_meta_to_proto() {
        let meta = make_block_meta();
        let proto = block_meta_to_proto(&meta);

        assert_eq!(proto.block_number, meta.block_number);
        assert_eq!(proto.created_at, meta.block_timestamp);
        assert_eq!(proto.cursor, meta.cursor);
    }
}
