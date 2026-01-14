//! Pipeline: PROPOSAL_CREATED/UPDATED + PROPOSAL_SETTINGS_SELECTED → space.governance
//!
//! Squashes paired proposal events into single Kafka messages.
//!
//! The contract emits two events for each proposal:
//! 1. PROPOSAL_CREATED: Contains proposer_id, space_id, proposal_id, voting_mode, actions
//! 2. PROPOSAL_SETTINGS_SELECTED: Contains proposal settings (start/end dates, thresholds)
//!
//! These are squashed into a single HermesProposalCreated/HermesProposalUpdated. If either event is missing
//! for a given proposal_id, both events are discarded with an error log.

use std::collections::HashMap;

use anyhow::Result;
use hermes_instrumentation::{debug, debug_span, warn};

use hermes_relay::{Action, actions};
use hermes_schema::pb::governance::{
    AddEditorAction, AddMemberAction, FlagAction, HermesProposalCreated, HermesProposalExecuted,
    HermesProposalUpdated, HermesProposalVoted, ProposalAction, ProposalSettings,
    ProposalVoteOption, PublishAction, RemoveEditorAction, RemoveMemberAction, UnflagAction,
    UnflagEditorAction, UpdateVotingSettingsAction, VotingMode, proposal_action,
};

use crate::decode::{
    self, ProposalActionType, decode_address_arg, decode_flag_args, decode_publish_args,
    decode_voting_settings_args,
};

use super::BlockMetadata;

/// Result of transforming governance actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub proposals_created: Vec<HermesProposalCreated>,
    pub proposals_updated: Vec<HermesProposalUpdated>,
    pub proposals_voted: Vec<HermesProposalVoted>,
    pub proposals_executed: Vec<HermesProposalExecuted>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.proposals_created.len()
            + self.proposals_updated.len()
            + self.proposals_voted.len()
            + self.proposals_executed.len()
    }
}

/// Intermediate data for a PROPOSAL_CREATED event before squashing.
struct ProposalCreatedPending {
    space_id: Vec<u8>,
    proposer_id: Vec<u8>,
    proposal_id: Vec<u8>,
    voting_mode: VotingMode,
    actions: Vec<ProposalAction>,
    sequence: u32,
}

/// Intermediate data for a PROPOSAL_SETTINGS_SELECTED event before squashing.
struct ProposalSettingsPending {
    settings: ProposalSettings,
}

/// Transform all governance actions in a block.
///
/// Squashes PROPOSAL_CREATED + PROPOSAL_SETTINGS_SELECTED pairs into single events.
/// For each PROPOSAL_CREATED, looks for a matching PROPOSAL_SETTINGS_SELECTED
/// with the same proposal_id. If not found, both events are discarded.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    // Collect PROPOSAL_CREATED/UPDATED and PROPOSAL_SETTINGS_SELECTED by proposal_id
    let mut created_map: HashMap<Vec<u8>, ProposalCreatedPending> = HashMap::new();
    let mut updated_map: HashMap<Vec<u8>, ProposalCreatedPending> = HashMap::new();
    let mut settings_map: HashMap<Vec<u8>, ProposalSettingsPending> = HashMap::new();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::PROPOSAL_CREATED) {
            if let Some(pending) = debug_span!(
                "parse.governance.created",
                proposer_id = %hex::encode(&action.from_id),
                space_id = %hex::encode(&action.to_id)
            )
            .in_scope(|| parse_proposal_created(action, sequence))
            {
                let proposal_id = pending.proposal_id.clone();
                created_map.insert(proposal_id, pending);
            }
        } else if actions::matches(action_type, &actions::PROPOSAL_UPDATED) {
            if let Some(pending) = debug_span!(
                "parse.governance.updated",
                proposer_id = %hex::encode(&action.from_id),
                space_id = %hex::encode(&action.to_id)
            )
            .in_scope(|| parse_proposal_created(action, sequence))
            {
                let proposal_id = pending.proposal_id.clone();
                updated_map.insert(proposal_id, pending);
            }
        } else if actions::matches(action_type, &actions::PROPOSAL_SETTINGS_SELECTED) {
            if let Some(pending) = debug_span!(
                "parse.governance.settings",
                proposal_id = %hex::encode(&action.topic[..16])
            )
            .in_scope(|| parse_proposal_settings_used(action))
            {
                // proposal_id is in topic field (bytes16 right-padded to 32, so first 16 bytes)
                let proposal_id = action.topic[..16].to_vec();
                settings_map.insert(proposal_id, pending);
            }
        } else if actions::matches(action_type, &actions::PROPOSAL_VOTED) {
            let event = debug_span!(
                "convert.governance.voted",
                voter_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic[..16])
            )
            .in_scope(|| convert_proposal_voted(action, meta, sequence))?;
            result.proposals_voted.push(event);
        } else if actions::matches(action_type, &actions::PROPOSAL_EXECUTED) {
            let event = debug_span!(
                "convert.governance.executed",
                space_id = %hex::encode(&action.from_id),
                proposal_id = %hex::encode(&action.topic[..16])
            )
            .in_scope(|| convert_proposal_executed(action, meta, sequence))?;
            result.proposals_executed.push(event);
        }
    }

    // Squash PROPOSAL_CREATED with PROPOSAL_SETTINGS_SELECTED
    for (proposal_id, created) in created_map {
        if let Some(settings_pending) = settings_map.remove(&proposal_id) {
            // Found matching pair - emit squashed event
            let event = HermesProposalCreated {
                space_id: created.space_id,
                proposer_id: created.proposer_id,
                proposal_id: created.proposal_id,
                voting_mode: created.voting_mode as i32,
                actions: created.actions,
                settings: Some(settings_pending.settings),
                meta: Some(meta.to_proto(created.sequence)),
            };
            result.proposals_created.push(event);
        } else {
            // Missing PROPOSAL_SETTINGS_SELECTED - log error and discard
            warn!(
                proposal_id = %hex::encode(&proposal_id),
                "PROPOSAL_CREATED without matching PROPOSAL_SETTINGS_SELECTED, discarding"
            );
        }
    }

    // Squash PROPOSAL_UPDATED with PROPOSAL_SETTINGS_SELECTED
    for (proposal_id, updated) in updated_map {
        if let Some(settings_pending) = settings_map.remove(&proposal_id) {
            let event = HermesProposalUpdated {
                space_id: updated.space_id,
                proposer_id: updated.proposer_id,
                proposal_id: updated.proposal_id,
                voting_mode: updated.voting_mode as i32,
                actions: updated.actions,
                settings: Some(settings_pending.settings),
                meta: Some(meta.to_proto(updated.sequence)),
            };
            result.proposals_updated.push(event);
        } else {
            warn!(
                proposal_id = %hex::encode(&proposal_id),
                "PROPOSAL_UPDATED without matching PROPOSAL_SETTINGS_SELECTED, discarding"
            );
        }
    }

    // Log any orphaned PROPOSAL_SETTINGS_SELECTED events
    for (proposal_id, _) in settings_map {
        warn!(
            proposal_id = %hex::encode(&proposal_id),
            "PROPOSAL_SETTINGS_SELECTED without matching PROPOSAL_CREATED/UPDATED, discarding"
        );
    }

    Ok(result)
}

/// Decode proposal action calldata into the appropriate oneof variant.
fn decode_proposal_action(
    action_type: &ProposalActionType,
    calldata: &[u8],
) -> Option<proposal_action::Action> {
    match action_type {
        ProposalActionType::AddMember => {
            let addr = decode_address_arg(calldata)?;
            Some(proposal_action::Action::AddMember(AddMemberAction {
                target_address: addr,
            }))
        }
        ProposalActionType::RemoveMember => {
            let addr = decode_address_arg(calldata)?;
            Some(proposal_action::Action::RemoveMember(RemoveMemberAction {
                target_address: addr,
            }))
        }
        ProposalActionType::AddEditor => {
            let addr = decode_address_arg(calldata)?;
            Some(proposal_action::Action::AddEditor(AddEditorAction {
                target_address: addr,
            }))
        }
        ProposalActionType::RemoveEditor => {
            let addr = decode_address_arg(calldata)?;
            Some(proposal_action::Action::RemoveEditor(RemoveEditorAction {
                target_address: addr,
            }))
        }
        ProposalActionType::UnrestrictSpace => {
            let addr = decode_address_arg(calldata)?;
            Some(proposal_action::Action::UnflagEditor(UnflagEditorAction {
                target_address: addr,
            }))
        }
        ProposalActionType::Publish => {
            let args = decode_publish_args(calldata).ok()?;
            Some(proposal_action::Action::Publish(PublishAction {
                content_uri: args.content_uri,
                metadata: args.metadata,
            }))
        }
        ProposalActionType::Flag => {
            let args = decode_flag_args(calldata).ok()?;
            Some(proposal_action::Action::Flag(FlagAction {
                content_id: args.content_id,
            }))
        }
        ProposalActionType::Unflag => {
            let args = decode_flag_args(calldata).ok()?;
            Some(proposal_action::Action::Unflag(UnflagAction {
                content_id: args.content_id,
            }))
        }
        ProposalActionType::UpdateVotingSettings => {
            let args = decode_voting_settings_args(calldata).ok()?;
            Some(proposal_action::Action::UpdateVotingSettings(
                UpdateVotingSettingsAction {
                    quorum: args.quorum,
                    fast_threshold: args.fast_threshold,
                    slow_threshold: args.slow_threshold,
                    duration: args.duration,
                },
            ))
        }
        ProposalActionType::Ping | ProposalActionType::Unknown => None,
    }
}

/// Parse a PROPOSAL_CREATED action into pending data.
///
/// The action structure for PROPOSAL_CREATED:
/// - from_id: proposer_id (16 bytes) - space creating the proposal
/// - to_id: space_id (16 bytes) - space owning the proposal
/// - topic: proposal_id (16 bytes, padded to 32)
/// - data: abi.encode(bytes16 proposalId, VotingMode, Action[])
fn parse_proposal_created(action: &Action, sequence: u32) -> Option<ProposalCreatedPending> {
    let (decoded, unwrap_level) = match decode::decode_proposal_created(&action.data) {
        Ok(decoded) => decoded,
        Err(e) => {
            let debug_chain = decode::unwrap_debug_chain(&action.data, 2);
            let mut levels: Vec<String> = Vec::new();
            for (idx, buf) in debug_chain.iter().enumerate() {
                let prefix_len = buf.len().min(64);
                let prefix = hex::encode(&buf[..prefix_len]);
                levels.push(format!("L{idx}:len={} prefix={}", buf.len(), prefix));
            }
            warn!(
                error = %e,
                proposal_id = %hex::encode(&action.topic[..16]),
                data_len = action.data.len(),
                data = %hex::encode(&action.data),
                unwrap_chain = %levels.join(" | "),
                "Failed to decode proposal created data"
            );
            return None;
        }
    };
    if unwrap_level > 0 {
        debug!(
            unwrap_level,
            proposal_id = %hex::encode(&action.topic[..16]),
            "Unwrapped proposal created data"
        );
    }

    let voting_mode = match decoded.voting_mode {
        0 => VotingMode::Slow,
        1 => VotingMode::Fast,
        _ => {
            warn!(
                voting_mode = decoded.voting_mode,
                proposal_id = %hex::encode(&action.topic[..16]),
                data_len = action.data.len(),
                data = %hex::encode(&action.data),
                "Invalid voting mode in proposal created"
            );
            return None;
        }
    };

    // Convert decoded actions to proto format
    let proto_actions: Vec<ProposalAction> = decoded
        .actions
        .into_iter()
        .map(|a| {
            let action_type = ProposalActionType::from_calldata(&a.data);
            let decoded_action = decode_proposal_action(&action_type, &a.data);

            debug!(
                action_type = ?action_type,
                target = %hex::encode(&a.to),
                "Decoded proposal action"
            );

            ProposalAction {
                to: a.to,
                value: a.value,
                data: a.data,
                action: decoded_action,
            }
        })
        .collect();

    Some(ProposalCreatedPending {
        space_id: action.to_id.clone(),
        proposer_id: action.from_id.clone(),
        proposal_id: decoded.proposal_id,
        voting_mode,
        actions: proto_actions,
        sequence,
    })
}

/// Parse a PROPOSAL_SETTINGS_SELECTED action into pending data.
///
/// The action structure for PROPOSAL_SETTINGS_SELECTED:
/// - from_id: space_id (16 bytes)
/// - to_id: space_id (16 bytes, same as from_id)
/// - topic: proposal_id (16 bytes, padded to 32)
/// - data: abi.encode(startDate, lastDate, votingMode, quorum, supportThreshold)
fn parse_proposal_settings_used(action: &Action) -> Option<ProposalSettingsPending> {
    let decoded = match decode::decode_proposal_settings_used(&action.data) {
        Ok(decoded) => decoded,
        Err(e) => {
            let debug_chain = decode::unwrap_debug_chain(&action.data, 2);
            let mut levels: Vec<String> = Vec::new();
            for (idx, buf) in debug_chain.iter().enumerate() {
                let prefix_len = buf.len().min(64);
                let prefix = hex::encode(&buf[..prefix_len]);
                levels.push(format!("L{idx}:len={} prefix={}", buf.len(), prefix));
            }
            warn!(
                error = %e,
                proposal_id = %hex::encode(&action.topic[..16]),
                data_len = action.data.len(),
                data = %hex::encode(&action.data),
                unwrap_chain = %levels.join(" | "),
                "Failed to decode proposal settings used data"
            );
            return None;
        }
    };

    // Determine threshold type based on voting mode
    // Slow=0 uses percentage threshold, Fast=1 uses flat threshold
    let (flat_threshold, percentage_threshold) = match decoded.voting_mode {
        0 => (0, decoded.support_threshold), // Slow path uses percentage threshold
        1 => (decoded.support_threshold, 0), // Fast path uses flat threshold
        _ => {
            let debug_chain = decode::unwrap_debug_chain(&action.data, 2);
            let mut levels: Vec<String> = Vec::new();
            for (idx, buf) in debug_chain.iter().enumerate() {
                let prefix_len = buf.len().min(64);
                let prefix = hex::encode(&buf[..prefix_len]);
                levels.push(format!("L{idx}:len={} prefix={}", buf.len(), prefix));
            }
            warn!(
                voting_mode = decoded.voting_mode,
                proposal_id = %hex::encode(&action.topic[..16]),
                data_len = action.data.len(),
                data = %hex::encode(&action.data),
                unwrap_chain = %levels.join(" | "),
                "Invalid voting mode in proposal settings"
            );
            return None;
        }
    };

    let voting_mode = match decoded.voting_mode {
        0 => VotingMode::Slow,
        1 => VotingMode::Fast,
        _ => unreachable!(), // Already handled above
    };

    Some(ProposalSettingsPending {
        settings: ProposalSettings {
            start_date: decoded.start_date,
            last_date: decoded.last_date,
            voting_mode: voting_mode as i32,
            quorum: decoded.quorum,
            flat_threshold,
            percentage_threshold,
        },
    })
}

/// Convert a PROPOSAL_VOTED action to HermesProposalVoted proto.
///
/// The action structure for PROPOSAL_VOTED:
/// - from_id: voter_id (16 bytes) - space casting the vote
/// - to_id: space_id (16 bytes) - space that owns the proposal
/// - topic: proposal_id (16 bytes, padded to 32)
/// - data: abi.encode(bytes16 proposalId, VoteOption)
fn convert_proposal_voted(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesProposalVoted> {
    // Decode the data field
    // VoteOption enum (IDAOSpace): None=0, Yes=1, No=2, Abstain=3
    let vote = match decode::decode_proposal_voted(&action.data) {
        Ok(decoded) => match decoded.vote {
            0 => ProposalVoteOption::VoteOptionNone,
            1 => ProposalVoteOption::VoteOptionYes,
            2 => ProposalVoteOption::VoteOptionNo,
            3 => ProposalVoteOption::VoteOptionAbstain,
            _ => ProposalVoteOption::VoteOptionNone,
        },
        Err(e) => {
            warn!(
                error = %e,
                proposal_id = %hex::encode(&action.topic[..16]),
                "Failed to decode proposal voted data"
            );
            ProposalVoteOption::VoteOptionNone
        }
    };

    // proposal_id is in topic field (bytes16 right-padded to 32, so it's in first 16 bytes)
    let proposal_id = action.topic[..16].to_vec();

    Ok(HermesProposalVoted {
        voter_id: action.from_id.clone(),
        space_id: action.to_id.clone(),
        proposal_id,
        vote: vote as i32,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a PROPOSAL_EXECUTED action to HermesProposalExecuted proto.
///
/// The action structure for PROPOSAL_EXECUTED:
/// - from_id: space_id (16 bytes) - space executing the proposal
/// - to_id: space_id (16 bytes, same as from_id)
/// - topic: proposal_id (16 bytes, padded to 32)
/// - data: empty or abi.encode(bytes16 proposalId)
fn convert_proposal_executed(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesProposalExecuted> {
    // proposal_id is in topic field (bytes16 right-padded to 32, so it's in first 16 bytes)
    let proposal_id = action.topic[..16].to_vec();

    Ok(HermesProposalExecuted {
        space_id: action.from_id.clone(),
        proposal_id,
        meta: Some(meta.to_proto(sequence)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, FixedBytes, U256};
    use alloy::sol;
    use alloy::sol_types::SolType;

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    // Helper to create encoded PROPOSAL_CREATED data
    fn encode_proposal_created_data(proposal_id: [u8; 16], voting_mode: u8) -> Vec<u8> {
        sol! {
            struct TestAction {
                address to;
                uint256 value;
                bytes data;
            }
        }
        type ProposalCreatedDataType = sol! { (bytes16, uint8, TestAction[]) };

        let action = TestAction {
            to: Address::ZERO,
            value: U256::ZERO,
            data: vec![].into(),
        };

        ProposalCreatedDataType::abi_encode(&(
            FixedBytes::<16>::from_slice(&proposal_id),
            voting_mode,
            vec![action],
        ))
    }

    // Helper to create encoded PROPOSAL_SETTINGS_SELECTED data
    fn encode_proposal_settings_data(
        start_date: u64,
        last_date: u64,
        voting_mode: u8,
        quorum: u64,
        threshold: u64,
    ) -> Vec<u8> {
        type SettingsDataType = sol! { (uint64, uint64, uint8, uint256, uint256) };
        SettingsDataType::abi_encode(&(
            start_date,
            last_date,
            voting_mode,
            U256::from(quorum),
            U256::from(threshold),
        ))
    }

    #[test]
    fn test_squash_proposal_created_with_settings() {
        let proposal_id = [0xAB; 16];
        let proposer_id = vec![1; 16];
        let space_id = vec![2; 16];

        let test_actions = vec![
            // PROPOSAL_CREATED
            // Topic: proposal_id in first 16 bytes, zeros in last 16 (right-padded)
            Action {
                from_id: proposer_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_created_data(proposal_id, 1), // Fast path (1)
            },
            // PROPOSAL_SETTINGS_SELECTED (must have matching proposal_id)
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 1, 100, 50),
            },
        ];

        let result = transform(&test_actions, &test_meta()).unwrap();

        // Should have 1 squashed event
        assert_eq!(result.proposals_created.len(), 1);

        let event = &result.proposals_created[0];
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.proposer_id, proposer_id);
        assert_eq!(event.proposal_id, proposal_id.to_vec());
        assert_eq!(event.voting_mode, VotingMode::Fast as i32);

        // Verify settings were included
        let settings = event.settings.as_ref().unwrap();
        assert_eq!(settings.start_date, 1000);
        assert_eq!(settings.last_date, 2000);
        assert_eq!(settings.flat_threshold, 50); // Fast path uses flat threshold
    }

    #[test]
    fn test_proposal_created_without_settings_discarded() {
        let proposal_id = [0xCD; 16];

        let test_actions = vec![
            // PROPOSAL_CREATED without matching PROPOSAL_SETTINGS_SELECTED
            // Topic: proposal_id in first 16 bytes, zeros in last 16 (right-padded)
            Action {
                from_id: vec![1; 16],
                to_id: vec![2; 16],
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_created_data(proposal_id, 0), // Slow path (0)
            },
        ];

        let result = transform(&test_actions, &test_meta()).unwrap();

        // Should be discarded - no matching settings
        assert_eq!(result.proposals_created.len(), 0);
    }

    #[test]
    fn test_proposal_settings_without_created_discarded() {
        let proposal_id = [0xEF; 16];

        let test_actions = vec![
            // PROPOSAL_SETTINGS_SELECTED without matching PROPOSAL_CREATED
            // Topic: proposal_id in first 16 bytes, zeros in last 16 (right-padded)
            Action {
                from_id: vec![1; 16],
                to_id: vec![1; 16],
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 0, 100, 50), // Slow path (0)
            },
        ];

        let result = transform(&test_actions, &test_meta()).unwrap();

        // Should be discarded - no matching created
        assert_eq!(result.proposals_created.len(), 0);
    }

    #[test]
    fn test_mismatched_proposal_ids_both_discarded() {
        let proposal_id_1 = [0x11; 16];
        let proposal_id_2 = [0x22; 16];

        let test_actions = vec![
            // PROPOSAL_CREATED with one ID
            // Topic: proposal_id in first 16 bytes, zeros in last 16 (right-padded)
            Action {
                from_id: vec![1; 16],
                to_id: vec![2; 16],
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id_1.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_created_data(proposal_id_1, 0),
            },
            // PROPOSAL_SETTINGS_SELECTED with different ID
            Action {
                from_id: vec![2; 16],
                to_id: vec![2; 16],
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id_2.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 0, 100, 50),
            },
        ];

        let result = transform(&test_actions, &test_meta()).unwrap();

        // Both should be discarded - IDs don't match
        assert_eq!(result.proposals_created.len(), 0);
    }

    #[test]
    fn test_convert_proposal_voted_empty_data() {
        // Empty data should default to None vote
        // proposal_id in first 16 bytes (right-padded to 32)
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: actions::PROPOSAL_VOTED.to_vec(),
            topic: vec![3; 16].into_iter().chain(vec![0; 16]).collect(),
            data: vec![],
        };

        let result = convert_proposal_voted(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.voter_id, vec![1; 16]);
        assert_eq!(result.space_id, vec![2; 16]);
        assert_eq!(result.proposal_id, vec![3; 16]);
        // Default vote when decode fails
        assert_eq!(result.vote, ProposalVoteOption::VoteOptionNone as i32);
    }

    #[test]
    fn test_convert_proposal_executed() {
        // proposal_id in first 16 bytes (right-padded to 32)
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![1; 16],
            action: actions::PROPOSAL_EXECUTED.to_vec(),
            topic: vec![2; 16].into_iter().chain(vec![0; 16]).collect(),
            data: vec![],
        };

        let result = convert_proposal_executed(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.proposal_id, vec![2; 16]);
    }

    #[test]
    fn test_transform_filters_actions() {
        let test_actions = vec![
            // PROPOSAL_VOTED
            // Topic: proposal_id in first 16 bytes, zeros in last 16 (right-padded)
            Action {
                from_id: vec![3; 16],
                to_id: vec![4; 16],
                action: actions::PROPOSAL_VOTED.to_vec(),
                topic: vec![5; 16].into_iter().chain(vec![0; 16]).collect(),
                data: vec![],
            },
            // PROPOSAL_EXECUTED
            Action {
                from_id: vec![6; 16],
                to_id: vec![6; 16],
                action: actions::PROPOSAL_EXECUTED.to_vec(),
                topic: vec![7; 16].into_iter().chain(vec![0; 16]).collect(),
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

        let result = transform(&test_actions, &test_meta()).unwrap();
        // No PROPOSAL_CREATED events without matching PROPOSAL_SETTINGS_USED
        assert_eq!(result.proposals_created.len(), 0);
        assert_eq!(result.proposals_voted.len(), 1);
        assert_eq!(result.proposals_executed.len(), 1);
    }
}
