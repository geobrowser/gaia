use serde::{Deserialize, Serialize};
use crate::types::{ObjectId, SpaceId, ObjectType};

/// Represents the aggregated vote counts for an entity and space.
///
/// This struct is intended to store the total number of upvotes and 
/// downvotes for a particular entity and space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VotesCount {
    pub object_id: ObjectId,
    pub space_id: SpaceId,
    pub object_type: ObjectType,
    pub upvotes: i64,
    pub downvotes: i64,
}
