use crate::types::{Action, UserVote, VotesCount};

/// Represents a collection of changes to be persisted in the actions repository.
///
/// A `Changeset` bundles new actions, updated user votes, and updated vote counts
/// together for atomic persistence operations.
pub struct Changeset<'a> {
    pub actions: &'a [Action],
    pub user_votes: &'a [UserVote],
    pub votes_count: &'a [VotesCount],
}
