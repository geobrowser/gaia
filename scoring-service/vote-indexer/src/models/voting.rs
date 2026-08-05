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

/// Which response axis a vote belongs to.
///
/// The three axes are independent — a user may hold one response of each on the
/// same object, and casting one never touches another. This is the decoded form
/// of the on-chain action hash: topic and data encoding are identical across all
/// nine actions, so the hash is the only discriminator.
///
/// `Curation` is 0 so rows written before this column existed read back as
/// curation, which is what they are.
#[derive(Clone, Debug, PartialEq, Eq, Copy, Hash, Default)]
pub enum ResponseKind {
    /// Upvote / downvote.
    #[default]
    Curation,
    /// Agree / disagree.
    Stance,
    /// Verify / dispute.
    Veracity,
}

impl From<ResponseKind> for i16 {
    fn from(k: ResponseKind) -> i16 {
        match k {
            ResponseKind::Curation => 0,
            ResponseKind::Stance => 1,
            ResponseKind::Veracity => 2,
        }
    }
}

impl From<i16> for ResponseKind {
    fn from(v: i16) -> ResponseKind {
        match v {
            1 => ResponseKind::Stance,
            2 => ResponseKind::Veracity,
            // Anything else — including a kind written by a newer producer that
            // this build does not know about — reads as curation, matching the
            // column default rather than panicking mid-batch.
            _ => ResponseKind::Curation,
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
    /// Which response axis this vote is on
    pub kind: ResponseKind,
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
    /// Which response axis this vote is on
    pub kind: ResponseKind,
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
    /// Which response axis these tallies are on
    pub kind: ResponseKind,
    /// Positive tally on this axis (upvotes / agrees / verifications)
    pub positive: i64,
    /// Negative tally on this axis (downvotes / disagrees / disputes)
    pub negative: i64,
}

/// Criteria for querying user votes:
/// (voter_id, object_id, space_id, object_type, kind)
///
/// `kind` is part of the key, not an attribute: it is what makes a Verify and an
/// upvote by the same user on the same object two separate rows rather than one
/// overwriting the other.
pub type UserVoteCriteria = (Uuid, Uuid, Uuid, VoteObjectType, ResponseKind);

/// Criteria for querying vote counts: (object_id, space_id, object_type, kind)
pub type VoteCountCriteria = (Uuid, Uuid, VoteObjectType, ResponseKind);

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
