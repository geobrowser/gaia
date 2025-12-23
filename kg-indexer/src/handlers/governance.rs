use hermes_schema::pb::governance::{
    HermesProposalCreated, HermesProposalExecuted, HermesProposalVoted,
    ProposalVoteOption, VotingMode as ProtoVotingMode,
    proposal_action::Action,
};
use indexer_utils::checksum_address;
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::governance::{
    ProposalActionItem, ProposalActionPayload, ProposalItem, ProposalVoteItem, VoteOption, VotingMode,
};

/// Result of processing a proposal creation
pub struct ProposalResult {
    pub proposal: ProposalItem,
    pub actions: Vec<ProposalActionItem>,
}

/// Result of processing a proposal execution
pub struct ProposalExecutionResult {
    pub proposal_id: Uuid,
    pub space_id: Uuid,
    pub executed_at: i64,
}

/// Process a HermesProposalCreated message
pub fn handle_proposal_created(msg: &HermesProposalCreated) -> Result<ProposalResult, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;
    let proposer_id = Uuid::from_slice(&msg.proposer_id)?;

    let voting_mode = match ProtoVotingMode::try_from(msg.voting_mode) {
        Ok(ProtoVotingMode::Fast) => VotingMode::Fast,
        Ok(ProtoVotingMode::Slow) | Err(_) => VotingMode::Slow,
    };

    let settings = msg.settings.as_ref().ok_or(HandlerError::MissingPayload)?;
    let meta = msg.meta.as_ref();

    let (created_at, created_at_block) = meta
        .map(|m| (m.created_at as i64, m.block_number as i64))
        .unwrap_or((0, 0));

    // For fast path, threshold is flat_threshold (absolute votes)
    // For slow path, threshold is percentage_threshold
    let threshold = match voting_mode {
        VotingMode::Fast => settings.flat_threshold as i64,
        VotingMode::Slow => settings.percentage_threshold as i64,
    };

    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposer_id,
        voting_mode,
        start_date: settings.start_date as i64,
        end_date: settings.last_date as i64,
        quorum: settings.quorum as i64,
        threshold,
        executed_at: None,
        created_at,
        created_at_block,
    };

    let actions = msg
        .actions
        .iter()
        .enumerate()
        .map(|(index, action)| map_proposal_action(proposal_id, index as i32, action))
        .collect();

    Ok(ProposalResult { proposal, actions })
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
        Ok(ProposalVoteOption::VoteOptionNone) | Err(_) => VoteOption::None,
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

    let executed_at = msg
        .meta
        .as_ref()
        .map(|m| m.created_at as i64)
        .unwrap_or(0);

    Ok(ProposalExecutionResult {
        proposal_id,
        space_id,
        executed_at,
    })
}

fn map_proposal_action(
    proposal_id: Uuid,
    index: i32,
    action: &hermes_schema::pb::governance::ProposalAction,
) -> ProposalActionItem {
    let to_address = checksum_address(format!("0x{}", hex::encode(&action.to)));
    let value = format_u256(&action.value);

    let payload = match &action.action {
        Some(Action::AddMember(a)) => ProposalActionPayload::AddMember {
            target_address: checksum_address(format!("0x{}", hex::encode(&a.target_address))),
        },
        Some(Action::RemoveMember(a)) => ProposalActionPayload::RemoveMember {
            target_address: checksum_address(format!("0x{}", hex::encode(&a.target_address))),
        },
        Some(Action::AddEditor(a)) => ProposalActionPayload::AddEditor {
            target_address: checksum_address(format!("0x{}", hex::encode(&a.target_address))),
        },
        Some(Action::RemoveEditor(a)) => ProposalActionPayload::RemoveEditor {
            target_address: checksum_address(format!("0x{}", hex::encode(&a.target_address))),
        },
        Some(Action::UnflagEditor(a)) => ProposalActionPayload::UnflagEditor {
            target_address: checksum_address(format!("0x{}", hex::encode(&a.target_address))),
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
        proposal_id,
        index,
        to_address,
        value,
        data: action.data.clone(),
        payload,
    }
}

/// Format a big-endian bytes slice as a decimal string (for u256 values)
fn format_u256(bytes: &[u8]) -> String {
    if bytes.is_empty() || bytes.iter().all(|&b| b == 0) {
        return "0".to_string();
    }
    // For simplicity, just hex encode - proper u256 decimal conversion would need a bigint library
    format!("0x{}", hex::encode(bytes))
}
