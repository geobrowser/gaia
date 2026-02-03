use hermes_schema::pb::governance::{
    proposal_action::Action, HermesProposalCreated, HermesProposalExecuted, HermesProposalUpdated,
    HermesProposalVoted, ProposalSettings, ProposalVoteOption, VotingMode as ProtoVotingMode,
};
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::governance::{
    ProposalActionItem, ProposalActionPayload, ProposalItem, ProposalVoteItem, VoteOption,
    VotingMode,
};

/// Result of processing a proposal creation
#[allow(dead_code)]
pub struct ProposalResult {
    pub proposal: ProposalItem,
    pub actions: Vec<ProposalActionItem>,
}

/// Result of processing a proposal execution
#[allow(dead_code)]
pub struct ProposalExecutionResult {
    pub proposal_id: Uuid,
    pub space_id: Uuid,
    pub executed_at: i64,
}

/// Process a HermesProposalCreated message
pub fn handle_proposal_created(
    msg: &HermesProposalCreated,
) -> Result<ProposalResult, HandlerError> {
    map_proposal_message(
        &msg.proposal_id,
        &msg.space_id,
        &msg.proposer_id,
        msg.voting_mode,
        msg.settings.as_ref(),
        &msg.actions,
        msg.meta.as_ref(),
    )
}

/// Process a HermesProposalUpdated message
pub fn handle_proposal_updated(
    msg: &HermesProposalUpdated,
) -> Result<ProposalResult, HandlerError> {
    map_proposal_message(
        &msg.proposal_id,
        &msg.space_id,
        &msg.proposer_id,
        msg.voting_mode,
        msg.settings.as_ref(),
        &msg.actions,
        msg.meta.as_ref(),
    )
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

    Ok(ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote,
        created_at,
        created_at_block,
    })
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

fn map_proposal_message(
    proposal_id: &[u8],
    space_id: &[u8],
    proposer_id: &[u8],
    voting_mode: i32,
    settings: Option<&ProposalSettings>,
    actions: &[hermes_schema::pb::governance::ProposalAction],
    meta: Option<&hermes_schema::pb::blockchain_metadata::BlockchainMetadata>,
) -> Result<ProposalResult, HandlerError> {
    let proposal_id = Uuid::from_slice(proposal_id)?;
    let space_id = Uuid::from_slice(space_id)?;
    let proposer_id = Uuid::from_slice(proposer_id)?;

    let voting_mode = match ProtoVotingMode::try_from(voting_mode) {
        Ok(ProtoVotingMode::Fast) => VotingMode::Fast,
        Ok(ProtoVotingMode::Slow) | Err(_) => VotingMode::Slow,
    };

    let settings = settings.ok_or(HandlerError::MissingPayload)?;
    let (created_at, created_at_block) = meta
        .map(|m| (m.created_at as i64, m.block_number as i64))
        .unwrap_or((0, 0));

    // For fast path, threshold is flat_threshold (absolute votes)
    // For slow path, threshold is percentage_threshold
    let threshold = match voting_mode {
        VotingMode::Fast => settings.flat_threshold as i64,
        VotingMode::Slow => settings.percentage_threshold as i64,
    };

    // Derive name from proto actions (before mapping to internal types)
    let name = derive_proposal_name(actions);

    // Map proto actions to internal types
    let actions: Vec<ProposalActionItem> = actions
        .iter()
        .enumerate()
        .map(|(index, action)| map_proposal_action(proposal_id, index as i32, action))
        .collect();

    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode,
        start_time: settings.start_date as i64,
        end_time: settings.last_date as i64,
        quorum: settings.quorum as i64,
        threshold,
        executed_at: None,
        created_at,
        created_at_block,
        name,
    };

    Ok(ProposalResult { proposal, actions })
}

/// Namespace UUID for generating deterministic action IDs
const PROPOSAL_ACTION_NAMESPACE: Uuid = Uuid::from_bytes([
    0x70, 0x72, 0x6f, 0x70, 0x6f, 0x73, 0x61, 0x6c, 0x5f, 0x61, 0x63, 0x74, 0x69, 0x6f, 0x6e, 0x73,
]);

fn map_proposal_action(
    proposal_id: Uuid,
    index: i32,
    action: &hermes_schema::pb::governance::ProposalAction,
) -> ProposalActionItem {
    // Generate deterministic UUID from proposal_id + index
    let id = Uuid::new_v5(
        &PROPOSAL_ACTION_NAMESPACE,
        format!("{}:{}", proposal_id, index).as_bytes(),
    );

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
            quorum: a.quorum,
            fast_threshold: a.fast_threshold,
            slow_threshold: a.slow_threshold,
            duration: a.duration,
        },
        None => ProposalActionPayload::Unknown,
    };

    ProposalActionItem {
        id,
        proposal_id,
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
}
