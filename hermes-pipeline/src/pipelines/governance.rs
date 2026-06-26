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
use hermes_instrumentation::{debug, debug_span, info, warn};

use crate::cache::CachedEdit;
use crate::decode::{
    self, ProposalActionType, decode_flag_args, decode_ping_args, decode_publish_args,
    decode_space_id_arg, decode_voting_settings_args,
};

use hermes_relay::{Action, actions};
use hermes_schema::pb::governance::{
    AddEditorAction, AddMemberAction, FlagAction, HermesProposalCreated, HermesProposalExecuted,
    HermesProposalSettingsUpdated, HermesProposalUpdated, HermesProposalVoted,
    HermesVotingSettingsUpdated, ProposalAction, ProposalSettings, ProposalVoteOption,
    PublishAction, RemoveEditorAction, RemoveMemberAction, SetTopicAction, SubspaceEdgeAction,
    SubspaceTopicAction, UnflagAction, UnflagEditorAction, UnsetTopicAction,
    UpdateVotingSettingsAction, VotingMode, proposal_action,
};

use super::BlockMetadata;

/// Result of transforming governance actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub proposals_created: Vec<HermesProposalCreated>,
    pub proposals_updated: Vec<HermesProposalUpdated>,
    pub proposals_voted: Vec<HermesProposalVoted>,
    pub proposals_executed: Vec<HermesProposalExecuted>,
    pub proposals_settings_updated: Vec<HermesProposalSettingsUpdated>,
    pub voting_settings_updated: Vec<HermesVotingSettingsUpdated>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.proposals_created.len()
            + self.proposals_updated.len()
            + self.proposals_voted.len()
            + self.proposals_executed.len()
            + self.proposals_settings_updated.len()
            + self.voting_settings_updated.len()
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
    space_id: Vec<u8>,
    sequence: u32,
    settings: ProposalSettings,
}

/// Transform all governance actions in a block.
///
/// Squashes PROPOSAL_CREATED + PROPOSAL_SETTINGS_SELECTED pairs into single events.
/// For each PROPOSAL_CREATED, looks for a matching PROPOSAL_SETTINGS_SELECTED
/// with the same proposal_id. If not found, both events are discarded.
///
/// For Publish actions, looks up the edit name from the prefetched IPFS cache.
pub fn transform(
    actions: &[Action],
    meta: &BlockMetadata,
    prefetched: &HashMap<String, CachedEdit>,
) -> Result<TransformResult> {
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
            .in_scope(|| parse_proposal_settings_used(action, sequence))
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
        } else if actions::matches(action_type, &actions::VOTING_SETTINGS_UPDATED)
            && let Some(event) = debug_span!(
                "convert.governance.voting_settings_updated",
                space_id = %hex::encode(&action.from_id)
            )
            .in_scope(|| convert_voting_settings_updated(action, meta, sequence))
        {
            result.voting_settings_updated.push(event);
        }
    }

    // Squash PROPOSAL_CREATED with PROPOSAL_SETTINGS_SELECTED
    for (proposal_id, created) in created_map {
        if let Some(settings_pending) = settings_map.remove(&proposal_id) {
            // Log proposal with action types for debugging
            let proposal_uuid = uuid::Uuid::from_slice(&proposal_id)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| hex::encode(&proposal_id));
            let action_types: Vec<String> = created
                .actions
                .iter()
                .map(|a| {
                    let action_type = crate::decode::ProposalActionType::from_calldata(&a.data);
                    format!("{:?}", action_type)
                })
                .collect();
            info!(
                proposal_id = %proposal_uuid,
                action_count = created.actions.len(),
                action_types = ?action_types,
                "Processing PROPOSAL_CREATED"
            );

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
            // Log proposal with action types for debugging
            let proposal_uuid = uuid::Uuid::from_slice(&proposal_id)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| hex::encode(&proposal_id));
            let action_types: Vec<String> = updated
                .actions
                .iter()
                .map(|a| {
                    let action_type = crate::decode::ProposalActionType::from_calldata(&a.data);
                    format!("{:?}", action_type)
                })
                .collect();
            info!(
                proposal_id = %proposal_uuid,
                action_count = updated.actions.len(),
                action_types = ?action_types,
                "Processing PROPOSAL_UPDATED"
            );

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

    // Emit orphaned PROPOSAL_SETTINGS_SELECTED as settings-only updates.
    // This handles fast-path → slow-path escalation: when a NO vote on a fast-path
    // proposal triggers escalation, the contract emits PROPOSAL_SETTINGS_SELECTED
    // (via ping) but not PROPOSAL_UPDATED, so the settings have no created/updated
    // event to squash with.
    for (proposal_id, settings_pending) in settings_map {
        info!(
            proposal_id = %hex::encode(&proposal_id),
            "Orphaned PROPOSAL_SETTINGS_SELECTED — emitting as settings update (fast→slow escalation)"
        );
        result
            .proposals_settings_updated
            .push(HermesProposalSettingsUpdated {
                space_id: settings_pending.space_id,
                proposal_id,
                settings: Some(settings_pending.settings),
                meta: Some(meta.to_proto(settings_pending.sequence)),
            });
    }

    // Enrich all Publish actions with edit names from the prefetched cache
    enrich_publish_action_names(
        &mut result.proposals_created,
        &mut result.proposals_updated,
        prefetched,
    );

    Ok(result)
}

/// Decode proposal action calldata into the appropriate oneof variant.
fn decode_proposal_action(
    action_type: &ProposalActionType,
    calldata: &[u8],
) -> Option<proposal_action::Action> {
    match action_type {
        ProposalActionType::AddMember => {
            let space_id = decode_space_id_arg(calldata)?;
            Some(proposal_action::Action::AddMember(AddMemberAction {
                target_address: space_id,
            }))
        }
        ProposalActionType::RemoveMember => {
            let space_id = decode_space_id_arg(calldata)?;
            Some(proposal_action::Action::RemoveMember(RemoveMemberAction {
                target_address: space_id,
            }))
        }
        ProposalActionType::AddEditor => {
            let space_id = decode_space_id_arg(calldata)?;
            Some(proposal_action::Action::AddEditor(AddEditorAction {
                target_address: space_id,
            }))
        }
        ProposalActionType::RemoveEditor => {
            let space_id = decode_space_id_arg(calldata)?;
            Some(proposal_action::Action::RemoveEditor(RemoveEditorAction {
                target_address: space_id,
            }))
        }
        ProposalActionType::UnrestrictSpace => {
            let space_id = decode_space_id_arg(calldata)?;
            Some(proposal_action::Action::UnflagEditor(UnflagEditorAction {
                target_address: space_id,
            }))
        }
        ProposalActionType::Publish => {
            let args = decode_publish_args(calldata).ok()?;
            // Note: name is populated later by enrich_publish_action_names
            Some(proposal_action::Action::Publish(PublishAction {
                content_uri: args.content_uri,
                metadata: args.metadata,
                name: String::new(),
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
                    partial_percentage_support_threshold: args.partial_percentage_support_threshold,
                    universal_percentage_support_threshold: args
                        .universal_percentage_support_threshold,
                    flat_support_threshold: args.flat_support_threshold,
                    quorum: args.quorum,
                    duration: args.duration,
                    disable_fast_path_access_for_new_members: args
                        .disable_fast_path_access_for_new_members,
                    execution_grace_period: args.execution_grace_period,
                },
            ))
        }
        ProposalActionType::Ping => decode_ping_governance_action(calldata),
        ProposalActionType::Unknown => None,
    }
}

/// Decode a ping calldata into a governance proposal action.
///
/// Decodes the ABI-encoded ping args and matches the inner `_action` bytes32
/// against the known governance action constants. Unrecognized pings return `None`
/// (stored as Unknown).
fn decode_ping_governance_action(calldata: &[u8]) -> Option<proposal_action::Action> {
    let args = match decode_ping_args(calldata) {
        Ok(args) => args,
        Err(e) => {
            warn!(error = %e, calldata_len = calldata.len(), "Failed to decode ping calldata");
            return None;
        }
    };

    // ZC16 topic layout depends on action type:
    //   Space topic actions (topic set/unset): bytes32(bytes16 topicId)
    //     → topic_id in [0..16], zero-padding in [16..32]
    //   Edge actions (verified/related/etc): bytes32(bytes16 targetSpaceId)
    //     → target in [0..16], zero-padding in [16..32]
    //   Subspace topic actions (subspace_topic_set/unset): [subspace_id: 16 | topic_id: 16]
    //     → topic_id in [16..32]

    match args.action {
        x if x == actions::TOPIC_SET => {
            let target = args.topic[0..16].to_vec();
            if target.iter().all(|b| *b == 0) {
                warn!("All-zero target ID in ping set-topic action, storing as Unknown");
                return None;
            }
            Some(proposal_action::Action::SetTopic(SetTopicAction {
                target_topic_id: target,
            }))
        }
        x if x == actions::TOPIC_UNSET => {
            let target = args.topic[0..16].to_vec();
            if target.iter().all(|b| *b == 0) {
                warn!("All-zero target ID in ping unset-topic action, storing as Unknown");
                return None;
            }
            Some(proposal_action::Action::UnsetTopic(UnsetTopicAction {
                target_topic_id: target,
            }))
        }
        x if x == actions::SUBSPACE_VERIFIED
            || x == actions::SUBSPACE_UNVERIFIED
            || x == actions::SUBSPACE_RELATED
            || x == actions::SUBSPACE_UNRELATED =>
        {
            let target = args.topic[0..16].to_vec();
            if target.iter().all(|b| *b == 0) {
                warn!("All-zero target ID in ping subspace edge action, storing as Unknown");
                return None;
            }
            match args.action {
                x if x == actions::SUBSPACE_VERIFIED => Some(
                    proposal_action::Action::SubspaceVerified(SubspaceEdgeAction {
                        target_space_id: target,
                    }),
                ),
                x if x == actions::SUBSPACE_UNVERIFIED => Some(
                    proposal_action::Action::SubspaceUnverified(SubspaceEdgeAction {
                        target_space_id: target,
                    }),
                ),
                x if x == actions::SUBSPACE_RELATED => Some(
                    proposal_action::Action::SubspaceRelated(SubspaceEdgeAction {
                        target_space_id: target,
                    }),
                ),
                x if x == actions::SUBSPACE_UNRELATED => Some(
                    proposal_action::Action::SubspaceUnrelated(SubspaceEdgeAction {
                        target_space_id: target,
                    }),
                ),
                _ => unreachable!(),
            }
        }
        x if x == actions::SUBSPACE_TOPIC_SET || x == actions::SUBSPACE_TOPIC_UNSET => {
            let target = args.topic[16..32].to_vec();
            if target.iter().all(|b| *b == 0) {
                warn!("All-zero target ID in ping subspace topic action, storing as Unknown");
                return None;
            }
            if x == actions::SUBSPACE_TOPIC_SET {
                Some(proposal_action::Action::SubspaceTopicDeclared(
                    SubspaceTopicAction {
                        target_topic_id: target,
                    },
                ))
            } else {
                Some(proposal_action::Action::SubspaceTopicRemoved(
                    SubspaceTopicAction {
                        target_topic_id: target,
                    },
                ))
            }
        }
        _ => {
            warn!(action_hash = %hex::encode(args.action), "Unrecognized ping action hash, storing as Unknown");
            None
        }
    }
}

/// Enrich Publish actions across all proposals with edit names from the prefetched cache.
fn enrich_publish_action_names(
    proposals_created: &mut [HermesProposalCreated],
    proposals_updated: &mut [HermesProposalUpdated],
    prefetched: &HashMap<String, CachedEdit>,
) {
    let mut enriched_count = 0u32;
    let mut cache_miss_count = 0u32;

    // Enrich proposals_created
    for proposal in proposals_created.iter_mut() {
        let proposal_id_hex = hex::encode(&proposal.proposal_id);
        for action in proposal.actions.iter_mut() {
            if let Some(proposal_action::Action::Publish(publish)) = &mut action.action {
                if let Some(cached_edit) = prefetched.get(&publish.content_uri) {
                    if let Some(name) = &cached_edit.name {
                        debug!(
                            proposal_id = %proposal_id_hex,
                            content_uri = %publish.content_uri,
                            name = %name,
                            "Enriched publish action with edit name"
                        );
                        publish.name = name.clone();
                        enriched_count += 1;
                    }
                } else {
                    debug!(
                        proposal_id = %proposal_id_hex,
                        content_uri = %publish.content_uri,
                        "Cache miss for publish action content URI"
                    );
                    cache_miss_count += 1;
                }
            }
        }
    }

    // Enrich proposals_updated
    for proposal in proposals_updated.iter_mut() {
        let proposal_id_hex = hex::encode(&proposal.proposal_id);
        for action in proposal.actions.iter_mut() {
            if let Some(proposal_action::Action::Publish(publish)) = &mut action.action {
                if let Some(cached_edit) = prefetched.get(&publish.content_uri) {
                    if let Some(name) = &cached_edit.name {
                        debug!(
                            proposal_id = %proposal_id_hex,
                            content_uri = %publish.content_uri,
                            name = %name,
                            "Enriched publish action with edit name"
                        );
                        publish.name = name.clone();
                        enriched_count += 1;
                    }
                } else {
                    debug!(
                        proposal_id = %proposal_id_hex,
                        content_uri = %publish.content_uri,
                        "Cache miss for publish action content URI"
                    );
                    cache_miss_count += 1;
                }
            }
        }
    }

    if enriched_count > 0 || cache_miss_count > 0 {
        info!(
            enriched = enriched_count,
            cache_misses = cache_miss_count,
            "Enriched publish action names"
        );
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

            // Log action types with their selector for debugging
            let selector = if a.data.len() >= 4 {
                hex::encode(&a.data[0..4])
            } else {
                "too_short".to_string()
            };
            info!(
                action_type = ?action_type,
                selector = %selector,
                to_address = %hex::encode(&a.to_address),
                to_space_id = %hex::encode(&a.to_space_id),
                data_len = a.data.len(),
                "Decoded proposal action"
            );

            ProposalAction {
                to_address: a.to_address,
                to_space_id: a.to_space_id,
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
fn parse_proposal_settings_used(action: &Action, sequence: u32) -> Option<ProposalSettingsPending> {
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

    let voting_mode = match decoded.voting_mode {
        0 => VotingMode::Slow,
        1 => VotingMode::Fast,
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

    Some(ProposalSettingsPending {
        space_id: action.from_id.clone(),
        sequence,
        settings: ProposalSettings {
            voting_mode: voting_mode as i32,
            partial_percentage_support_threshold: decoded.partial_percentage_support_threshold,
            universal_percentage_support_threshold: decoded.universal_percentage_support_threshold,
            flat_support_threshold: decoded.flat_support_threshold,
            quorum: decoded.quorum,
            start_date: decoded.start_date,
            last_date: decoded.last_date,
            execute_by: decoded.execute_by,
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
    let (vote, proposal_version) = match decode::decode_proposal_voted(&action.data) {
        Ok(decoded) => {
            let vote_option = match decoded.vote {
                0 => ProposalVoteOption::VoteOptionNone,
                1 => ProposalVoteOption::VoteOptionYes,
                2 => ProposalVoteOption::VoteOptionNo,
                3 => ProposalVoteOption::VoteOptionAbstain,
                _ => ProposalVoteOption::VoteOptionNone,
            };
            (vote_option, decoded.proposal_version)
        }
        Err(e) => {
            warn!(
                error = %e,
                proposal_id = %hex::encode(&action.topic[..16]),
                "Failed to decode proposal voted data"
            );
            (ProposalVoteOption::VoteOptionNone, 0)
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
        proposal_version: proposal_version as u32,
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

/// Convert a VOTING_SETTINGS_UPDATED action into a HermesVotingSettingsUpdated proto.
///
/// Event encoding (emitted by DAOSpace via SpaceRegistry._ping):
/// - from_id: space_id that updated its settings
/// - to_id:   space_id (same — ping event)
/// - topic:   bytes32(0) (unused for this space-level event)
/// - data:    abi.encode(VotingSettings) — 7-field tuple
///
/// Returns None if decoding fails (logged as a warning).
fn convert_voting_settings_updated(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Option<HermesVotingSettingsUpdated> {
    let decoded = match decode::decode_voting_settings_data(&action.data) {
        Ok(decoded) => decoded,
        Err(e) => {
            warn!(
                error = %e,
                space_id = %hex::encode(&action.from_id),
                data_len = action.data.len(),
                "Failed to decode voting settings updated payload"
            );
            return None;
        }
    };

    Some(HermesVotingSettingsUpdated {
        space_id: action.from_id.clone(),
        partial_percentage_support_threshold: decoded.partial_percentage_support_threshold,
        universal_percentage_support_threshold: decoded.universal_percentage_support_threshold,
        flat_support_threshold: decoded.flat_support_threshold,
        quorum: decoded.quorum,
        duration: decoded.duration,
        disable_fast_path_access_for_new_members: decoded.disable_fast_path_access_for_new_members,
        execution_grace_period: decoded.execution_grace_period,
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

    fn empty_prefetch() -> HashMap<String, CachedEdit> {
        HashMap::new()
    }

    // Helper to create encoded PROPOSAL_CREATED data
    fn encode_proposal_created_data(proposal_id: [u8; 16], voting_mode: u8) -> Vec<u8> {
        use ethabi::{Token, ethereum_types::U256 as EthU256};

        let action_tuple = Token::Tuple(vec![
            Token::Address(ethabi::Address::zero()), // toAddress
            Token::FixedBytes(vec![0u8; 16]),        // toSpaceId
            Token::Uint(EthU256::zero()),
            Token::Bytes(vec![]),
        ]);

        ethabi::encode(&[
            Token::FixedBytes(proposal_id.to_vec()),
            Token::Uint(EthU256::from(voting_mode)),
            Token::Array(vec![action_tuple]),
        ])
    }

    // Helper to create encoded PROPOSAL_SETTINGS_SELECTED data (V2 ProposalParameters shape).
    // The single `threshold` parameter is mapped to `flat_support_threshold`; the
    // other V2 threshold fields are zeroed for test simplicity. Field order matches
    // the Solidity `ProposalParameters` struct: votingMode, partialPct, universalPct,
    // flat, quorum, startDate, lastDate, executeBy.
    fn encode_proposal_settings_data(
        start_date: u64,
        last_date: u64,
        voting_mode: u8,
        quorum: u64,
        threshold: u64,
    ) -> Vec<u8> {
        use ethabi::{Token, ethereum_types::U256 as EthU256};

        ethabi::encode(&[
            Token::Uint(EthU256::from(voting_mode)),
            Token::Uint(EthU256::zero()), // partial_percentage_support_threshold
            Token::Uint(EthU256::zero()), // universal_percentage_support_threshold
            Token::Uint(EthU256::from(threshold)), // flat_support_threshold
            Token::Uint(EthU256::from(quorum)),
            Token::Uint(EthU256::from(start_date)),
            Token::Uint(EthU256::from(last_date)),
            Token::Uint(EthU256::zero()), // execute_by
        ])
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

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

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
        assert_eq!(settings.flat_support_threshold, 50); // Fast path uses flat threshold
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

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

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

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

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

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

        // Both should be discarded - IDs don't match
        assert_eq!(result.proposals_created.len(), 0);
    }

    // =========================================================================
    // End-to-end tests: PROPOSAL_CREATED → transform → correct proto variant
    // =========================================================================

    /// Build PROPOSAL_CREATED data with custom inner actions.
    ///
    /// Encoding: `abi.encode(bytes16 proposalId, uint8 votingMode, Action[])`
    /// where each Action is `(address to, uint256 value, bytes data)`.
    fn encode_proposal_created_data_with_actions(
        proposal_id: [u8; 16],
        voting_mode: u8,
        inner_actions: Vec<(ethabi::Address, Vec<u8>)>,
    ) -> Vec<u8> {
        use ethabi::{Token, ethereum_types::U256 as EthU256};

        let action_tokens: Vec<Token> = inner_actions
            .into_iter()
            .map(|(to, data)| {
                Token::Tuple(vec![
                    Token::Address(to),               // toAddress
                    Token::FixedBytes(vec![0u8; 16]), // toSpaceId
                    Token::Uint(EthU256::zero()),
                    Token::Bytes(data),
                ])
            })
            .collect();

        ethabi::encode(&[
            Token::FixedBytes(proposal_id.to_vec()),
            Token::Uint(EthU256::from(voting_mode)),
            Token::Array(action_tokens),
        ])
    }

    /// End-to-end: a PROPOSAL_CREATED containing a ping(SUBSPACE_VERIFIED) action
    /// flows through transform() and produces a proto with SubspaceVerified variant.
    #[test]
    fn test_e2e_proposal_created_with_subspace_verified_action() {
        let proposal_id = [0xA1; 16];
        let proposer_id = vec![0x01; 16];
        let space_id = vec![0x02; 16];
        let target_space_id = [0xCC; 16];

        let topic = make_edge_topic(&target_space_id);
        let ping_calldata = encode_ping_calldata(&actions::SUBSPACE_VERIFIED, &topic, &[]);

        let created_data = encode_proposal_created_data_with_actions(
            proposal_id,
            1, // Fast
            vec![(ethabi::Address::zero(), ping_calldata)],
        );

        let test_actions = vec![
            Action {
                from_id: proposer_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: created_data,
            },
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 1, 100, 50),
            },
        ];

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

        assert_eq!(
            result.proposals_created.len(),
            1,
            "should produce 1 proposal"
        );
        let proposal = &result.proposals_created[0];
        assert_eq!(proposal.proposal_id, proposal_id.to_vec());
        assert_eq!(proposal.actions.len(), 1, "should have 1 action");

        let action = &proposal.actions[0];
        match &action.action {
            Some(proposal_action::Action::SubspaceVerified(edge)) => {
                assert_eq!(edge.target_space_id, target_space_id.to_vec());
            }
            other => panic!("Expected SubspaceVerified, got {other:?}"),
        }
    }

    /// End-to-end: a PROPOSAL_CREATED containing a ping(SUBSPACE_RELATED) action.
    #[test]
    fn test_e2e_proposal_created_with_subspace_related_action() {
        let proposal_id = [0xA2; 16];
        let proposer_id = vec![0x01; 16];
        let space_id = vec![0x02; 16];
        let target_space_id = [0xDD; 16];

        let topic = make_edge_topic(&target_space_id);
        let ping_calldata = encode_ping_calldata(&actions::SUBSPACE_RELATED, &topic, &[]);

        let created_data = encode_proposal_created_data_with_actions(
            proposal_id,
            0, // Slow
            vec![(ethabi::Address::zero(), ping_calldata)],
        );

        let test_actions = vec![
            Action {
                from_id: proposer_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: created_data,
            },
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 0, 100, 50),
            },
        ];

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

        assert_eq!(result.proposals_created.len(), 1);
        let action = &result.proposals_created[0].actions[0];
        match &action.action {
            Some(proposal_action::Action::SubspaceRelated(edge)) => {
                assert_eq!(edge.target_space_id, target_space_id.to_vec());
            }
            other => panic!("Expected SubspaceRelated, got {other:?}"),
        }
    }

    /// End-to-end: a PROPOSAL_CREATED containing a ping(SUBSPACE_UNVERIFIED) removal action.
    #[test]
    fn test_e2e_proposal_created_with_subspace_unverified_action() {
        let proposal_id = [0xA3; 16];
        let proposer_id = vec![0x01; 16];
        let space_id = vec![0x02; 16];
        let target_space_id = [0xEE; 16];

        let topic = make_edge_topic(&target_space_id);
        let ping_calldata = encode_ping_calldata(&actions::SUBSPACE_UNVERIFIED, &topic, &[]);

        let created_data = encode_proposal_created_data_with_actions(
            proposal_id,
            1,
            vec![(ethabi::Address::zero(), ping_calldata)],
        );

        let test_actions = vec![
            Action {
                from_id: proposer_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: created_data,
            },
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 1, 100, 50),
            },
        ];

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

        assert_eq!(result.proposals_created.len(), 1);
        let action = &result.proposals_created[0].actions[0];
        match &action.action {
            Some(proposal_action::Action::SubspaceUnverified(edge)) => {
                assert_eq!(edge.target_space_id, target_space_id.to_vec());
            }
            other => panic!("Expected SubspaceUnverified, got {other:?}"),
        }
    }

    /// End-to-end: a PROPOSAL_CREATED containing a ping(SUBSPACE_TOPIC_SET) action.
    #[test]
    fn test_e2e_proposal_created_with_subspace_topic_set_action() {
        let proposal_id = [0xA4; 16];
        let proposer_id = vec![0x01; 16];
        let space_id = vec![0x02; 16];
        let topic_id = [0xFF; 16];

        let topic = make_topic_topic(&topic_id);
        let ping_calldata = encode_ping_calldata(&actions::SUBSPACE_TOPIC_SET, &topic, &[]);

        let created_data = encode_proposal_created_data_with_actions(
            proposal_id,
            1,
            vec![(ethabi::Address::zero(), ping_calldata)],
        );

        let test_actions = vec![
            Action {
                from_id: proposer_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: created_data,
            },
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 1, 100, 50),
            },
        ];

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

        assert_eq!(result.proposals_created.len(), 1);
        let action = &result.proposals_created[0].actions[0];
        match &action.action {
            Some(proposal_action::Action::SubspaceTopicDeclared(topic_action)) => {
                assert_eq!(topic_action.target_topic_id, topic_id.to_vec());
            }
            other => panic!("Expected SubspaceTopicDeclared, got {other:?}"),
        }
    }

    /// End-to-end: a PROPOSAL_CREATED containing a ping(TOPIC_SET) action.
    #[test]
    fn test_e2e_proposal_created_with_set_topic_action() {
        let proposal_id = [0xA5; 16];
        let proposer_id = vec![0x01; 16];
        let space_id = vec![0x02; 16];
        let topic_id = [0xAB; 16];

        let topic = make_edge_topic(&topic_id);
        let ping_calldata = encode_ping_calldata(&actions::TOPIC_SET, &topic, b"topic-data");

        let created_data = encode_proposal_created_data_with_actions(
            proposal_id,
            1,
            vec![(ethabi::Address::zero(), ping_calldata)],
        );

        let test_actions = vec![
            Action {
                from_id: proposer_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_CREATED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: created_data,
            },
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_proposal_settings_data(1000, 2000, 1, 100, 50),
            },
        ];

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

        assert_eq!(result.proposals_created.len(), 1);
        let action = &result.proposals_created[0].actions[0];
        match &action.action {
            Some(proposal_action::Action::SetTopic(topic_action)) => {
                assert_eq!(topic_action.target_topic_id, topic_id.to_vec());
            }
            other => panic!("Expected SetTopic, got {other:?}"),
        }
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
                action: actions::SUBSPACE_VERIFIED.to_vec(),
                topic: vec![10; 32],
                data: vec![],
            },
        ];

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();
        // No PROPOSAL_CREATED events without matching PROPOSAL_SETTINGS_USED
        assert_eq!(result.proposals_created.len(), 0);
        assert_eq!(result.proposals_voted.len(), 1);
        assert_eq!(result.proposals_executed.len(), 1);
    }

    /// Encode a VOTING_SETTINGS_UPDATED event's `data` field: `abi.encode(VotingSettings)`
    /// as a 7-field tuple.
    fn encode_voting_settings_event(
        partial: u64,
        universal: u64,
        flat: u64,
        quorum: u64,
        duration: u64,
        disable_fast_path: bool,
        grace_period: u64,
    ) -> Vec<u8> {
        use alloy::primitives::U256;
        use alloy::sol_types::SolValue;

        (
            U256::from(partial),
            U256::from(universal),
            U256::from(flat),
            U256::from(quorum),
            U256::from(duration),
            disable_fast_path,
            U256::from(grace_period),
        )
            .abi_encode_params()
    }

    #[test]
    fn transform_routes_voting_settings_updated_event() {
        let space_id = vec![0x42; 16];

        let test_actions = vec![Action {
            from_id: space_id.clone(),
            to_id: space_id.clone(),
            action: actions::VOTING_SETTINGS_UPDATED.to_vec(),
            topic: vec![0u8; 32], // bytes32(0) per contract
            data: encode_voting_settings_event(
                1_000_000, // partial
                2_000_000, // universal
                3,         // flat
                4,         // quorum
                5,         // duration
                true,      // disableFastPathAccessForNewMembers
                6,         // executionGracePeriod
            ),
        }];

        let result = transform(&test_actions, &test_meta(), &empty_prefetch()).unwrap();

        assert_eq!(result.voting_settings_updated.len(), 1);
        let event = &result.voting_settings_updated[0];
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.partial_percentage_support_threshold, 1_000_000);
        assert_eq!(event.universal_percentage_support_threshold, 2_000_000);
        assert_eq!(event.flat_support_threshold, 3);
        assert_eq!(event.quorum, 4);
        assert_eq!(event.duration, 5);
        assert!(event.disable_fast_path_access_for_new_members);
        assert_eq!(event.execution_grace_period, 6);
    }

    /// Encode vote data matching Solidity's `abi.encode(bytes16 proposalId, VoteOption)`.
    ///
    /// ABI encoding for the V2 vote payload `(bytes16, uint8, uint8)`:
    ///   Word 0: bytes16 proposalId (left-aligned, right-padded with zeros)
    ///   Word 1: uint8  proposalVersion (right-aligned, left-padded with zeros)
    ///   Word 2: uint8  voteOption (right-aligned, left-padded with zeros)
    fn encode_vote_data(proposal_id: [u8; 16], proposal_version: u8, vote_option: u8) -> Vec<u8> {
        let mut data = vec![0u8; 96];
        data[..16].copy_from_slice(&proposal_id);
        data[63] = proposal_version;
        data[95] = vote_option;
        data
    }

    /// Wrap raw bytes in ABI `bytes` encoding (offset + length + data), matching
    /// what the EVM produces for a non-indexed `bytes` event parameter.
    fn wrap_in_abi_bytes(inner: &[u8]) -> Vec<u8> {
        let mut wrapped = Vec::new();
        // Offset to data (always 0x20 = 32 for a single bytes param)
        wrapped.extend_from_slice(&[0u8; 31]);
        wrapped.push(0x20);
        // Length of inner data
        let len = inner.len();
        wrapped.extend_from_slice(&[0u8; 24]);
        wrapped.extend_from_slice(&(len as u64).to_be_bytes());
        // Inner data, padded to 32-byte boundary
        wrapped.extend_from_slice(inner);
        let padding = (32 - (len % 32)) % 32;
        wrapped.extend(std::iter::repeat_n(0u8, padding));
        wrapped
    }

    /// Test that convert_proposal_voted correctly decodes each VoteOption
    /// when data is ABI-encoded as the contract produces it (raw, no bytes wrapper).
    #[test]
    fn test_convert_proposal_voted_with_abi_encoded_vote_options() {
        let proposal_id = [0xAA; 16];
        let voter_id = vec![0x01; 16];
        let space_id = vec![0x02; 16];

        let cases = [
            (0u8, ProposalVoteOption::VoteOptionNone as i32, "None"),
            (1, ProposalVoteOption::VoteOptionYes as i32, "Yes"),
            (2, ProposalVoteOption::VoteOptionNo as i32, "No"),
            (3, ProposalVoteOption::VoteOptionAbstain as i32, "Abstain"),
        ];

        for (vote_value, expected_proto, label) in cases {
            let action = Action {
                from_id: voter_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_VOTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: encode_vote_data(proposal_id, 3, vote_value),
            };

            let result = convert_proposal_voted(&action, &test_meta(), 0)
                .unwrap_or_else(|e| panic!("Failed to convert VoteOption.{label}: {e}"));

            assert_eq!(
                result.vote, expected_proto,
                "VoteOption.{label} (value={vote_value}): expected proto value {expected_proto}, got {}",
                result.vote
            );
            assert_eq!(result.voter_id, voter_id);
            assert_eq!(result.space_id, space_id);
            assert_eq!(result.proposal_id, proposal_id.to_vec());
            assert_eq!(result.proposal_version, 3);
        }
    }

    /// Regression test: the EVM wraps non-indexed `bytes` event parameters in
    /// ABI encoding (offset + length + content). decode_proposal_voted must
    /// unwrap this to extract the actual vote data.
    ///
    /// Before the fix, the bytes-wrapped data failed to decode as `(bytes16, uint8)`,
    /// causing convert_proposal_voted to default to VoteOptionNone, which kg-indexer
    /// then mapped to Abstain — making every vote appear as Abstain.
    #[test]
    fn test_convert_proposal_voted_with_bytes_wrapped_data() {
        let proposal_id = [0xAA; 16];
        let voter_id = vec![0x01; 16];
        let space_id = vec![0x02; 16];

        let cases = [
            (1u8, ProposalVoteOption::VoteOptionYes as i32, "Yes"),
            (2, ProposalVoteOption::VoteOptionNo as i32, "No"),
            (3, ProposalVoteOption::VoteOptionAbstain as i32, "Abstain"),
        ];

        for (vote_value, expected_proto, label) in cases {
            // Inner data: abi.encode(bytes16 proposalId, uint8 proposalVersion, uint8 VoteOption)
            let inner = encode_vote_data(proposal_id, 1, vote_value);
            // Wrapped as the EVM would produce for a non-indexed `bytes` event parameter
            let wrapped = wrap_in_abi_bytes(&inner);

            let action = Action {
                from_id: voter_id.clone(),
                to_id: space_id.clone(),
                action: actions::PROPOSAL_VOTED.to_vec(),
                topic: proposal_id.iter().copied().chain(vec![0; 16]).collect(),
                data: wrapped,
            };

            let result = convert_proposal_voted(&action, &test_meta(), 0).unwrap_or_else(|e| {
                panic!("Failed to convert bytes-wrapped VoteOption.{label}: {e}")
            });

            assert_eq!(
                result.vote, expected_proto,
                "Bytes-wrapped VoteOption.{label}: expected proto value {expected_proto}, got {}",
                result.vote
            );
        }
    }

    // Tests for enrich_publish_action_names

    fn make_proposal_with_publish(content_uri: &str) -> HermesProposalCreated {
        use hermes_schema::pb::governance::{ProposalAction, ProposalSettings, PublishAction};

        HermesProposalCreated {
            space_id: vec![1u8; 16],
            proposer_id: vec![2u8; 16],
            proposal_id: vec![3u8; 16],
            voting_mode: 0,
            actions: vec![ProposalAction {
                to_address: vec![],
                to_space_id: vec![],
                value: vec![],
                data: vec![],
                action: Some(proposal_action::Action::Publish(PublishAction {
                    content_uri: content_uri.to_string(),
                    metadata: vec![],
                    name: String::new(), // starts empty
                })),
            }],
            settings: Some(ProposalSettings {
                voting_mode: 0,
                partial_percentage_support_threshold: 0,
                universal_percentage_support_threshold: 0,
                flat_support_threshold: 0,
                quorum: 0,
                start_date: 0,
                last_date: 0,
                execute_by: 0,
            }),
            meta: None,
        }
    }

    fn make_cached_edit(name: Option<&str>) -> CachedEdit {
        CachedEdit {
            cid: "Qmtest".to_string(),
            payload: Some(vec![]),
            is_errored: false,
            space_id: vec![1u8; 16],
            name: name.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_enrich_publish_action_names_with_cached_name() {
        let mut proposals = vec![make_proposal_with_publish("ipfs://Qmtest123")];
        let mut prefetched = HashMap::new();
        prefetched.insert(
            "ipfs://Qmtest123".to_string(),
            make_cached_edit(Some("My Edit Name")),
        );

        enrich_publish_action_names(&mut proposals, &mut [], &prefetched);

        let action = &proposals[0].actions[0];
        if let Some(proposal_action::Action::Publish(publish)) = &action.action {
            assert_eq!(publish.name, "My Edit Name");
        } else {
            panic!("Expected Publish action");
        }
    }

    #[test]
    fn test_enrich_publish_action_names_cache_miss() {
        let mut proposals = vec![make_proposal_with_publish("ipfs://Qmnotfound")];
        let prefetched = HashMap::new(); // empty cache

        enrich_publish_action_names(&mut proposals, &mut [], &prefetched);

        let action = &proposals[0].actions[0];
        if let Some(proposal_action::Action::Publish(publish)) = &action.action {
            assert_eq!(publish.name, ""); // unchanged
        } else {
            panic!("Expected Publish action");
        }
    }

    #[test]
    fn test_enrich_publish_action_names_with_none_name() {
        let mut proposals = vec![make_proposal_with_publish("ipfs://Qmtest456")];
        let mut prefetched = HashMap::new();
        prefetched.insert(
            "ipfs://Qmtest456".to_string(),
            make_cached_edit(None), // no name in cache
        );

        enrich_publish_action_names(&mut proposals, &mut [], &prefetched);

        let action = &proposals[0].actions[0];
        if let Some(proposal_action::Action::Publish(publish)) = &action.action {
            assert_eq!(publish.name, ""); // defaults to empty string
        } else {
            panic!("Expected Publish action");
        }
    }

    // =========================================================================
    // Tests for decode_ping_governance_action
    // =========================================================================

    /// Build valid ping calldata: selector + ABI-encode(bytes32 action, bytes32 topic, bytes data).
    ///
    /// Uses `abi_encode_params` to produce raw function parameters (no outer
    /// tuple wrapping), matching what `encodeFunctionData` produces in viem.
    fn encode_ping_calldata(action: &[u8; 32], topic: &[u8; 32], data: &[u8]) -> Vec<u8> {
        use alloy::primitives::Bytes as PrimBytes;
        use alloy::sol_types::SolType;

        type PingArgsType = alloy::sol! { (bytes32, bytes32, bytes) };
        let encoded =
            PingArgsType::abi_encode_params(&(*action, *topic, PrimBytes::from(data.to_vec())));

        let mut calldata = Vec::with_capacity(4 + encoded.len());
        calldata.extend_from_slice(&decode::selectors::PING);
        calldata.extend_from_slice(&encoded);
        calldata
    }

    /// Build a topic field for edge actions: target in [0..16], zeros in [16..32].
    /// ZC16: Solidity `bytes32(bytes16)` right-pads the bytes16 value.
    fn make_edge_topic(target_id: &[u8; 16]) -> [u8; 32] {
        let mut topic = [0u8; 32];
        topic[0..16].copy_from_slice(target_id);
        topic
    }

    /// Build a topic field for topic actions: [subspace_id: 16 | topic_id: 16].
    fn make_topic_topic(topic_id: &[u8; 16]) -> [u8; 32] {
        let mut topic = [0u8; 32];
        topic[16..32].copy_from_slice(topic_id);
        topic
    }

    #[test]
    fn test_decode_ping_subspace_verified() {
        let target_id = [0xAA; 16];
        let topic = make_edge_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::SUBSPACE_VERIFIED, &topic, &[]);

        // Debug: verify decode_ping_args succeeds and action bytes match
        let args = decode_ping_args(&calldata).expect("decode_ping_args should succeed");
        assert_eq!(
            args.action,
            actions::SUBSPACE_VERIFIED,
            "Action bytes mismatch:\n  got:      {:02x?}\n  expected: {:02x?}",
            args.action,
            actions::SUBSPACE_VERIFIED
        );

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::SubspaceVerified(action)) => {
                assert_eq!(action.target_space_id, target_id.to_vec());
            }
            other => panic!("Expected SubspaceVerified, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_subspace_unverified() {
        let target_id = [0xBB; 16];
        let topic = make_edge_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::SUBSPACE_UNVERIFIED, &topic, &[]);

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::SubspaceUnverified(action)) => {
                assert_eq!(action.target_space_id, target_id.to_vec());
            }
            other => panic!("Expected SubspaceUnverified, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_subspace_related() {
        let target_id = [0xCC; 16];
        let topic = make_edge_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::SUBSPACE_RELATED, &topic, &[]);

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::SubspaceRelated(action)) => {
                assert_eq!(action.target_space_id, target_id.to_vec());
            }
            other => panic!("Expected SubspaceRelated, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_subspace_unrelated() {
        let target_id = [0xDD; 16];
        let topic = make_edge_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::SUBSPACE_UNRELATED, &topic, &[]);

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::SubspaceUnrelated(action)) => {
                assert_eq!(action.target_space_id, target_id.to_vec());
            }
            other => panic!("Expected SubspaceUnrelated, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_subspace_topic_set() {
        let target_id = [0xEE; 16];
        let topic = make_topic_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::SUBSPACE_TOPIC_SET, &topic, &[]);

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::SubspaceTopicDeclared(action)) => {
                assert_eq!(action.target_topic_id, target_id.to_vec());
            }
            other => panic!("Expected SubspaceTopicDeclared, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_subspace_topic_unset() {
        let target_id = [0xFF; 16];
        let topic = make_topic_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::SUBSPACE_TOPIC_UNSET, &topic, &[]);

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::SubspaceTopicRemoved(action)) => {
                assert_eq!(action.target_topic_id, target_id.to_vec());
            }
            other => panic!("Expected SubspaceTopicRemoved, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_set_topic() {
        let target_id = [0x12; 16];
        let topic = make_edge_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::TOPIC_SET, &topic, b"topic-data");

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::SetTopic(action)) => {
                assert_eq!(action.target_topic_id, target_id.to_vec());
            }
            other => panic!("Expected SetTopic, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_unset_topic() {
        let target_id = [0x34; 16];
        let topic = make_edge_topic(&target_id);
        let calldata = encode_ping_calldata(&actions::TOPIC_UNSET, &topic, &[]);

        let result = decode_ping_governance_action(&calldata);
        match result {
            Some(proposal_action::Action::UnsetTopic(action)) => {
                assert_eq!(action.target_topic_id, target_id.to_vec());
            }
            other => panic!("Expected UnsetTopic, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_ping_unrecognized_action_returns_none() {
        let unknown_action = [0x42; 32]; // Not a known subspace action
        let target_id = [0xAA; 16];
        let topic = make_edge_topic(&target_id);
        let calldata = encode_ping_calldata(&unknown_action, &topic, &[]);

        let result = decode_ping_governance_action(&calldata);
        assert!(
            result.is_none(),
            "Unrecognized ping action should return None"
        );
    }

    #[test]
    fn test_enrich_publish_action_names_proposals_updated() {
        use hermes_schema::pb::governance::{ProposalAction, ProposalSettings, PublishAction};

        let mut proposals_updated = vec![HermesProposalUpdated {
            space_id: vec![1u8; 16],
            proposer_id: vec![2u8; 16],
            proposal_id: vec![3u8; 16],
            voting_mode: VotingMode::Fast as i32,
            actions: vec![ProposalAction {
                to_address: vec![],
                to_space_id: vec![],
                value: vec![],
                data: vec![],
                action: Some(proposal_action::Action::Publish(PublishAction {
                    content_uri: "ipfs://Qmupdated".to_string(),
                    metadata: vec![],
                    name: String::new(),
                })),
            }],
            settings: Some(ProposalSettings {
                voting_mode: 0,
                partial_percentage_support_threshold: 0,
                universal_percentage_support_threshold: 0,
                flat_support_threshold: 0,
                quorum: 0,
                start_date: 0,
                last_date: 0,
                execute_by: 0,
            }),
            meta: None,
        }];

        let mut prefetched = HashMap::new();
        prefetched.insert(
            "ipfs://Qmupdated".to_string(),
            make_cached_edit(Some("Updated Edit")),
        );

        enrich_publish_action_names(&mut [], &mut proposals_updated, &prefetched);

        let action = &proposals_updated[0].actions[0];
        if let Some(proposal_action::Action::Publish(publish)) = &action.action {
            assert_eq!(publish.name, "Updated Edit");
        } else {
            panic!("Expected Publish action");
        }
    }
}
