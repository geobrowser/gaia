use super::action_vote::Vote;

/// Represents a processed action with its associated data.
///
/// This enum is intended to hold the structured data of an action
/// after it has been consumed and processed by the indexer pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Represents a vote action, containing details about the vote.
    Vote(Vote),
}
