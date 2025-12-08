//! Canonical graph emitter
//!
//! Emits canonical graph updates to Kafka when the graph changes.
//!
//! # Example
//!
//! ```ignore
//! use atlas::kafka::{AtlasProducer, CanonicalGraphEmitter};
//! use atlas::graph::{CanonicalProcessor, GraphState, TransitiveProcessor};
//!
//! // Set up Kafka producer and emitter
//! let producer = AtlasProducer::new("localhost:9092", "topology.canonical")?;
//! let emitter = CanonicalGraphEmitter::new(producer);
//!
//! // Process events and emit canonical graph updates
//! for event in events {
//!     state.apply_event(&event);
//!     transitive.handle_event(&event, &state);
//!
//!     if let Some(graph) = canonical.compute(&state, &mut transitive) {
//!         emitter.emit(&graph, &event.meta)?;
//!     }
//! }
//! ```

use crate::events::BlockMetadata;
use crate::graph::{CanonicalGraph, EdgeType, TreeNode};
use crate::kafka::{AtlasProducer, ProducerError};
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata as ProtoBlockchainMetadata;
use hermes_schema::pb::topology::{
    CanonicalGraphUpdated, CanonicalTreeNode, EdgeType as ProtoEdgeType,
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

    /// Emit a canonical graph update to Kafka
    ///
    /// Converts the graph to protobuf, encodes it, and sends to Kafka.
    pub fn emit(&self, graph: &CanonicalGraph, meta: &BlockMetadata) -> Result<(), ProducerError> {
        let update = CanonicalGraphUpdated {
            root_id: graph.root.to_vec(),
            tree: Some(tree_node_to_proto(&graph.tree)),
            canonical_space_ids: graph.flat.iter().map(|id| id.to_vec()).collect(),
            meta: Some(ProtoBlockchainMetadata {
                created_at: meta.block_timestamp,
                created_by: Vec::new(),
                block_number: meta.block_number,
                cursor: meta.cursor.clone(),
            }),
        };

        let mut payload = Vec::with_capacity(update.encoded_len());
        update
            .encode(&mut payload)
            .expect("Vec<u8> provides sufficient buffer capacity");

        self.producer.send_and_flush(&graph.root, &payload)
    }
}

fn tree_node_to_proto(node: &TreeNode) -> CanonicalTreeNode {
    CanonicalTreeNode {
        space_id: node.space_id.to_vec(),
        edge_type: match node.edge_type {
            EdgeType::Root => ProtoEdgeType::Root,
            EdgeType::Verified => ProtoEdgeType::Verified,
            EdgeType::Related => ProtoEdgeType::Related,
            EdgeType::Topic => ProtoEdgeType::Topic,
        } as i32,
        topic_id: node.topic_id.map(|t| t.to_vec()).unwrap_or_default(),
        children: node.children.iter().map(tree_node_to_proto).collect(),
    }
}

impl std::fmt::Debug for CanonicalGraphEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanonicalGraphEmitter")
            .finish_non_exhaustive()
    }
}
