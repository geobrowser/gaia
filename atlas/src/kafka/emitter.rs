//! Canonical graph emitter
//!
//! Emits canonical graph updates to Kafka when the graph changes.
//! Supports both full snapshots (CanonicalGraphUpdated) and incremental diffs (CanonicalGraphDiff).
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
//! // Process events and emit diffs
//! for event in events {
//!     state.apply_event(&event);
//!     transitive.handle_event(&event, &state);
//!
//!     if let Some(graph) = canonical.compute(&state, &mut transitive) {
//!         let diff = diff_tracker.track(&graph);
//!         emitter.emit_diff(&graph.root, &diff, &event.meta)?;
//!     }
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
    CanonicalTreeNode, ChangeType as ProtoChangeType, EdgeInfo, EditorEdge, MemberEdge,
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
            canonical_space_ids: graph.flat.iter().map(|id| id.to_vec()).collect(),
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

fn tree_node_to_proto(node: &TreeNode) -> CanonicalTreeNode {
    let edge = match node.edge_type {
        EdgeType::Root => Edge::Root(RootEdge {}),
        EdgeType::Verified => Edge::Verified(VerifiedEdge {}),
        EdgeType::Related => Edge::Related(RelatedEdge {}),
        EdgeType::Topic => Edge::Topic(TopicEdge {
            topic_id: node
                .topic_id
                .expect("Topic edge must have topic_id")
                .to_vec(),
        }),
        EdgeType::Editor => Edge::Editor(EditorEdge {}),
        EdgeType::Member => Edge::Member(MemberEdge {}),
    };

    CanonicalTreeNode {
        space_id: node.space_id.to_vec(),
        edge: Some(edge),
        children: node.children.iter().map(tree_node_to_proto).collect(),
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
        EdgeType::Topic => edge_info::EdgeType::Topic(TopicEdge {
            topic_id: pos.topic_id.map(|t| t.to_vec()).unwrap_or_default(),
        }),
        EdgeType::Editor => edge_info::EdgeType::Editor(EditorEdge {}),
        EdgeType::Member => edge_info::EdgeType::Member(MemberEdge {}),
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
