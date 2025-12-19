//! Pipeline: PROPOSAL_CREATED, PROPOSAL_VOTED, PROPOSAL_EXECUTED → space.governance
//!
//! Converts governance actions to typed Hermes events with decoded data.
//!
//! For PROPOSAL_CREATED, one event is emitted per action in the proposal. This allows
//! consumers to understand the proposal type(s) without decoding calldata themselves.
//! Proposals with multiple actions (slow path only) emit multiple events with the same
//! proposal_id but different action_index values.

use anyhow::Result;
use hermes_instrumentation::{debug, debug_span, warn};

use hermes_relay::{Action, actions};
use hermes_schema::pb::governance::{
    HermesProposalCreated, HermesProposalExecuted, HermesProposalVoted, ProposalAction,
    ProposalVoteOption, VotingMode,
};

use crate::decode::{self, ProposalActionType, decode_address_arg};

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
        self.proposals_created.len() + self.proposals_voted.len() + self.proposals_executed.len()
    }
}

/// Transform all governance actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::PROPOSAL_CREATED) {
            let events = debug_span!(
                "convert.governance.created",
                space_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_proposal_created(action, meta, sequence))?;
            result.proposals_created.extend(events);
        } else if actions::matches(action_type, &actions::PROPOSAL_VOTED) {
            let event = debug_span!(
                "convert.governance.voted",
                voter_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_proposal_voted(action, meta, sequence))?;
            result.proposals_voted.push(event);
        } else if actions::matches(action_type, &actions::PROPOSAL_EXECUTED) {
            let event = debug_span!(
                "convert.governance.executed",
                space_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_proposal_executed(action, meta, sequence))?;
            result.proposals_executed.push(event);
        }
    }

    Ok(result)
}

/// Convert a PROPOSAL_CREATED action to HermesProposalCreated proto events.
///
/// Emits one event per action in the proposal. For proposals with multiple actions,
/// multiple events are emitted with the same proposal_id but different action_index values.
///
/// The action structure for PROPOSAL_CREATED:
/// - from_id: space_id (16 bytes) - space creating the proposal
/// - to_id: unused
/// - topic: proposal_id (32 bytes) - unique proposal identifier
/// - data: abi.encode(VotingMode, Action[])
fn convert_proposal_created(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<Vec<HermesProposalCreated>> {
    // Decode the data field
    let decoded = match decode::decode_proposal_created(&action.data) {
        Ok(decoded) => decoded,
        Err(e) => {
            warn!(
                error = %e,
                proposal_id = %hex::encode(&action.topic),
                "Failed to decode proposal created data"
            );
            // Return empty event with unknown action type
            return Ok(vec![HermesProposalCreated {
                space_id: action.from_id.clone(),
                proposal_id: action.topic.clone(),
                voting_mode: VotingMode::Fast as i32,
                action_index: 0,
                action_count: 0,
                action_type: ProposalActionType::Unknown.to_proto_value(),
                action: None,
                meta: Some(meta.to_proto(sequence)),
            }]);
        }
    };

    let voting_mode = match decoded.voting_mode {
        0 => VotingMode::Fast,
        1 => VotingMode::Slow,
        _ => VotingMode::Fast,
    };

    let action_count = decoded.actions.len() as u32;

    // Handle empty actions (shouldn't happen but be defensive)
    if decoded.actions.is_empty() {
        return Ok(vec![HermesProposalCreated {
            space_id: action.from_id.clone(),
            proposal_id: action.topic.clone(),
            voting_mode: voting_mode as i32,
            action_index: 0,
            action_count: 0,
            action_type: ProposalActionType::Unknown.to_proto_value(),
            action: None,
            meta: Some(meta.to_proto(sequence)),
        }]);
    }

    // Emit one event per action
    let events = decoded
        .actions
        .into_iter()
        .enumerate()
        .map(|(idx, a)| {
            let action_type = ProposalActionType::from_calldata(&a.data);
            let selector = if a.data.len() >= 4 {
                hex::encode(&a.data[0..4])
            } else {
                "none".to_string()
            };

            // Decode address argument if this action type takes one
            let target_address = if action_type.has_address_arg() {
                decode_address_arg(&a.data).unwrap_or_default()
            } else {
                vec![]
            };

            debug!(
                action_index = idx,
                action_count = action_count,
                selector = %selector,
                action_type = ?action_type,
                target = %hex::encode(&a.to),
                target_address = %hex::encode(&target_address),
                "Decoded proposal action"
            );

            HermesProposalCreated {
                space_id: action.from_id.clone(),
                proposal_id: action.topic.clone(),
                voting_mode: voting_mode as i32,
                action_index: idx as u32,
                action_count,
                action_type: action_type.to_proto_value(),
                action: Some(ProposalAction {
                    to: a.to,
                    value: a.value,
                    data: a.data,
                    target_address,
                }),
                meta: Some(meta.to_proto(sequence)),
            }
        })
        .collect();

    Ok(events)
}

/// Convert a PROPOSAL_VOTED action to HermesProposalVoted proto.
///
/// The action structure for PROPOSAL_VOTED:
/// - from_id: voter_id (16 bytes) - space casting the vote
/// - to_id: space_id (16 bytes) - space that owns the proposal
/// - topic: proposal_id (32 bytes) - proposal being voted on
/// - data: abi.encode(uint256(proposalId), VoteOption)
fn convert_proposal_voted(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesProposalVoted> {
    // Decode the data field
    // VoteOption enum: None=0, Yes=1, No=2, Abstain=3
    let vote = match decode::decode_proposal_voted(&action.data) {
        Ok(decoded) => match decoded.vote {
            0 => ProposalVoteOption::None,
            1 => ProposalVoteOption::Yes,
            2 => ProposalVoteOption::No,
            3 => ProposalVoteOption::Abstain,
            _ => ProposalVoteOption::None,
        },
        Err(e) => {
            warn!(
                error = %e,
                proposal_id = %hex::encode(&action.topic),
                "Failed to decode proposal voted data"
            );
            ProposalVoteOption::None
        }
    };

    Ok(HermesProposalVoted {
        voter_id: action.from_id.clone(),
        space_id: action.to_id.clone(),
        proposal_id: action.topic.clone(),
        vote: vote as i32,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a PROPOSAL_EXECUTED action to HermesProposalExecuted proto.
///
/// The action structure for PROPOSAL_EXECUTED:
/// - from_id: space_id (16 bytes) - space executing the proposal
/// - to_id: unused
/// - topic: proposal_id (32 bytes) - executed proposal identifier
/// - data: abi.encode(uint256(proposalId)) - redundant, not needed
fn convert_proposal_executed(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesProposalExecuted> {
    Ok(HermesProposalExecuted {
        space_id: action.from_id.clone(),
        proposal_id: action.topic.clone(),
        meta: Some(meta.to_proto(sequence)),
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
    fn test_convert_proposal_created_empty_data() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::PROPOSAL_CREATED.to_vec(),
            topic: vec![2; 32],
            data: vec![],
        };

        let results = convert_proposal_created(&action, &test_meta(), 0).unwrap();
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.proposal_id, vec![2; 32]);
        assert_eq!(result.action_count, 0);
        assert_eq!(result.voting_mode, VotingMode::Fast as i32);
    }

    #[test]
    fn test_convert_proposal_voted_empty_data() {
        // Empty data should default to None vote
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: actions::PROPOSAL_VOTED.to_vec(),
            topic: vec![3; 32],
            data: vec![],
        };

        let result = convert_proposal_voted(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.voter_id, vec![1; 16]);
        assert_eq!(result.space_id, vec![2; 16]);
        assert_eq!(result.proposal_id, vec![3; 32]);
        // Default vote when decode fails
        assert_eq!(result.vote, ProposalVoteOption::None as i32);
    }

    #[test]
    fn test_convert_proposal_executed() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::PROPOSAL_EXECUTED.to_vec(),
            topic: vec![2; 32],
            data: vec![],
        };

        let result = convert_proposal_executed(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.proposal_id, vec![2; 32]);
    }

    #[test]
    fn test_transform_filters_actions() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![4; 16],
                action: actions::PROPOSAL_VOTED.to_vec(),
                topic: vec![5; 32],
                data: vec![],
            },
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

    #[test]
    fn test_convert_proposal_created_with_add_member_action() {
        use alloy::primitives::{Address, U256};
        use alloy::sol;
        use alloy::sol_types::SolType;

        // Define the Action struct for encoding
        sol! {
            struct TestAction {
                address to;
                uint256 value;
                bytes data;
            }
        }

        // Create ABI-encoded data for a proposal with addMember(address) action
        let member_address: [u8; 20] = [0xAA; 20];

        // Build the calldata for addMember(address)
        let mut calldata = decode::selectors::ADD_MEMBER.to_vec();
        calldata.extend_from_slice(&[0u8; 12]); // padding
        calldata.extend_from_slice(&member_address);

        // Build the Action struct
        let action_data = TestAction {
            to: Address::ZERO,
            value: U256::ZERO,
            data: calldata.into(),
        };

        // Encode: (uint8 votingMode, Action[])
        type ProposalCreatedDataType = sol! { (uint8, TestAction[]) };
        let encoded = ProposalCreatedDataType::abi_encode(&(0u8, vec![action_data]));

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::PROPOSAL_CREATED.to_vec(),
            topic: vec![2; 32],
            data: encoded,
        };

        let results = convert_proposal_created(&action, &test_meta(), 0).unwrap();
        assert_eq!(results.len(), 1);

        let result = &results[0];
        assert_eq!(
            result.action_type,
            decode::ProposalActionType::AddMember.to_proto_value()
        );
        assert_eq!(result.action_count, 1);
        assert_eq!(result.action_index, 0);
        assert_eq!(result.voting_mode, VotingMode::Fast as i32);

        // Verify the target_address was decoded
        let proposal_action = result.action.as_ref().unwrap();
        assert_eq!(proposal_action.target_address, member_address.to_vec());
    }
}
