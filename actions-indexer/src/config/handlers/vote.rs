use actions_indexer_pipeline::errors::ProcessorError;
use actions_indexer_pipeline::processor::HandleAction;
use actions_indexer_shared::types::{Action, ActionRaw, Vote, VoteValue};

pub struct VoteHandler;

impl HandleAction for VoteHandler {
    /// Handles a vote action.
    ///
    /// This method converts the `ActionRaw` into a `Vote` enum variant.
    ///
    /// # Arguments
    ///
    /// * `action` - A reference to the `ActionRaw` to handle
    ///
    /// # Returns
    ///
    /// A `Result` containing the `Action` enum variant.
    ///
    /// # Errors
    ///
    /// Returns a `ProcessorError` if the vote is invalid.
    ///
    fn handle(&self, action: &ActionRaw) -> Result<Action, ProcessorError> {
        Ok(Action::Vote(Vote {
            raw: action.clone().into(),
            vote: match action.metadata.as_ref().and_then(|m| m.first()) {
                Some(&0) => VoteValue::Up,
                Some(&1) => VoteValue::Down,
                Some(&2) => VoteValue::Remove,
                _ => return Err(ProcessorError::InvalidVote),
            },
        }))
    }
}
