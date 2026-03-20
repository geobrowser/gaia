//! Space topology events
//!
//! These events represent changes to the space topology graph,
//! consumed from the blockchain via substreams.

/// Unique identifier for a space
pub type SpaceId = [u8; 16];

/// Unique identifier for a topic
pub type TopicId = [u8; 16];

/// Blockchain address (32 bytes)
pub type Address = [u8; 32];

/// Block metadata associated with each event
#[derive(Debug, Clone)]
pub struct BlockMetadata {
    pub block_number: u64,
    pub block_timestamp: u64,
    pub tx_hash: String,
    pub cursor: String,
}

/// A space topology event from the blockchain
#[derive(Debug, Clone)]
pub struct SpaceTopologyEvent {
    pub meta: BlockMetadata,
    pub payload: SpaceTopologyPayload,
}

/// The payload of a space topology event
#[derive(Debug, Clone)]
pub enum SpaceTopologyPayload {
    SpaceCreated(SpaceCreated),
    TrustExtended(TrustExtended),
}

/// A new space was created
#[derive(Debug, Clone)]
pub struct SpaceCreated {
    pub space_id: SpaceId,
    /// The topic this space announces at creation
    pub topic_id: TopicId,
    pub space_type: SpaceType,
}

/// The type of space being created
#[derive(Debug, Clone)]
pub enum SpaceType {
    Personal {
        owner: Address,
    },
    Dao {
        initial_editors: Vec<SpaceId>,
        initial_members: Vec<SpaceId>,
    },
}

/// A space extended trust to another space or topic
#[derive(Debug, Clone)]
pub struct TrustExtended {
    /// The space extending trust
    pub source_space_id: SpaceId,
    pub extension: TrustExtension,
}

/// The type of trust extension
#[derive(Debug, Clone)]
pub enum TrustExtension {
    // --- Additions ---
    /// Explicit edge granting canonical trust (verified)
    Verified { target_space_id: SpaceId },
    /// Explicit edge for related spaces
    Related { target_space_id: SpaceId },
    /// Topic edge pointing to a topic
    Subtopic { target_topic_id: TopicId },
    /// Editor added to a DAO space (canonical-granting)
    EditorAdded { member_space_id: SpaceId },
    /// Member added to a DAO space (canonical-granting)
    MemberAdded { member_space_id: SpaceId },

    // --- Removals ---
    /// Verified edge removed
    VerifiedRemoved { target_space_id: SpaceId },
    /// Related edge removed
    RelatedRemoved { target_space_id: SpaceId },
    /// Editor removed from a DAO space
    EditorRemoved { member_space_id: SpaceId },
    /// Member removed from a DAO space
    MemberRemoved { member_space_id: SpaceId },
    /// Subtopic edge removed
    SubtopicRemoved { target_topic_id: TopicId },
}
