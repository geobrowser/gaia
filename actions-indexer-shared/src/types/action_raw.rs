use crate::types::{UserAddress, SpaceId, ObjectId, GroupId, ObjectType, ActionType};
use alloy::primitives::{BlockNumber, BlockTimestamp, Bytes, TxHash};
use serde::{Deserialize, Serialize};

/// Represents the raw data of an action as extracted from an event.
///
/// This struct holds the essential fields common to all action types,
/// providing a base for further processing into specific `Action` variants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Eq)]
pub struct ActionRaw {
    pub action_type: ActionType,
    pub action_version: u64,
    pub sender: UserAddress,
    pub object_id: ObjectId,
    pub group_id: Option<GroupId>,
    pub space_pov: SpaceId,
    pub metadata: Option<Bytes>,
    pub block_number: BlockNumber,
    pub block_timestamp: BlockTimestamp,
    pub tx_hash: TxHash,
    pub object_type: ObjectType,
}