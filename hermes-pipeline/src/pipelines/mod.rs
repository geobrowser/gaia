//! Data pipelines for converting Actions to Kafka events.
//!
//! Each pipeline handles a specific action type:
//! - `spaces`: SPACE_REGISTERED → space.creations
//! - `trust`: SUBSPACE_VERIFIED/RELATED/TOPIC_SET/UNSET → space.trust.extensions
//! - `edits`: EDITS_PUBLISHED → knowledge.edits
//! - `governance`: PROPOSAL_CREATED/UPDATED/VOTED/EXECUTED → space.governance
//! - `membership`: EDITOR/MEMBER ADDED/REMOVED, SPACE_LEFT → space.membership
//! - `moderation`: SPACE_FAST_PATH_RESTRICTED/UNRESTRICTED, FLAGGED/UNFLAGGED → space.moderation
//! - `topics`: TOPIC_SET → space.topics
//! - `voting`: UPVOTED/DOWNVOTED/UNVOTED → curation.votes

pub mod edits;
pub mod governance;
pub mod membership;
pub mod moderation;
pub mod prefetch;
pub mod spaces;
pub mod topics;
pub mod trust;
pub mod voting;

use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::governance::{
    HermesProposalCreated, HermesProposalExecuted, HermesProposalSettingsUpdated,
    HermesProposalUpdated, HermesProposalVoted, HermesVotingSettingsUpdated,
};
use hermes_schema::pb::knowledge::HermesEdit;
use hermes_schema::pb::membership::{HermesRoleGranted, HermesRoleRevoked, HermesSpaceLeft};
use hermes_schema::pb::moderation::{
    HermesContentFlagged, HermesContentUnflagged, HermesEditorFlagged, HermesEditorUnflagged,
};
use hermes_schema::pb::space::{HermesCreateSpace, HermesSpaceTrustExtension};
use hermes_schema::pb::topics::{HermesTopicDeclared, HermesTopicRemoved};
use hermes_schema::pb::voting::HermesVoteCast;

/// Trait for event types that have blockchain metadata.
pub trait HasMeta {
    fn meta(&self) -> Option<&BlockchainMetadata>;
    fn meta_mut(&mut self) -> Option<&mut BlockchainMetadata>;
}

macro_rules! impl_has_meta {
    ($($ty:ty),* $(,)?) => {
        $(
            impl HasMeta for $ty {
                fn meta(&self) -> Option<&BlockchainMetadata> {
                    self.meta.as_ref()
                }
                fn meta_mut(&mut self) -> Option<&mut BlockchainMetadata> {
                    self.meta.as_mut()
                }
            }
        )*
    };
}

impl_has_meta!(
    HermesCreateSpace,
    HermesSpaceTrustExtension,
    HermesRoleGranted,
    HermesRoleRevoked,
    HermesSpaceLeft,
    HermesEditorFlagged,
    HermesEditorUnflagged,
    HermesContentFlagged,
    HermesContentUnflagged,
    HermesTopicDeclared,
    HermesTopicRemoved,
    HermesProposalCreated,
    HermesProposalUpdated,
    HermesProposalVoted,
    HermesProposalExecuted,
    HermesProposalSettingsUpdated,
    HermesVotingSettingsUpdated,
    HermesVoteCast,
    HermesEdit,
);

/// Get the maximum sequence number from a slice of events.
pub fn max_sequence<T: HasMeta>(events: &[T]) -> u32 {
    events
        .iter()
        .filter_map(|e| e.meta().map(|m| m.sequence))
        .max()
        .unwrap_or(0)
}

/// Set is_last on the first event with the given sequence number.
/// Returns true if an event was marked.
pub fn mark_sequence_as_last<T: HasMeta>(events: &mut [T], target_seq: u32) -> bool {
    for event in events.iter_mut() {
        if let Some(meta) = event.meta_mut()
            && meta.sequence == target_seq
        {
            meta.is_last = true;
            return true;
        }
    }
    false
}

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
    ///
    /// The `sequence` parameter is the action array index, representing
    /// blockchain order within the block. The `is_last` flag defaults to
    /// false and is set at emission time.
    pub fn to_proto(&self, sequence: u32) -> BlockchainMetadata {
        let created_at: u64 = self.timestamp.parse().unwrap_or(0);

        BlockchainMetadata {
            created_at,
            created_by: vec![], // Not available in block metadata
            block_number: self.block_number,
            cursor: self.cursor.clone(),
            sequence,
            is_last: false,
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
