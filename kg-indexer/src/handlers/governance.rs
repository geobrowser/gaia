use hermes_schema::pb::governance::{
    proposal_action::Action, HermesProposalCreated, HermesProposalExecuted,
    HermesProposalSettingsUpdated, HermesProposalUpdated, HermesProposalVoted,
    HermesVotingSettingsUpdated, ProposalSettings, ProposalVoteOption,
    VotingMode as ProtoVotingMode,
};
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::governance::{
    ProposalActionItem, ProposalActionPayload, ProposalIdentity, ProposalVersionItem,
    ProposalVoteItem, SpaceVotingSettingsItem, VoteOption, VotingMode,
};

/// Result of processing a `HermesProposalCreated` event: the immutable
/// identity (inserted on first encounter), the initial version's state, and
/// the actions for v1. Storage composes all three into a single transaction.
#[allow(dead_code)]
pub struct ProposalCreatedResult {
    pub identity: ProposalIdentity,
    pub version: ProposalVersionItem,
    /// Actions with `proposal_version = 1` pre-set, ready for storage insert.
    pub actions: Vec<ProposalActionItem>,
}

/// Result of processing a `HermesProposalUpdated` event: a new version's
/// state + the new action set. Storage atomically inserts the version row
/// (assigning the next `proposal_version` number), bumps
/// `proposals.current_version`, then inserts the actions against that new
/// version. Actions returned here carry `proposal_version = 0` as a sentinel
/// — storage overwrites it with the assigned version before writing.
#[allow(dead_code)]
pub struct ProposalUpdatedResult {
    pub proposal_id: Uuid,
    pub version: ProposalVersionItem,
    pub actions: Vec<ProposalActionItem>,
}

/// Result of processing a proposal execution
#[allow(dead_code)]
pub struct ProposalExecutionResult {
    pub proposal_id: Uuid,
    pub space_id: Uuid,
    pub executed_at: i64,
}

/// Process a HermesProposalCreated message.
///
/// Produces identity + v1 version + actions (pre-stamped with version 1).
pub fn handle_proposal_created(
    msg: &HermesProposalCreated,
) -> Result<ProposalCreatedResult, HandlerError> {
    let (proposal_id, space_id, proposer_id, version, actions) = map_proposal_message(
        &msg.proposal_id,
        &msg.space_id,
        &msg.proposer_id,
        msg.voting_mode,
        msg.settings.as_ref(),
        &msg.actions,
        msg.meta.as_ref(),
    )?;

    // Stamp version=1 on all actions for this CREATE.
    let actions: Vec<ProposalActionItem> = actions
        .into_iter()
        .map(|mut a| {
            a.proposal_version = 1;
            a
        })
        .collect();

    let identity = ProposalIdentity {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        created_at: version.version_created_at,
        created_at_block: version.version_created_at_block,
    };

    Ok(ProposalCreatedResult {
        identity,
        version,
        actions,
    })
}

/// Process a HermesProposalUpdated message.
///
/// Returns the proposal_id + the new version's state + actions with
/// `proposal_version = 0` as a sentinel — storage assigns the real version
/// number atomically when the version row is inserted.
pub fn handle_proposal_updated(
    msg: &HermesProposalUpdated,
) -> Result<ProposalUpdatedResult, HandlerError> {
    let (proposal_id, _space_id, _proposer_id, version, actions) = map_proposal_message(
        &msg.proposal_id,
        &msg.space_id,
        &msg.proposer_id,
        msg.voting_mode,
        msg.settings.as_ref(),
        &msg.actions,
        msg.meta.as_ref(),
    )?;

    // proposal_version left at 0; storage stamps it after the version insert.
    Ok(ProposalUpdatedResult {
        proposal_id,
        version,
        actions,
    })
}

/// Process a HermesProposalVoted message
pub fn handle_proposal_voted(msg: &HermesProposalVoted) -> Result<ProposalVoteItem, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;
    let voter_id = Uuid::from_slice(&msg.voter_id)?;

    let vote = match ProposalVoteOption::try_from(msg.vote) {
        Ok(ProposalVoteOption::VoteOptionYes) => VoteOption::Yes,
        Ok(ProposalVoteOption::VoteOptionNo) => VoteOption::No,
        Ok(ProposalVoteOption::VoteOptionAbstain) => VoteOption::Abstain,
        // Default to Abstain for unknown vote types
        Ok(ProposalVoteOption::VoteOptionNone) | Err(_) => VoteOption::Abstain,
    };

    let meta = msg.meta.as_ref();
    let (created_at, created_at_block) = meta
        .map(|m| (m.created_at as i64, m.block_number as i64))
        .unwrap_or((0, 0));

    // proto3 scalars can't distinguish "unset" from "zero"; the contract
    // says proposal versions start at 1, so treat 0 as unset and default to 1.
    let proposal_version = normalize_proposal_version(msg.proposal_version);

    Ok(ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote,
        created_at,
        created_at_block,
        proposal_version,
    })
}

/// Normalize a proto3 `proposal_version` scalar: treat 0 as "unset" and
/// default to 1 (per the documented "versions start at 1" contract).
fn normalize_proposal_version(raw: u32) -> i32 {
    if raw == 0 {
        1
    } else {
        raw as i32
    }
}

/// Normalize a proto3 `execute_by` scalar: treat 0 as "no deadline" (None)
/// so it is distinguishable from an actual epoch timestamp.
fn normalize_execute_by(raw: u64) -> Option<i64> {
    (raw != 0).then_some(raw as i64)
}

/// Process a HermesProposalExecuted message
pub fn handle_proposal_executed(
    msg: &HermesProposalExecuted,
) -> Result<ProposalExecutionResult, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;

    let executed_at = msg.meta.as_ref().map(|m| m.created_at as i64).unwrap_or(0);

    Ok(ProposalExecutionResult {
        proposal_id,
        space_id,
        executed_at,
    })
}

/// Result of processing a proposal settings update (fast→slow escalation).
///
/// Version is NOT bumped on escalation; all new V2 threshold fields + execute_by
/// are carried through so the storage layer can overwrite the existing row's
/// per-proposal settings snapshot.
#[allow(dead_code)]
pub struct ProposalSettingsUpdateResult {
    pub proposal_id: Uuid,
    pub space_id: Uuid,
    pub voting_mode: VotingMode,
    pub start_time: i64,
    pub end_time: i64,
    pub quorum: i64,
    /// Legacy threshold: voting-mode-dependent selection from V2 fields.
    pub threshold: i64,
    pub partial_percentage_support_threshold: i64,
    pub universal_percentage_support_threshold: i64,
    pub flat_support_threshold: i64,
    pub execute_by: Option<i64>,
}

/// Process a HermesProposalSettingsUpdated message (fast→slow escalation)
pub fn handle_proposal_settings_updated(
    msg: &HermesProposalSettingsUpdated,
) -> Result<ProposalSettingsUpdateResult, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;

    let settings = msg.settings.as_ref().ok_or(HandlerError::MissingPayload)?;

    let voting_mode = match ProtoVotingMode::try_from(settings.voting_mode) {
        Ok(ProtoVotingMode::Fast) => VotingMode::Fast,
        Ok(ProtoVotingMode::Slow) | Err(_) => VotingMode::Slow,
    };

    let threshold = legacy_threshold(&voting_mode, settings);

    Ok(ProposalSettingsUpdateResult {
        proposal_id,
        space_id,
        voting_mode,
        start_time: settings.start_date as i64,
        end_time: settings.last_date as i64,
        quorum: settings.quorum as i64,
        threshold,
        partial_percentage_support_threshold: settings.partial_percentage_support_threshold as i64,
        universal_percentage_support_threshold: settings.universal_percentage_support_threshold
            as i64,
        flat_support_threshold: settings.flat_support_threshold as i64,
        execute_by: normalize_execute_by(settings.execute_by),
    })
}

/// Derive the legacy `threshold` value from V2 settings using the V1
/// voting-mode-dependent selection rule. Kept in one spot so create +
/// escalation paths stay in sync.
fn legacy_threshold(voting_mode: &VotingMode, settings: &ProposalSettings) -> i64 {
    match voting_mode {
        VotingMode::Fast => settings.flat_support_threshold as i64,
        VotingMode::Slow => settings.partial_percentage_support_threshold as i64,
    }
}

/// Process a HermesVotingSettingsUpdated message.
///
/// Maps the DAO-global voting settings into `SpaceVotingSettingsItem`.
pub fn handle_voting_settings_updated(
    msg: &HermesVotingSettingsUpdated,
) -> Result<SpaceVotingSettingsItem, HandlerError> {
    let space_id = Uuid::from_slice(&msg.space_id)?;

    let meta = msg.meta.as_ref();
    let (updated_at, updated_at_block) = meta
        .map(|m| (m.created_at as i64, m.block_number as i64))
        .unwrap_or((0, 0));

    Ok(SpaceVotingSettingsItem {
        space_id,
        partial_percentage_support_threshold: msg.partial_percentage_support_threshold as i64,
        universal_percentage_support_threshold: msg.universal_percentage_support_threshold as i64,
        flat_support_threshold: msg.flat_support_threshold as i64,
        quorum: msg.quorum as i64,
        duration: msg.duration as i64,
        disable_fast_path_access_for_new_members: msg.disable_fast_path_access_for_new_members,
        execution_grace_period: msg.execution_grace_period as i64,
        updated_at,
        updated_at_block,
    })
}

/// Maximum length for proposal names (truncated with "..." if exceeded)
const MAX_PROPOSAL_NAME_LENGTH: usize = 500;

/// Derive a human-readable name for a proposal from its proto actions.
///
/// - Publish actions: use the name from IPFS cache (or "Publish" if empty)
/// - Other actions: use a human-readable label based on action type
/// - Multiple actions: concatenate with ", "
/// - Truncate to MAX_PROPOSAL_NAME_LENGTH with "..." if too long
fn derive_proposal_name(
    actions: &[hermes_schema::pb::governance::ProposalAction],
) -> Option<String> {
    if actions.is_empty() {
        return None;
    }

    let names: Vec<&str> = actions
        .iter()
        .map(|a| match &a.action {
            Some(Action::AddMember(_)) => "Add Member",
            Some(Action::RemoveMember(_)) => "Remove Member",
            Some(Action::AddEditor(_)) => "Add Editor",
            Some(Action::RemoveEditor(_)) => "Remove Editor",
            Some(Action::UnflagEditor(_)) => "Unflag Editor",
            Some(Action::Publish(p)) => {
                if p.name.is_empty() {
                    "Publish"
                } else {
                    p.name.as_str()
                }
            }
            Some(Action::Flag(_)) => "Flag",
            Some(Action::Unflag(_)) => "Unflag",
            Some(Action::UpdateVotingSettings(_)) => "Update Voting Settings",
            Some(Action::SubspaceVerified(_)) => "Add Verified Space",
            Some(Action::SubspaceUnverified(_)) => "Remove Verified Space",
            Some(Action::SubspaceRelated(_)) => "Add Related Space",
            Some(Action::SubspaceUnrelated(_)) => "Remove Related Space",
            Some(Action::SubspaceTopicDeclared(_)) => "Add subtopic",
            Some(Action::SubspaceTopicRemoved(_)) => "Remove subtopic",
            Some(Action::SetTopic(_)) => "Set Topic",
            Some(Action::UnsetTopic(_)) => "Unset Topic",
            None => "Unknown Action",
        })
        .collect();

    let joined = names.join(", ");

    if joined.len() <= MAX_PROPOSAL_NAME_LENGTH {
        Some(joined)
    } else {
        // Truncate to safe UTF-8 boundary, leaving room for "..."
        let max_truncate = MAX_PROPOSAL_NAME_LENGTH - 3;
        let safe_end = truncate_to_char_boundary(&joined, max_truncate);

        // Find last comma to avoid cutting mid-name
        if let Some(last_comma) = safe_end.rfind(", ") {
            Some(format!("{}...", &safe_end[..last_comma]))
        } else {
            Some(format!("{}...", safe_end))
        }
    }
}

/// Truncate a string to at most `max_bytes` bytes, ensuring we don't cut
/// in the middle of a multi-byte UTF-8 character.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the last valid char boundary at or before max_bytes
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Shared CREATE/UPDATE mapping. Returns the tuple `(proposal_id, space_id,
/// proposer_id, version_item, actions)`. The caller wraps these fields into
/// whichever result type matches its event source.
#[allow(clippy::type_complexity)]
fn map_proposal_message(
    proposal_id: &[u8],
    space_id: &[u8],
    proposer_id: &[u8],
    voting_mode: i32,
    settings: Option<&ProposalSettings>,
    actions: &[hermes_schema::pb::governance::ProposalAction],
    meta: Option<&hermes_schema::pb::blockchain_metadata::BlockchainMetadata>,
) -> Result<
    (
        Uuid,
        Uuid,
        Uuid,
        ProposalVersionItem,
        Vec<ProposalActionItem>,
    ),
    HandlerError,
> {
    let proposal_id = Uuid::from_slice(proposal_id)?;
    let space_id = Uuid::from_slice(space_id)?;
    let proposer_id = Uuid::from_slice(proposer_id)?;

    let voting_mode = match ProtoVotingMode::try_from(voting_mode) {
        Ok(ProtoVotingMode::Fast) => VotingMode::Fast,
        Ok(ProtoVotingMode::Slow) | Err(_) => VotingMode::Slow,
    };

    let settings = settings.ok_or(HandlerError::MissingPayload)?;
    let (version_created_at, version_created_at_block) = meta
        .map(|m| (m.created_at as i64, m.block_number as i64))
        .unwrap_or((0, 0));

    let threshold = legacy_threshold(&voting_mode, settings);

    // Derive name from proto actions (before mapping to internal types)
    let name = derive_proposal_name(actions);

    // Map proto actions to internal types. `proposal_version` is left as 0
    // here — the CREATE handler stamps 1; the UPDATE handler defers to storage.
    let actions: Vec<ProposalActionItem> = actions
        .iter()
        .enumerate()
        .map(|(index, action)| map_proposal_action(proposal_id, index as i32, action))
        .collect();

    let version = ProposalVersionItem {
        voting_mode,
        start_time: settings.start_date as i64,
        end_time: settings.last_date as i64,
        quorum: settings.quorum as i64,
        threshold,
        partial_percentage_support_threshold: settings.partial_percentage_support_threshold as i64,
        universal_percentage_support_threshold: settings.universal_percentage_support_threshold
            as i64,
        flat_support_threshold: settings.flat_support_threshold as i64,
        execute_by: normalize_execute_by(settings.execute_by),
        name,
        version_created_at,
        version_created_at_block,
    };

    Ok((proposal_id, space_id, proposer_id, version, actions))
}

fn map_proposal_action(
    proposal_id: Uuid,
    index: i32,
    action: &hermes_schema::pb::governance::ProposalAction,
) -> ProposalActionItem {
    // Helper to convert bytes16 space ID to UUID
    // The target_address field contains a 16-byte space ID (bytes16), not an Ethereum address
    let bytes_to_uuid = |bytes: &[u8]| -> Option<Uuid> { Uuid::from_slice(bytes).ok() };

    let payload = match &action.action {
        Some(Action::AddMember(a)) => match bytes_to_uuid(&a.target_address) {
            Some(target_id) => ProposalActionPayload::AddMember { target_id },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::RemoveMember(a)) => match bytes_to_uuid(&a.target_address) {
            Some(target_id) => ProposalActionPayload::RemoveMember { target_id },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::AddEditor(a)) => match bytes_to_uuid(&a.target_address) {
            Some(target_id) => ProposalActionPayload::AddEditor { target_id },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::RemoveEditor(a)) => match bytes_to_uuid(&a.target_address) {
            Some(target_id) => ProposalActionPayload::RemoveEditor { target_id },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::UnflagEditor(a)) => match bytes_to_uuid(&a.target_address) {
            Some(target_id) => ProposalActionPayload::UnflagEditor { target_id },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::Publish(a)) => ProposalActionPayload::Publish {
            content_uri: a.content_uri.clone(),
            metadata: a.metadata.clone(),
        },
        Some(Action::Flag(a)) => ProposalActionPayload::Flag {
            content_id: a.content_id.clone(),
        },
        Some(Action::Unflag(a)) => ProposalActionPayload::Unflag {
            content_id: a.content_id.clone(),
        },
        Some(Action::UpdateVotingSettings(a)) => ProposalActionPayload::UpdateVotingSettings {
            partial_percentage_support_threshold: a.partial_percentage_support_threshold,
            universal_percentage_support_threshold: a.universal_percentage_support_threshold,
            flat_support_threshold: a.flat_support_threshold,
            quorum: a.quorum,
            duration: a.duration,
            disable_fast_path_access_for_new_members: a.disable_fast_path_access_for_new_members,
            execution_grace_period: a.execution_grace_period,
        },
        Some(Action::SubspaceVerified(a)) => match bytes_to_uuid(&a.target_space_id) {
            Some(id) => ProposalActionPayload::SubspaceVerified {
                target_space_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::SubspaceUnverified(a)) => match bytes_to_uuid(&a.target_space_id) {
            Some(id) => ProposalActionPayload::SubspaceUnverified {
                target_space_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::SubspaceRelated(a)) => match bytes_to_uuid(&a.target_space_id) {
            Some(id) => ProposalActionPayload::SubspaceRelated {
                target_space_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::SubspaceUnrelated(a)) => match bytes_to_uuid(&a.target_space_id) {
            Some(id) => ProposalActionPayload::SubspaceUnrelated {
                target_space_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::SubspaceTopicDeclared(a)) => match bytes_to_uuid(&a.target_topic_id) {
            Some(id) => ProposalActionPayload::SubspaceTopicDeclared {
                target_topic_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::SubspaceTopicRemoved(a)) => match bytes_to_uuid(&a.target_topic_id) {
            Some(id) => ProposalActionPayload::SubspaceTopicRemoved {
                target_topic_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::SetTopic(a)) => match bytes_to_uuid(&a.target_topic_id) {
            Some(id) => ProposalActionPayload::SetTopic {
                target_topic_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        Some(Action::UnsetTopic(a)) => match bytes_to_uuid(&a.target_topic_id) {
            Some(id) => ProposalActionPayload::UnsetTopic {
                target_topic_id: id,
            },
            None => ProposalActionPayload::Unknown,
        },
        None => ProposalActionPayload::Unknown,
    };

    ProposalActionItem {
        proposal_id,
        // Stamped by the handler layer (CREATE → 1; UPDATE → set by storage).
        proposal_version: 0,
        index,
        payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::governance::{AddMemberAction, ProposalAction, PublishAction};

    fn make_publish_action(name: &str) -> ProposalAction {
        ProposalAction {
            to: vec![],
            value: vec![],
            data: vec![],
            action: Some(Action::Publish(PublishAction {
                content_uri: "ipfs://test".to_string(),
                metadata: vec![],
                name: name.to_string(),
            })),
        }
    }

    fn make_add_member_action() -> ProposalAction {
        ProposalAction {
            to: vec![],
            value: vec![],
            data: vec![],
            action: Some(Action::AddMember(AddMemberAction {
                target_address: vec![0u8; 16],
            })),
        }
    }

    #[test]
    fn test_derive_proposal_name_empty_actions() {
        let actions: Vec<ProposalAction> = vec![];
        assert_eq!(derive_proposal_name(&actions), None);
    }

    #[test]
    fn test_derive_proposal_name_single_publish_with_name() {
        let actions = vec![make_publish_action("My Edit Name")];
        assert_eq!(
            derive_proposal_name(&actions),
            Some("My Edit Name".to_string())
        );
    }

    #[test]
    fn test_derive_proposal_name_single_publish_without_name() {
        let actions = vec![make_publish_action("")];
        assert_eq!(derive_proposal_name(&actions), Some("Publish".to_string()));
    }

    #[test]
    fn test_derive_proposal_name_multiple_actions() {
        let actions = vec![
            make_add_member_action(),
            make_publish_action("Article Title"),
        ];
        assert_eq!(
            derive_proposal_name(&actions),
            Some("Add Member, Article Title".to_string())
        );
    }

    #[test]
    fn test_derive_proposal_name_truncation() {
        // Create enough actions to exceed MAX_PROPOSAL_NAME_LENGTH (500)
        let long_name = "A".repeat(200);
        let actions = vec![
            make_publish_action(&long_name),
            make_publish_action(&long_name),
            make_publish_action(&long_name),
        ];
        let result = derive_proposal_name(&actions).unwrap();

        assert!(result.len() <= MAX_PROPOSAL_NAME_LENGTH);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_derive_proposal_name_truncation_at_comma() {
        // Create actions that will be truncated, verify it truncates at comma boundary
        let actions: Vec<ProposalAction> = (0..50).map(|_| make_add_member_action()).collect();
        let result = derive_proposal_name(&actions).unwrap();

        assert!(result.len() <= MAX_PROPOSAL_NAME_LENGTH);
        assert!(result.ends_with("..."));
        // Should not end with partial "Add Member" - should be clean truncation
        assert!(!result.contains("Add Mem..."));
    }

    #[test]
    fn test_derive_proposal_name_unicode_safety() {
        // Test with multi-byte UTF-8 characters (emoji)
        let actions = vec![make_publish_action(&"🎉".repeat(200))];
        let result = derive_proposal_name(&actions).unwrap();

        // Should not panic and should be valid UTF-8
        assert!(result.len() <= MAX_PROPOSAL_NAME_LENGTH);
        assert!(result.is_char_boundary(result.len()));
    }

    #[test]
    fn test_truncate_to_char_boundary_ascii() {
        let s = "hello world";
        assert_eq!(truncate_to_char_boundary(s, 5), "hello");
        assert_eq!(truncate_to_char_boundary(s, 100), "hello world");
    }

    #[test]
    fn test_truncate_to_char_boundary_utf8() {
        let s = "héllo"; // é is 2 bytes
                         // "h" = 1 byte, "é" = 2 bytes (positions 1-2), "llo" = 3 bytes
        assert_eq!(truncate_to_char_boundary(s, 1), "h");
        assert_eq!(truncate_to_char_boundary(s, 2), "h"); // can't cut mid-é
        assert_eq!(truncate_to_char_boundary(s, 3), "hé");
    }

    #[test]
    fn test_truncate_to_char_boundary_emoji() {
        let s = "🎉🎊"; // Each emoji is 4 bytes
        assert_eq!(truncate_to_char_boundary(s, 4), "🎉");
        assert_eq!(truncate_to_char_boundary(s, 5), "🎉"); // can't cut mid-emoji
        assert_eq!(truncate_to_char_boundary(s, 8), "🎉🎊");
    }

    fn make_meta(
        created_at: u64,
        block_number: u64,
    ) -> hermes_schema::pb::blockchain_metadata::BlockchainMetadata {
        hermes_schema::pb::blockchain_metadata::BlockchainMetadata {
            created_at,
            created_by: vec![],
            block_number,
            cursor: String::new(),
            sequence: 0,
            is_last: false,
        }
    }

    fn v2_settings(voting_mode: i32) -> ProposalSettings {
        ProposalSettings {
            voting_mode,
            partial_percentage_support_threshold: 500_000,
            universal_percentage_support_threshold: 750_000,
            flat_support_threshold: 5,
            quorum: 10,
            start_date: 1_000,
            last_date: 2_000,
            execute_by: 3_000,
        }
    }

    #[test]
    fn handle_voting_settings_updated_maps_all_fields() {
        use hermes_schema::pb::governance::HermesVotingSettingsUpdated;

        let space_id = Uuid::new_v4();
        let msg = HermesVotingSettingsUpdated {
            space_id: space_id.as_bytes().to_vec(),
            partial_percentage_support_threshold: 1_000_000,
            universal_percentage_support_threshold: 2_000_000,
            flat_support_threshold: 3,
            quorum: 4,
            duration: 5,
            disable_fast_path_access_for_new_members: true,
            execution_grace_period: 6,
            meta: Some(make_meta(1_700_000_000, 999)),
        };

        let item = handle_voting_settings_updated(&msg).unwrap();

        assert_eq!(item.space_id, space_id);
        assert_eq!(item.partial_percentage_support_threshold, 1_000_000);
        assert_eq!(item.universal_percentage_support_threshold, 2_000_000);
        assert_eq!(item.flat_support_threshold, 3);
        assert_eq!(item.quorum, 4);
        assert_eq!(item.duration, 5);
        assert!(item.disable_fast_path_access_for_new_members);
        assert_eq!(item.execution_grace_period, 6);
        assert_eq!(item.updated_at, 1_700_000_000);
        assert_eq!(item.updated_at_block, 999);
    }

    #[test]
    fn handle_proposal_voted_propagates_proposal_version() {
        let msg = HermesProposalVoted {
            voter_id: Uuid::new_v4().as_bytes().to_vec(),
            space_id: Uuid::new_v4().as_bytes().to_vec(),
            proposal_id: Uuid::new_v4().as_bytes().to_vec(),
            vote: ProposalVoteOption::VoteOptionYes as i32,
            meta: Some(make_meta(100, 1)),
            proposal_version: 7,
        };

        let item = handle_proposal_voted(&msg).unwrap();

        assert_eq!(item.proposal_version, 7);
        assert_eq!(item.vote, VoteOption::Yes);
    }

    #[test]
    fn handle_proposal_created_populates_v2_fields() {
        let space_id = Uuid::new_v4();
        let proposer_id = Uuid::new_v4();
        let proposal_id = Uuid::new_v4();

        let msg = HermesProposalCreated {
            space_id: space_id.as_bytes().to_vec(),
            proposer_id: proposer_id.as_bytes().to_vec(),
            proposal_id: proposal_id.as_bytes().to_vec(),
            voting_mode: ProtoVotingMode::Slow as i32,
            actions: vec![],
            settings: Some(v2_settings(ProtoVotingMode::Slow as i32)),
            meta: Some(make_meta(1_700_000_000, 42)),
        };

        let result = handle_proposal_created(&msg).unwrap();
        let p = result.version;

        assert_eq!(p.partial_percentage_support_threshold, 500_000);
        assert_eq!(p.universal_percentage_support_threshold, 750_000);
        assert_eq!(p.flat_support_threshold, 5);
        assert_eq!(p.execute_by, Some(3_000));
        // Legacy threshold: Slow path → partial_percentage_support_threshold
        assert_eq!(p.threshold, 500_000);
        assert_eq!(p.quorum, 10);
    }

    #[test]
    fn handle_proposal_created_legacy_threshold_fast_path_uses_flat() {
        let space_id = Uuid::new_v4();
        let proposer_id = Uuid::new_v4();
        let proposal_id = Uuid::new_v4();

        let msg = HermesProposalCreated {
            space_id: space_id.as_bytes().to_vec(),
            proposer_id: proposer_id.as_bytes().to_vec(),
            proposal_id: proposal_id.as_bytes().to_vec(),
            voting_mode: ProtoVotingMode::Fast as i32,
            actions: vec![],
            settings: Some(v2_settings(ProtoVotingMode::Fast as i32)),
            meta: Some(make_meta(1, 1)),
        };

        let result = handle_proposal_created(&msg).unwrap();
        // Legacy threshold: Fast path → flat_support_threshold
        assert_eq!(result.version.threshold, 5);
    }

    #[test]
    fn update_voting_settings_action_payload_has_v2_seven_fields() {
        use hermes_schema::pb::governance::{ProposalAction, UpdateVotingSettingsAction};

        let proposer_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let proposal_id = Uuid::new_v4();

        let action = ProposalAction {
            to: vec![],
            value: vec![],
            data: vec![],
            action: Some(Action::UpdateVotingSettings(UpdateVotingSettingsAction {
                partial_percentage_support_threshold: 1_000,
                universal_percentage_support_threshold: 2_000,
                flat_support_threshold: 3,
                quorum: 4,
                duration: 5,
                disable_fast_path_access_for_new_members: true,
                execution_grace_period: 6,
            })),
        };

        let msg = HermesProposalCreated {
            space_id: space_id.as_bytes().to_vec(),
            proposer_id: proposer_id.as_bytes().to_vec(),
            proposal_id: proposal_id.as_bytes().to_vec(),
            voting_mode: ProtoVotingMode::Slow as i32,
            actions: vec![action],
            settings: Some(v2_settings(ProtoVotingMode::Slow as i32)),
            meta: Some(make_meta(0, 0)),
        };

        let result = handle_proposal_created(&msg).unwrap();
        let payload = &result.actions[0].payload;

        match payload {
            ProposalActionPayload::UpdateVotingSettings {
                partial_percentage_support_threshold,
                universal_percentage_support_threshold,
                flat_support_threshold,
                quorum,
                duration,
                disable_fast_path_access_for_new_members,
                execution_grace_period,
            } => {
                assert_eq!(*partial_percentage_support_threshold, 1_000);
                assert_eq!(*universal_percentage_support_threshold, 2_000);
                assert_eq!(*flat_support_threshold, 3);
                assert_eq!(*quorum, 4);
                assert_eq!(*duration, 5);
                assert!(*disable_fast_path_access_for_new_members);
                assert_eq!(*execution_grace_period, 6);
            }
            other => panic!("expected UpdateVotingSettings payload, got {:?}", other),
        }
    }

    #[test]
    fn handle_proposal_settings_updated_carries_v2_fields() {
        let proposal_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let msg = HermesProposalSettingsUpdated {
            proposal_id: proposal_id.as_bytes().to_vec(),
            space_id: space_id.as_bytes().to_vec(),
            settings: Some(v2_settings(ProtoVotingMode::Slow as i32)),
            meta: Some(make_meta(0, 0)),
        };

        let result = handle_proposal_settings_updated(&msg).unwrap();

        assert_eq!(result.partial_percentage_support_threshold, 500_000);
        assert_eq!(result.universal_percentage_support_threshold, 750_000);
        assert_eq!(result.flat_support_threshold, 5);
        assert_eq!(result.execute_by, Some(3_000));
        // Legacy threshold preserved: Slow → partial
        assert_eq!(result.threshold, 500_000);
    }

    #[test]
    fn handle_proposal_voted_defaults_proposal_version_to_one_when_zero() {
        // proto3 scalars can't distinguish unset from 0, so a 0 on the wire
        // should be normalized to the documented starting version of 1.
        let msg = HermesProposalVoted {
            voter_id: Uuid::new_v4().as_bytes().to_vec(),
            space_id: Uuid::new_v4().as_bytes().to_vec(),
            proposal_id: Uuid::new_v4().as_bytes().to_vec(),
            vote: ProposalVoteOption::VoteOptionYes as i32,
            meta: Some(make_meta(100, 1)),
            proposal_version: 0,
        };

        let item = handle_proposal_voted(&msg).unwrap();

        assert_eq!(item.proposal_version, 1);
    }

    #[test]
    fn handle_proposal_voted_preserves_explicit_version_one() {
        // A non-zero wire value should pass through unchanged.
        let msg = HermesProposalVoted {
            voter_id: Uuid::new_v4().as_bytes().to_vec(),
            space_id: Uuid::new_v4().as_bytes().to_vec(),
            proposal_id: Uuid::new_v4().as_bytes().to_vec(),
            vote: ProposalVoteOption::VoteOptionYes as i32,
            meta: Some(make_meta(100, 1)),
            proposal_version: 1,
        };

        let item = handle_proposal_voted(&msg).unwrap();

        assert_eq!(item.proposal_version, 1);
    }

    #[test]
    fn handle_proposal_created_maps_zero_execute_by_to_none() {
        // execute_by == 0 means "no deadline" on the wire; the mapped row
        // should carry None so it is distinguishable from an epoch timestamp.
        let space_id = Uuid::new_v4();
        let proposer_id = Uuid::new_v4();
        let proposal_id = Uuid::new_v4();

        let mut settings = v2_settings(ProtoVotingMode::Slow as i32);
        settings.execute_by = 0;

        let msg = HermesProposalCreated {
            space_id: space_id.as_bytes().to_vec(),
            proposer_id: proposer_id.as_bytes().to_vec(),
            proposal_id: proposal_id.as_bytes().to_vec(),
            voting_mode: ProtoVotingMode::Slow as i32,
            actions: vec![],
            settings: Some(settings),
            meta: Some(make_meta(1_700_000_000, 42)),
        };

        let result = handle_proposal_created(&msg).unwrap();

        assert_eq!(result.version.execute_by, None);
    }

    #[test]
    fn handle_proposal_settings_updated_maps_zero_execute_by_to_none() {
        // Same contract on the escalation path.
        let proposal_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut settings = v2_settings(ProtoVotingMode::Slow as i32);
        settings.execute_by = 0;

        let msg = HermesProposalSettingsUpdated {
            proposal_id: proposal_id.as_bytes().to_vec(),
            space_id: space_id.as_bytes().to_vec(),
            settings: Some(settings),
            meta: Some(make_meta(0, 0)),
        };

        let result = handle_proposal_settings_updated(&msg).unwrap();

        assert_eq!(result.execute_by, None);
    }
}
