//! Pipeline: PROPOSAL_CREATED, PROPOSAL_VOTED, PROPOSAL_EXECUTED → space.governance
//!
//! Converts governance actions to typed Hermes events.

use anyhow::Result;
use hermes_instrumentation::debug_span;

use hermes_relay::{actions, Action};
use hermes_schema::pb::governance::{
    HermesProposalCreated, HermesProposalExecuted, HermesProposalVoted,
};

use super::BlockMetadata;

/// Result of transforming governance actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub proposals_created: Vec<HermesProposalCreated>,
    pub proposals_voted: Vec<HermesProposalVoted>,
    pub proposals_executed: Vec<HermesProposalExecuted>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.proposals_created.len()
            + self.proposals_voted.len()
            + self.proposals_executed.len()
    }
}

/// Transform all governance actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for action in actions {
        let action_type = action.action.as_slice();

        if actions::matches(action_type, &actions::PROPOSAL_CREATED) {
            let event = debug_span!(
                "convert.governance.created",
                space_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_proposal_created(action, meta))?;
            result.proposals_created.push(event);
        } else if actions::matches(action_type, &actions::PROPOSAL_VOTED) {
            let event = debug_span!(
                "convert.governance.voted",
                voter_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_proposal_voted(action, meta))?;
            result.proposals_voted.push(event);
        } else if actions::matches(action_type, &actions::PROPOSAL_EXECUTED) {
            let event = debug_span!(
                "convert.governance.executed",
                space_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_proposal_executed(action, meta))?;
            result.proposals_executed.push(event);
        }
    }

    Ok(result)
}

/// Convert a PROPOSAL_CREATED action to HermesProposalCreated proto.
///
/// The action structure for PROPOSAL_CREATED:
/// - from_id: space_id (16 bytes) - space creating the proposal
/// - to_id: unused
/// - topic: proposal_id (32 bytes) - unique proposal identifier
/// - data: proposal metadata (title, description, voting period, etc.)
fn convert_proposal_created(action: &Action, meta: &BlockMetadata) -> Result<HermesProposalCreated> {
    Ok(HermesProposalCreated {
        space_id: action.from_id.clone(),
        proposal_id: action.topic.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

/// Convert a PROPOSAL_VOTED action to HermesProposalVoted proto.
///
/// The action structure for PROPOSAL_VOTED:
/// - from_id: voter_id (16 bytes) - space casting the vote
/// - to_id: space_id (16 bytes) - space that owns the proposal
/// - topic: proposal_id (32 bytes) - proposal being voted on
/// - data: vote choice and additional vote data
fn convert_proposal_voted(action: &Action, meta: &BlockMetadata) -> Result<HermesProposalVoted> {
    Ok(HermesProposalVoted {
        voter_id: action.from_id.clone(),
        space_id: action.to_id.clone(),
        proposal_id: action.topic.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

/// Convert a PROPOSAL_EXECUTED action to HermesProposalExecuted proto.
///
/// The action structure for PROPOSAL_EXECUTED:
/// - from_id: space_id (16 bytes) - space executing the proposal
/// - to_id: unused
/// - topic: proposal_id (32 bytes) - executed proposal identifier
/// - data: execution result/details
fn convert_proposal_executed(
    action: &Action,
    meta: &BlockMetadata,
) -> Result<HermesProposalExecuted> {
    Ok(HermesProposalExecuted {
        space_id: action.from_id.clone(),
        proposal_id: action.topic.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    #[test]
    fn test_convert_proposal_created() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::PROPOSAL_CREATED.to_vec(),
            topic: vec![2; 32],
            data: vec![3, 4, 5],
        };

        let result = convert_proposal_created(&action, &test_meta()).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.proposal_id, vec![2; 32]);
        assert_eq!(result.data, vec![3, 4, 5]);
        assert!(result.meta.is_some());
    }

    #[test]
    fn test_convert_proposal_voted() {
        let action = Action {
            from_id: vec![1; 16], // voter
            to_id: vec![2; 16],   // space
            action: actions::PROPOSAL_VOTED.to_vec(),
            topic: vec![3; 32],   // proposal
            data: vec![4, 5, 6],  // vote data
        };

        let result = convert_proposal_voted(&action, &test_meta()).unwrap();
        assert_eq!(result.voter_id, vec![1; 16]);
        assert_eq!(result.space_id, vec![2; 16]);
        assert_eq!(result.proposal_id, vec![3; 32]);
        assert_eq!(result.data, vec![4, 5, 6]);
    }

    #[test]
    fn test_convert_proposal_executed() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::PROPOSAL_EXECUTED.to_vec(),
            topic: vec![2; 32],
            data: vec![7, 8, 9],
        };

        let result = convert_proposal_executed(&action, &test_meta()).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.proposal_id, vec![2; 32]);
        assert_eq!(result.data, vec![7, 8, 9]);
    }

    #[test]
    fn test_transform_filters_actions() {
        let actions = vec![
            // Should be included
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            // Should be included
            Action {
                from_id: vec![3; 16],
                to_id: vec![4; 16],
                action: actions::PROPOSAL_VOTED.to_vec(),
                topic: vec![5; 32],
                data: vec![],
            },
            // Should be included
            Action {
                from_id: vec![6; 16],
                to_id: vec![],
                action: actions::PROPOSAL_EXECUTED.to_vec(),
                topic: vec![7; 32],
                data: vec![],
            },
            // Should NOT be included (different action type)
            Action {
                from_id: vec![8; 16],
                to_id: vec![9; 16],
                action: actions::SUBSPACE_ADDED.to_vec(),
                topic: vec![10; 32],
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.proposals_created.len(), 1);
        assert_eq!(result.proposals_voted.len(), 1);
        assert_eq!(result.proposals_executed.len(), 1);
        assert_eq!(result.total(), 3);
    }
}
