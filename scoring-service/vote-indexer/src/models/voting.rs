use uuid::Uuid;

/// Vote direction (matches VoteDirection from hermes-schema)
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum VoteValue {
    Up,
    Down,
    Remove,
}

impl From<VoteValue> for i16 {
    fn from(v: VoteValue) -> i16 {
        match v {
            VoteValue::Up => 0,
            VoteValue::Down => 1,
            VoteValue::Remove => 2,
        }
    }
}

impl From<i16> for VoteValue {
    fn from(v: i16) -> VoteValue {
        match v {
            0 => VoteValue::Up,
            1 => VoteValue::Down,
            _ => VoteValue::Remove,
        }
    }
}

/// Type of object being voted on
#[derive(Clone, Debug, PartialEq, Eq, Copy, Hash)]
pub enum VoteObjectType {
    Entity,
    Relation,
}

impl From<VoteObjectType> for i16 {
    fn from(t: VoteObjectType) -> i16 {
        match t {
            VoteObjectType::Entity => 0,
            VoteObjectType::Relation => 1,
        }
    }
}

impl From<i16> for VoteObjectType {
    fn from(v: i16) -> VoteObjectType {
        match v {
            1 => VoteObjectType::Relation,
            _ => VoteObjectType::Entity,
        }
    }
}

/// Processed vote from HermesVoteCast
#[derive(Clone, Debug)]
pub struct VoteItem {
    /// Voter's space ID
    pub voter_id: Uuid,
    /// Entity or relation being voted on
    pub object_id: Uuid,
    /// Type of object (Entity or Relation)
    pub object_type: VoteObjectType,
    /// Space point of view
    pub space_id: Uuid,
    /// Vote direction
    pub vote: VoteValue,
    /// Block number when vote was cast
    pub block_number: u64,
    /// Block timestamp when vote was cast
    pub block_timestamp: u64,
}

/// Current vote state per user/entity/space (for upsert operations)
#[derive(Clone, Debug)]
pub struct UserVoteItem {
    /// Voter's space ID
    pub voter_id: Uuid,
    /// Entity or relation being voted on
    pub object_id: Uuid,
    /// Type of object (Entity or Relation)
    pub object_type: VoteObjectType,
    /// Space point of view
    pub space_id: Uuid,
    /// Current vote type
    pub vote_type: VoteValue,
    /// Timestamp when vote was cast
    pub voted_at: u64,
}

/// Aggregated vote counts per entity/space
#[derive(Clone, Debug)]
pub struct VotesCountItem {
    /// Entity or relation ID
    pub object_id: Uuid,
    /// Type of object (Entity or Relation)
    pub object_type: VoteObjectType,
    /// Space point of view
    pub space_id: Uuid,
    /// Total upvotes
    pub upvotes: i64,
    /// Total downvotes
    pub downvotes: i64,
}

/// Criteria for querying user votes: (voter_id, object_id, space_id, object_type)
pub type UserVoteCriteria = (Uuid, Uuid, Uuid, VoteObjectType);

/// Criteria for querying vote counts: (object_id, space_id, object_type)
pub type VoteCountCriteria = (Uuid, Uuid, VoteObjectType);

/// Row to upsert into the `values` table mirroring an entity's net score.
///
/// `id` is a deterministic UUIDv5 of the name `score:<entity>:<space>` under
/// `GEO_SYSTEM_NAMESPACE`. The `score:` tag keeps these ids disjoint from
/// kg-indexer-minted value ids and any other `(entity_id, space_id)` scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreValueItem {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub space_id: Uuid,
    pub integer: i64,
}
