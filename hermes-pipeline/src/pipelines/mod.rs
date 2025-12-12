//! Data pipelines for converting Actions to Kafka events.
//!
//! Each pipeline handles a specific action type:
//! - `spaces`: SPACE_REGISTERED → space.creations
//! - `trust`: SUBSPACE_ADDED/REMOVED → space.trust.extensions
//! - `edits`: EDITS_PUBLISHED → knowledge.edits

pub mod edits;
pub mod spaces;
pub mod trust;

use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;

/// Block metadata extracted from substream data.
///
/// Used by all pipelines for enriching events with blockchain context.
#[derive(Debug, Clone)]
pub struct BlockMetadata {
    pub cursor: String,
    pub block_number: u64,
    pub timestamp: String,
}

impl BlockMetadata {
    /// Convert block metadata to BlockchainMetadata proto.
    pub fn to_proto(&self) -> BlockchainMetadata {
        let created_at: u64 = self.timestamp.parse().unwrap_or(0);

        BlockchainMetadata {
            created_at,
            created_by: vec![], // Not available in block metadata
            block_number: self.block_number,
            cursor: self.cursor.clone(),
        }
    }
}

impl From<hermes_relay::stream::utils::BlockMetadata> for BlockMetadata {
    fn from(meta: hermes_relay::stream::utils::BlockMetadata) -> Self {
        Self {
            cursor: meta.cursor,
            block_number: meta.block_number,
            timestamp: meta.timestamp,
        }
    }
}
