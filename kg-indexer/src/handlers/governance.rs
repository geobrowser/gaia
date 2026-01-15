use hermes_schema::pb::governance::{
    proposal_action::Action, HermesProposalCreated, HermesProposalExecuted, HermesProposalUpdated,
    HermesProposalVoted, ProposalSettings, ProposalVoteOption, VotingMode as ProtoVotingMode,
};
use indexer_utils::checksum_address;
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
    };

    let actions = actions
        .iter()
        .enumerate()
        .map(|(index, action)| map_proposal_action(proposal_id, index as i32, action))
        .collect();

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
        id,
        proposal_id,
        payload,
    }
}
