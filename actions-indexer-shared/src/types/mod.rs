//! This module defines the core data structures and types used across the actions indexer.
//! It re-exports specific types like `Action`, `UserVote`, `VotesCount`, `Changeset`, `ActionRaw`, `Vote`, and `VoteValue`.
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod action;
mod action_raw;
mod action_vote;
mod changeset;
mod user_vote;
mod votes_count;

pub use action::Action;
pub use action_raw::ActionRaw;
pub use action_vote::{Vote, VoteValue};
pub use changeset::Changeset;
pub use user_vote::UserVote;
pub use votes_count::VotesCount;

pub type ObjectId = Uuid;
pub type GroupId = Uuid;
pub type SpaceId = Uuid;
pub type UserAddress = Address;
pub type VoteCriteria = (UserAddress, ObjectId, SpaceId, ObjectType);
pub type VoteCountCriteria = (ObjectId, SpaceId, ObjectType);
pub type ActionVersion = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash, Copy)]
pub enum ObjectType {
    Entity,
    Relation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash, Copy)]
pub enum ActionType {
    Vote,
}
