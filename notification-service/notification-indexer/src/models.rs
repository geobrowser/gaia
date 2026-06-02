//! Domain models for notification events.

use hermes_schema::pb::governance::{
    proposal_action, HermesProposalCreated, HermesProposalExecuted, HermesProposalSettingsUpdated,
    HermesProposalUpdated, HermesProposalVoted, ProposalVoteOption,
};
use serde::Serialize;
use uuid::Uuid;

use crate::error::HandlerError;

/// Notification event types sent to webhooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationEventType {
    // Governance events
    ProposalCreated,
    ProposalUpdated,
    ProposalVoted,
    ProposalExecuted,
    ProposalSettingsUpdated,
    ProposalRejected,
    // Bounty events
    BountyInterest,
    BountyAllocated,
    BountyPayout,
}

impl NotificationEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotificationEventType::ProposalCreated => "proposal_created",
            NotificationEventType::ProposalUpdated => "proposal_updated",
            NotificationEventType::ProposalVoted => "proposal_voted",
            NotificationEventType::ProposalExecuted => "proposal_executed",
            NotificationEventType::ProposalSettingsUpdated => "proposal_settings_updated",
            NotificationEventType::ProposalRejected => "proposal_rejected",
            NotificationEventType::BountyInterest => "bounty_interest",
            NotificationEventType::BountyAllocated => "bounty_allocated",
            NotificationEventType::BountyPayout => "bounty_payout",
        }
    }

    /// Event category for filtering — app servers can use this to route notifications.
    pub fn category(&self) -> &'static str {
        match self {
            NotificationEventType::ProposalCreated
            | NotificationEventType::ProposalUpdated
            | NotificationEventType::ProposalVoted
            | NotificationEventType::ProposalExecuted
            | NotificationEventType::ProposalSettingsUpdated
            | NotificationEventType::ProposalRejected => "governance",
            NotificationEventType::BountyInterest
            | NotificationEventType::BountyAllocated
            | NotificationEventType::BountyPayout => "bounty",
        }
    }
}

/// Webhook payload version. Increment when the payload schema changes
/// in a backwards-incompatible way so consumers can handle both formats.
pub const PAYLOAD_VERSION: u32 = 1;

/// Webhook notification payload sent to app servers.
///
/// Common fields are shared across all event types. Event-specific fields
/// are in the `data` enum, which serializes flat (no nesting) via `serde(flatten)`.
#[derive(Debug, Clone, Serialize)]
pub struct NotificationPayload {
    pub version: u32,
    pub event_type: String,
    pub category: String,
    pub space_id: String,
    /// The recipient's account space UUID.
    /// Set by the notification-indexer during per-user fan-out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_space_id: Option<String>,
    /// Unique key for deduplication. Set by the storage layer during insert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    /// Human-readable space name (best-effort, from KG values table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_name: Option<String>,
    /// Event-specific fields (governance or bounty).
    #[serde(flatten)]
    pub data: NotificationData,
}

/// Event-specific payload data. Serialized flat into the parent payload.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum NotificationData {
    Governance(GovernanceData),
    Bounty(BountyData),
}

/// Governance-specific payload fields.
#[derive(Debug, Clone, Serialize)]
pub struct GovernanceData {
    pub proposal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voter_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voting_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<ActionSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<ProposalSettingsPayload>,
    /// Human-readable proposal name (best-effort, from proposals table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_name: Option<String>,
    /// Human-readable proposer name (best-effort, from KG values table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposer_name: Option<String>,
    /// Human-readable voter name (best-effort, from KG values table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voter_name: Option<String>,
    /// Current vote tallies (best-effort, from proposals table). Present on `proposal_voted` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abstain_count: Option<i64>,
}

/// Summary of a single proposal action for the webhook payload.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ActionSummary {
    /// Action type: "add_member", "remove_editor", "publish", etc.
    #[serde(rename = "type")]
    pub action_type: String,
    /// Target address (hex-encoded, for member/editor actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_address: Option<String>,
    /// Target space ID (for subspace actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_space_id: Option<String>,
    /// Target topic ID (for subspace topic actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_topic_id: Option<String>,
    /// Content URI (for publish actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_uri: Option<String>,
    /// Edit name (for publish actions, if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Voting settings details (for update_voting_settings actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voting_settings: Option<VotingSettingsUpdate>,
}

/// Details of a voting settings update action.
#[derive(Debug, Clone, Serialize)]
pub struct VotingSettingsUpdate {
    pub quorum: u64,
    pub fast_threshold: u64,
    pub slow_threshold: u64,
    pub duration: u64,
}

/// Proposal voting settings for the webhook payload.
#[derive(Debug, Clone, Serialize)]
pub struct ProposalSettingsPayload {
    pub start_date: u64,
    pub end_date: u64,
    pub voting_mode: String,
    pub quorum: u64,
    pub flat_threshold: u64,
    pub percentage_threshold: u64,
}

/// Bounty-specific payload fields.
#[derive(Debug, Clone, Serialize)]
pub struct BountyData {
    pub bounty_entity_id: String,
    pub relation_id: String,
    pub curator_space_id: String,
    pub bounty_space_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interested_user_space_id: Option<String>,
    /// Human-readable bounty name (best-effort, from KG values table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounty_name: Option<String>,
    /// Human-readable curator name (best-effort, from KG values table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curator_name: Option<String>,
}

/// Result of handling a governance event: payload + idempotency key.
#[derive(Debug)]
pub struct NotificationEvent {
    pub event_type: NotificationEventType,
    pub idempotency_key: String,
    pub payload: NotificationPayload,
}

impl NotificationEvent {
    /// The governance proposal id this event concerns, parsed from the payload.
    ///
    /// Returns `None` for non-governance events or an unparseable id. Used to
    /// resolve targeted recipients (the proposer for voted/executed events, the
    /// prior voters for an updated proposal).
    pub fn governance_proposal_id(&self) -> Option<Uuid> {
        match &self.payload.data {
            NotificationData::Governance(gov) => Uuid::parse_str(&gov.proposal_id).ok(),
            NotificationData::Bounty(_) => None,
        }
    }
}

/// Build a notification event from a PROPOSAL_CREATED protobuf message.
pub fn handle_proposal_created(
    msg: &HermesProposalCreated,
) -> Result<NotificationEvent, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;
    let proposer_id = Uuid::from_slice(&msg.proposer_id)?;

    let meta = msg.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = meta.sequence;
    let timestamp = meta.created_at;

    let idempotency_base = format!("{}:{}:proposal_created", block_number, sequence);

    Ok(NotificationEvent {
        event_type: NotificationEventType::ProposalCreated,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::ProposalCreated.as_str().to_string(),
            category: NotificationEventType::ProposalCreated
                .category()
                .to_string(),
            space_id: space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(block_number),
            timestamp: Some(timestamp),
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: Some(proposer_id.to_string()),
                voter_id: None,
                vote: None,
                voting_mode: Some(voting_mode_to_string(msg.voting_mode)?),
                actions: Some(msg.actions.iter().map(action_to_summary).collect()),
                settings: msg.settings.as_ref().map(settings_to_payload).transpose()?,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    })
}

/// Build a notification event from a PROPOSAL_UPDATED protobuf message.
pub fn handle_proposal_updated(
    msg: &HermesProposalUpdated,
) -> Result<NotificationEvent, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;
    let proposer_id = Uuid::from_slice(&msg.proposer_id)?;

    let meta = msg.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = meta.sequence;
    let timestamp = meta.created_at;

    let idempotency_base = format!("{}:{}:proposal_updated", block_number, sequence);

    Ok(NotificationEvent {
        event_type: NotificationEventType::ProposalUpdated,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::ProposalUpdated.as_str().to_string(),
            category: NotificationEventType::ProposalUpdated
                .category()
                .to_string(),
            space_id: space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(block_number),
            timestamp: Some(timestamp),
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: Some(proposer_id.to_string()),
                voter_id: None,
                vote: None,
                voting_mode: Some(voting_mode_to_string(msg.voting_mode)?),
                actions: Some(msg.actions.iter().map(action_to_summary).collect()),
                settings: msg.settings.as_ref().map(settings_to_payload).transpose()?,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    })
}

/// Map a protobuf voting mode to a string.
fn voting_mode_to_string(mode: i32) -> Result<String, HandlerError> {
    use hermes_schema::pb::governance::VotingMode;
    match VotingMode::try_from(mode) {
        Ok(VotingMode::Fast) => Ok("fast".to_string()),
        Ok(VotingMode::Slow) => Ok("slow".to_string()),
        Err(_) => Err(HandlerError::InvalidVotingMode(mode)),
    }
}

/// Convert protobuf ProposalSettings to payload struct.
fn settings_to_payload(
    settings: &hermes_schema::pb::governance::ProposalSettings,
) -> Result<ProposalSettingsPayload, HandlerError> {
    Ok(ProposalSettingsPayload {
        start_date: settings.start_date,
        end_date: settings.last_date,
        voting_mode: voting_mode_to_string(settings.voting_mode)?,
        quorum: settings.quorum,
        flat_threshold: settings.flat_threshold,
        percentage_threshold: settings.percentage_threshold,
    })
}

/// Convert a protobuf ProposalAction to a webhook-friendly summary.
fn action_to_summary(action: &hermes_schema::pb::governance::ProposalAction) -> ActionSummary {
    match &action.action {
        Some(proposal_action::Action::AddMember(a)) => ActionSummary {
            action_type: "add_member".to_string(),
            target_address: Some(hex::encode(&a.target_address)),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::RemoveMember(a)) => ActionSummary {
            action_type: "remove_member".to_string(),
            target_address: Some(hex::encode(&a.target_address)),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::AddEditor(a)) => ActionSummary {
            action_type: "add_editor".to_string(),
            target_address: Some(hex::encode(&a.target_address)),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::RemoveEditor(a)) => ActionSummary {
            action_type: "remove_editor".to_string(),
            target_address: Some(hex::encode(&a.target_address)),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::UnflagEditor(a)) => ActionSummary {
            action_type: "unflag_editor".to_string(),
            target_address: Some(hex::encode(&a.target_address)),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::Publish(p)) => ActionSummary {
            action_type: "publish".to_string(),
            content_uri: if p.content_uri.is_empty() {
                None
            } else {
                Some(p.content_uri.clone())
            },
            name: if p.name.is_empty() {
                None
            } else {
                Some(p.name.clone())
            },
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::Flag(f)) => ActionSummary {
            action_type: "flag".to_string(),
            target_address: Some(hex::encode(&f.content_id)),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::Unflag(u)) => ActionSummary {
            action_type: "unflag".to_string(),
            target_address: Some(hex::encode(&u.content_id)),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::UpdateVotingSettings(s)) => ActionSummary {
            action_type: "update_voting_settings".to_string(),
            voting_settings: Some(VotingSettingsUpdate {
                quorum: s.quorum,
                fast_threshold: s.fast_threshold,
                slow_threshold: s.slow_threshold,
                duration: s.duration,
            }),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::SubspaceVerified(s)) => ActionSummary {
            action_type: "subspace_verified".to_string(),
            target_space_id: Uuid::from_slice(&s.target_space_id)
                .ok()
                .map(|u| u.to_string()),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::SubspaceUnverified(s)) => ActionSummary {
            action_type: "subspace_unverified".to_string(),
            target_space_id: Uuid::from_slice(&s.target_space_id)
                .ok()
                .map(|u| u.to_string()),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::SubspaceRelated(s)) => ActionSummary {
            action_type: "subspace_related".to_string(),
            target_space_id: Uuid::from_slice(&s.target_space_id)
                .ok()
                .map(|u| u.to_string()),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::SubspaceUnrelated(s)) => ActionSummary {
            action_type: "subspace_unrelated".to_string(),
            target_space_id: Uuid::from_slice(&s.target_space_id)
                .ok()
                .map(|u| u.to_string()),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::SubspaceTopicDeclared(t)) => ActionSummary {
            action_type: "subspace_topic_declared".to_string(),
            target_topic_id: Uuid::from_slice(&t.target_topic_id)
                .ok()
                .map(|u| u.to_string()),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::SubspaceTopicRemoved(t)) => ActionSummary {
            action_type: "subspace_topic_removed".to_string(),
            target_topic_id: Uuid::from_slice(&t.target_topic_id)
                .ok()
                .map(|u| u.to_string()),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::SetTopic(t)) => ActionSummary {
            action_type: "set_topic".to_string(),
            target_topic_id: Uuid::from_slice(&t.target_topic_id)
                .ok()
                .map(|u| u.to_string()),
            ..ActionSummary::default()
        },
        Some(proposal_action::Action::UnsetTopic(_)) => ActionSummary {
            action_type: "unset_topic".to_string(),
            ..ActionSummary::default()
        },
        None => ActionSummary {
            action_type: "unknown".to_string(),
            ..ActionSummary::default()
        },
    }
}

/// Map a protobuf vote option to a string for the webhook payload.
fn vote_option_to_string(vote: i32) -> Result<String, HandlerError> {
    match ProposalVoteOption::try_from(vote) {
        Ok(ProposalVoteOption::VoteOptionYes) => Ok("yes".to_string()),
        Ok(ProposalVoteOption::VoteOptionNo) => Ok("no".to_string()),
        Ok(ProposalVoteOption::VoteOptionAbstain) => Ok("abstain".to_string()),
        _ => Err(HandlerError::InvalidVoteOption(vote)),
    }
}

/// Build a notification event from a PROPOSAL_VOTED protobuf message.
pub fn handle_proposal_voted(msg: &HermesProposalVoted) -> Result<NotificationEvent, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;
    let voter_id = Uuid::from_slice(&msg.voter_id)?;

    let meta = msg.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = meta.sequence;
    let timestamp = meta.created_at;

    let idempotency_base = format!("{}:{}:proposal_voted", block_number, sequence);

    Ok(NotificationEvent {
        event_type: NotificationEventType::ProposalVoted,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::ProposalVoted.as_str().to_string(),
            category: NotificationEventType::ProposalVoted.category().to_string(),
            space_id: space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(block_number),
            timestamp: Some(timestamp),
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: None,
                voter_id: Some(voter_id.to_string()),
                vote: Some(vote_option_to_string(msg.vote)?),
                voting_mode: None,
                actions: None,
                settings: None,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    })
}

/// Build a notification event from a PROPOSAL_EXECUTED protobuf message.
pub fn handle_proposal_executed(
    msg: &HermesProposalExecuted,
) -> Result<NotificationEvent, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;

    let meta = msg.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = meta.sequence;
    let timestamp = meta.created_at;

    let idempotency_base = format!("{}:{}:proposal_executed", block_number, sequence);

    Ok(NotificationEvent {
        event_type: NotificationEventType::ProposalExecuted,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::ProposalExecuted.as_str().to_string(),
            category: NotificationEventType::ProposalExecuted
                .category()
                .to_string(),
            space_id: space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(block_number),
            timestamp: Some(timestamp),
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: None,
                voter_id: None,
                vote: None,
                voting_mode: None,
                actions: None,
                settings: None,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    })
}

/// Build a notification event from a PROPOSAL_SETTINGS_UPDATED protobuf message.
pub fn handle_proposal_settings_updated(
    msg: &HermesProposalSettingsUpdated,
) -> Result<NotificationEvent, HandlerError> {
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;
    let space_id = Uuid::from_slice(&msg.space_id)?;

    let meta = msg.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = meta.sequence;
    let timestamp = meta.created_at;

    let idempotency_base = format!("{}:{}:proposal_settings_updated", block_number, sequence);

    Ok(NotificationEvent {
        event_type: NotificationEventType::ProposalSettingsUpdated,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::ProposalSettingsUpdated
                .as_str()
                .to_string(),
            category: NotificationEventType::ProposalSettingsUpdated
                .category()
                .to_string(),
            space_id: space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(block_number),
            timestamp: Some(timestamp),
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: None,
                voter_id: None,
                vote: None,
                voting_mode: msg
                    .settings
                    .as_ref()
                    .map(|s| voting_mode_to_string(s.voting_mode))
                    .transpose()?,
                actions: None,
                settings: msg.settings.as_ref().map(settings_to_payload).transpose()?,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    })
}

/// Build a notification event for a rejected proposal (expired without execution).
pub fn build_rejection_event(
    proposal_id: Uuid,
    space_id: Uuid,
    proposed_by: Uuid,
    end_time: i64,
) -> NotificationEvent {
    // Rejections have no block/sequence — use proposal_id as the unique component
    // (a proposal can only be rejected once).
    let idempotency_base = format!("{}:proposal_rejected", proposal_id);

    NotificationEvent {
        event_type: NotificationEventType::ProposalRejected,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::ProposalRejected.as_str().to_string(),
            category: NotificationEventType::ProposalRejected
                .category()
                .to_string(),
            space_id: space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: None,
            timestamp: Some(end_time as u64),
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: Some(proposed_by.to_string()),
                voter_id: None,
                vote: None,
                voting_mode: None,
                actions: None,
                settings: None,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// Bounty configuration and handlers
// ---------------------------------------------------------------------------

/// Well-known relation type UUIDs for bounty events.
/// Sourced from the curator-app (packages/curator-utils/src/ids.ts); env vars override if set.
///
/// Interest:  INTERESTED_IN_PROPERTY_ID — curator (Person) → bounty, in curator's personal space
/// Allocated: ALLOCATED_PROPERTY_ID     — bounty → person, in public space (optional DAO proposal)
/// Payout:    PAYOUT_RECIPIENT_PROPERTY_ID — space → recipient space, creates Payout entity
///            (Payout is a multi-relation entity; we detect it by the recipient relation type)
const DEFAULT_INTEREST_TYPE_ID: &str = "ff7e1b44-44a2-4191-8732-4e6c222afe07";
const DEFAULT_ALLOCATED_TYPE_ID: &str = "cfeb6422-23c5-4df4-b3f9-375a489d9e22";
const DEFAULT_PAYOUT_TYPE_ID: &str = "fddacaae-8513-8a43-ec1a-50ff71564d42";

/// Configuration for bounty relation type detection.
#[derive(Debug, Clone)]
pub struct BountyConfig {
    pub interest_type_id: Uuid,
    pub allocated_type_id: Uuid,
    pub payout_type_id: Uuid,
}

impl Default for BountyConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl BountyConfig {
    /// Create config from hardcoded defaults with optional env var overrides.
    pub fn new() -> Self {
        let interest = std::env::var("BOUNTY_INTEREST_RELATION_TYPE_ID")
            .ok()
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::parse_str(DEFAULT_INTEREST_TYPE_ID).expect("valid UUID"));

        let allocated = std::env::var("BOUNTY_ALLOCATED_RELATION_TYPE_ID")
            .ok()
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::parse_str(DEFAULT_ALLOCATED_TYPE_ID).expect("valid UUID"));

        let payout = std::env::var("BOUNTY_PAYOUT_RELATION_TYPE_ID")
            .ok()
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::parse_str(DEFAULT_PAYOUT_TYPE_ID).expect("valid UUID"));

        Self {
            interest_type_id: interest,
            allocated_type_id: allocated,
            payout_type_id: payout,
        }
    }

    /// Check if a relation type matches any bounty type.
    pub fn match_type(&self, relation_type: &Uuid) -> Option<NotificationEventType> {
        if *relation_type == self.interest_type_id {
            Some(NotificationEventType::BountyInterest)
        } else if *relation_type == self.allocated_type_id {
            Some(NotificationEventType::BountyAllocated)
        } else if *relation_type == self.payout_type_id {
            Some(NotificationEventType::BountyPayout)
        } else {
            None
        }
    }
}

/// Decoded bounty relation fields extracted from a GRC-20 CreateRelation.
#[derive(Debug, Clone)]
pub struct BountyRelationInfo {
    pub relation_id: Uuid,
    pub bounty_entity_id: Uuid,
    /// The curator's entity ID (rel.to for allocated/payout).
    /// Used for entity→space DB lookup when curator_space_id is nil.
    pub curator_entity_id: Uuid,
    pub curator_space_id: Uuid,
    pub bounty_space_id: Uuid,
    pub proposal_id: Option<Uuid>,
    pub block_number: u64,
    pub sequence: u64,
    pub timestamp: u64,
}

/// Build a notification event for a bounty interest expression.
pub fn handle_bounty_interest(info: &BountyRelationInfo) -> NotificationEvent {
    let idempotency_base = format!("{}:{}:bounty_interest", info.block_number, info.sequence);

    NotificationEvent {
        event_type: NotificationEventType::BountyInterest,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::BountyInterest.as_str().to_string(),
            category: NotificationEventType::BountyInterest.category().to_string(),
            space_id: info.bounty_space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(info.block_number),
            timestamp: Some(info.timestamp),
            space_name: None,
            data: NotificationData::Bounty(BountyData {
                bounty_entity_id: info.bounty_entity_id.to_string(),
                relation_id: info.relation_id.to_string(),
                curator_space_id: info.curator_space_id.to_string(),
                bounty_space_id: info.bounty_space_id.to_string(),
                proposal_id: None,
                interested_user_space_id: Some(info.curator_space_id.to_string()),
                bounty_name: None,
                curator_name: None,
            }),
        },
    }
}

/// Build a notification event for a bounty allocation.
pub fn handle_bounty_allocated(info: &BountyRelationInfo) -> NotificationEvent {
    let idempotency_base = format!("{}:{}:bounty_allocated", info.block_number, info.sequence);

    NotificationEvent {
        event_type: NotificationEventType::BountyAllocated,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::BountyAllocated.as_str().to_string(),
            category: NotificationEventType::BountyAllocated
                .category()
                .to_string(),
            space_id: info.bounty_space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(info.block_number),
            timestamp: Some(info.timestamp),
            space_name: None,
            data: NotificationData::Bounty(BountyData {
                bounty_entity_id: info.bounty_entity_id.to_string(),
                relation_id: info.relation_id.to_string(),
                curator_space_id: info.curator_space_id.to_string(),
                bounty_space_id: info.bounty_space_id.to_string(),
                proposal_id: info.proposal_id.map(|p| p.to_string()),
                interested_user_space_id: None,
                bounty_name: None,
                curator_name: None,
            }),
        },
    }
}

/// Build a notification event for a bounty payout.
pub fn handle_bounty_payout(info: &BountyRelationInfo) -> NotificationEvent {
    let idempotency_base = format!("{}:{}:bounty_payout", info.block_number, info.sequence);

    NotificationEvent {
        event_type: NotificationEventType::BountyPayout,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::BountyPayout.as_str().to_string(),
            category: NotificationEventType::BountyPayout.category().to_string(),
            space_id: info.bounty_space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(info.block_number),
            timestamp: Some(info.timestamp),
            space_name: None,
            data: NotificationData::Bounty(BountyData {
                bounty_entity_id: info.bounty_entity_id.to_string(),
                relation_id: info.relation_id.to_string(),
                curator_space_id: info.curator_space_id.to_string(),
                bounty_space_id: info.bounty_space_id.to_string(),
                proposal_id: info.proposal_id.map(|p| p.to_string()),
                interested_user_space_id: None,
                bounty_name: None,
                curator_name: None,
            }),
        },
    }
}

/// Extract bounty-related CreateRelation operations from a HermesEdit.
///
/// Decodes the GRC-20 payload, iterates over ops, and returns a list of
/// `(BountyRelationInfo, NotificationEventType)` pairs for each matching
/// `CreateRelation` whose `relation_type` matches a bounty type.
pub fn extract_bounty_relations(
    edit: &hermes_schema::pb::knowledge::HermesEdit,
    config: &BountyConfig,
) -> Result<Vec<(BountyRelationInfo, NotificationEventType)>, crate::error::HandlerError> {
    use crate::error::HandlerError;

    let meta = edit.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = u64::from(meta.sequence);
    let timestamp = meta.created_at;

    let edit_space_id = Uuid::from_slice(&edit.space_id).map_err(HandlerError::Uuid)?;

    let decoded = grc_20::decode_edit(&edit.payload)
        .map_err(|e| HandlerError::Grc20Decode(format!("{}", e)))?;

    let mut results = Vec::new();
    for op in &decoded.ops {
        if let grc_20::Op::CreateRelation(rel) = op {
            let rel_type_uuid = Uuid::from_bytes(rel.relation_type);
            if let Some(event_type) = config.match_type(&rel_type_uuid) {
                let info = match event_type {
                    NotificationEventType::BountyInterest => {
                        // Interest: from=curator entity, to=bounty entity
                        // curator_space_id = HermesEdit.space_id (curator's personal space)
                        BountyRelationInfo {
                            relation_id: Uuid::from_bytes(rel.id),
                            bounty_entity_id: Uuid::from_bytes(rel.to),
                            curator_entity_id: Uuid::from_bytes(rel.from),
                            curator_space_id: edit_space_id,
                            bounty_space_id: Uuid::nil(), // Needs DB lookup
                            proposal_id: None,
                            block_number,
                            sequence,
                            timestamp,
                        }
                    }
                    NotificationEventType::BountyAllocated
                    | NotificationEventType::BountyPayout => {
                        // Allocated/Payout: from=bounty/space, to=person/recipient_space
                        let curator_space = rel.to_space.map(Uuid::from_bytes);
                        BountyRelationInfo {
                            relation_id: Uuid::from_bytes(rel.id),
                            bounty_entity_id: Uuid::from_bytes(rel.from),
                            curator_entity_id: Uuid::from_bytes(rel.to),
                            curator_space_id: curator_space.unwrap_or(Uuid::nil()),
                            bounty_space_id: edit_space_id,
                            proposal_id: None,
                            block_number,
                            sequence,
                            timestamp,
                        }
                    }
                    _ => continue,
                };
                results.push((info, event_type));
            }
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
    use hermes_schema::pb::governance::{
        HermesProposalCreated, HermesProposalExecuted, HermesProposalSettingsUpdated,
        HermesProposalUpdated, HermesProposalVoted, ProposalSettings,
    };

    fn make_test_uuid(byte: u8) -> Vec<u8> {
        vec![byte; 16]
    }

    fn make_metadata(block_number: u64, created_at: u64) -> BlockchainMetadata {
        BlockchainMetadata {
            block_number,
            created_at,
            created_by: vec![],
            cursor: String::new(),
            sequence: 0,
            is_last: false,
        }
    }

    // -----------------------------------------------------------------------
    // PROPOSAL_CREATED
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_proposal_created() {
        let msg = HermesProposalCreated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(12345, 1700000000)),
        };

        let event = handle_proposal_created(&msg).expect("should parse");
        assert_eq!(event.event_type, NotificationEventType::ProposalCreated);
        assert_eq!(event.payload.event_type, "proposal_created");
        assert_eq!(event.payload.category, "governance");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        let expected_space = Uuid::from_slice(&make_test_uuid(0x01)).expect("valid uuid");
        let expected_proposal = Uuid::from_slice(&make_test_uuid(0x03)).expect("valid uuid");
        let expected_proposer = Uuid::from_slice(&make_test_uuid(0x02)).expect("valid uuid");

        assert_eq!(json["space_id"], expected_space.to_string());
        assert_eq!(json["proposal_id"], expected_proposal.to_string());
        assert_eq!(json["proposer_id"], expected_proposer.to_string());
        assert_eq!(json["block_number"], 12345);
        assert_eq!(json["timestamp"], 1700000000);
        // user_space_id should be absent at handler level — stamped later by storage
        assert!(json.get("user_space_id").is_none());
        // bounty fields should be absent for governance events
        assert!(json.get("bounty_entity_id").is_none());
    }

    #[test]
    fn test_build_payload_proposal_created() {
        let msg = HermesProposalCreated {
            space_id: make_test_uuid(0xAA),
            proposer_id: make_test_uuid(0xBB),
            proposal_id: make_test_uuid(0xCC),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(100, 999)),
        };

        let event = handle_proposal_created(&msg).expect("should parse");
        let json = serde_json::to_value(&event.payload).expect("should serialize");

        assert_eq!(json["event_type"], "proposal_created");
        assert!(json["space_id"].is_string());
        assert!(json["proposal_id"].is_string());
        assert!(json["proposer_id"].is_string());
        assert_eq!(json["block_number"], 100);
        assert_eq!(json["timestamp"], 999);
        // user_space_id, voter_id, and vote should be absent when not set
        assert!(json.get("user_space_id").is_none());
        assert!(json.get("voter_id").is_none());
        assert!(json.get("vote").is_none());
    }

    #[test]
    fn test_user_space_id_stamped_onto_payload() {
        let msg = HermesProposalCreated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(100, 999)),
        };

        let event = handle_proposal_created(&msg).expect("should parse");

        // Simulate what storage does: stamp user_space_id onto payload
        let mut payload = event.payload.clone();
        let editor_id = Uuid::from_bytes([0xDD; 16]);
        payload.user_space_id = Some(editor_id.to_string());

        let json = serde_json::to_value(&payload).expect("should serialize");
        assert_eq!(json["user_space_id"], editor_id.to_string());
        // Other fields still present
        assert_eq!(json["event_type"], "proposal_created");
        assert!(json["proposal_id"].is_string());
    }

    #[test]
    fn test_idempotency_base_created() {
        let msg = HermesProposalCreated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(42, 100)),
        };

        let event = handle_proposal_created(&msg).expect("should parse");
        // Base key: {block_number}:{sequence}:{event_type}
        assert_eq!(event.idempotency_key, "42:0:proposal_created");
    }

    // -----------------------------------------------------------------------
    // PROPOSAL_UPDATED
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_proposal_updated() {
        let msg = HermesProposalUpdated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(555, 1700002000)),
        };

        let event = handle_proposal_updated(&msg).expect("should parse");
        assert_eq!(event.event_type, NotificationEventType::ProposalUpdated);
        assert_eq!(event.payload.event_type, "proposal_updated");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        let expected_proposer = Uuid::from_slice(&make_test_uuid(0x02)).expect("valid uuid");
        assert_eq!(json["proposer_id"], expected_proposer.to_string());
        assert_eq!(json["block_number"], 555);
        assert_eq!(json["timestamp"], 1700002000);
    }

    #[test]
    fn test_idempotency_base_updated() {
        let msg = HermesProposalUpdated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(77, 100)),
        };

        let event = handle_proposal_updated(&msg).expect("should parse");
        assert_eq!(event.idempotency_key, "77:0:proposal_updated");
    }

    // -----------------------------------------------------------------------
    // PROPOSAL_VOTED
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_proposal_voted() {
        let msg = HermesProposalVoted {
            voter_id: make_test_uuid(0x0A),
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            vote: ProposalVoteOption::VoteOptionYes as i32,
            meta: Some(make_metadata(888, 1700003000)),
        };

        let event = handle_proposal_voted(&msg).expect("should parse");
        assert_eq!(event.event_type, NotificationEventType::ProposalVoted);
        assert_eq!(event.payload.event_type, "proposal_voted");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        let expected_voter = Uuid::from_slice(&make_test_uuid(0x0A)).expect("valid uuid");
        assert_eq!(json["voter_id"], expected_voter.to_string());
        assert_eq!(json["vote"], "yes");
        assert!(json.get("proposer_id").is_none());
        assert_eq!(json["block_number"], 888);
        assert_eq!(json["timestamp"], 1700003000);
    }

    #[test]
    fn test_proposal_voted_no() {
        let msg = HermesProposalVoted {
            voter_id: make_test_uuid(0x0A),
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            vote: ProposalVoteOption::VoteOptionNo as i32,
            meta: Some(make_metadata(889, 1700003001)),
        };

        let event = handle_proposal_voted(&msg).expect("should parse");
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["vote"], "no");
    }

    #[test]
    fn test_proposal_voted_abstain() {
        let msg = HermesProposalVoted {
            voter_id: make_test_uuid(0x0A),
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            vote: ProposalVoteOption::VoteOptionAbstain as i32,
            meta: Some(make_metadata(890, 1700003002)),
        };

        let event = handle_proposal_voted(&msg).expect("should parse");
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["vote"], "abstain");
    }

    #[test]
    fn test_idempotency_base_voted() {
        let msg = HermesProposalVoted {
            voter_id: make_test_uuid(0x0A),
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            vote: ProposalVoteOption::VoteOptionYes as i32,
            meta: Some(make_metadata(42, 100)),
        };

        let event = handle_proposal_voted(&msg).expect("should parse");
        assert_eq!(event.idempotency_key, "42:0:proposal_voted");
    }

    #[test]
    fn test_build_payload_proposal_voted() {
        let msg = HermesProposalVoted {
            voter_id: make_test_uuid(0x0A),
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            vote: ProposalVoteOption::VoteOptionYes as i32,
            meta: Some(make_metadata(100, 999)),
        };

        let event = handle_proposal_voted(&msg).expect("should parse");
        let json = serde_json::to_value(&event.payload).expect("should serialize");

        assert_eq!(json["event_type"], "proposal_voted");
        assert!(json["voter_id"].is_string());
        assert_eq!(json["vote"], "yes");
        // proposer_id should be absent
        assert!(json.get("proposer_id").is_none());
    }

    // -----------------------------------------------------------------------
    // PROPOSAL_EXECUTED
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_proposal_executed() {
        let msg = HermesProposalExecuted {
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            meta: Some(make_metadata(99999, 1700001000)),
        };

        let event = handle_proposal_executed(&msg).expect("should parse");
        assert_eq!(event.event_type, NotificationEventType::ProposalExecuted);
        assert_eq!(event.payload.event_type, "proposal_executed");
        assert_eq!(event.payload.block_number, Some(99999));
        assert_eq!(event.payload.timestamp, Some(1700001000));
    }

    #[test]
    fn test_build_payload_proposal_executed() {
        let msg = HermesProposalExecuted {
            space_id: make_test_uuid(0xAA),
            proposal_id: make_test_uuid(0xCC),
            meta: Some(make_metadata(200, 1000)),
        };

        let event = handle_proposal_executed(&msg).expect("should parse");
        let json = serde_json::to_value(&event.payload).expect("should serialize");

        assert_eq!(json["event_type"], "proposal_executed");
        assert_eq!(json["block_number"], 200);
        assert_eq!(json["timestamp"], 1000);
    }

    // -----------------------------------------------------------------------
    // PROPOSAL_SETTINGS_UPDATED
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_proposal_settings_updated() {
        let msg = HermesProposalSettingsUpdated {
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            settings: Some(ProposalSettings {
                start_date: 1700000000,
                last_date: 1700086400,
                voting_mode: 1,
                quorum: 5,
                flat_threshold: 0,
                percentage_threshold: 5000000,
            }),
            meta: Some(make_metadata(777, 1700004000)),
        };

        let event = handle_proposal_settings_updated(&msg).expect("should parse");
        assert_eq!(
            event.event_type,
            NotificationEventType::ProposalSettingsUpdated
        );
        assert_eq!(event.payload.event_type, "proposal_settings_updated");
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["block_number"], 777);
        assert_eq!(json["timestamp"], 1700004000);
        assert!(json.get("proposer_id").is_none());
        assert!(json.get("voter_id").is_none());
    }

    #[test]
    fn test_idempotency_key_settings_updated() {
        let msg = HermesProposalSettingsUpdated {
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            settings: None,
            meta: Some(make_metadata(99, 100)),
        };

        let event = handle_proposal_settings_updated(&msg).expect("should parse");
        assert_eq!(event.idempotency_key, "99:0:proposal_settings_updated");
    }

    // -----------------------------------------------------------------------
    // Missing metadata
    // -----------------------------------------------------------------------

    #[test]
    fn test_missing_metadata_created() {
        let msg = HermesProposalCreated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: None,
        };
        assert!(matches!(
            handle_proposal_created(&msg).unwrap_err(),
            HandlerError::MissingMetadata
        ));
    }

    #[test]
    fn test_missing_metadata_updated() {
        let msg = HermesProposalUpdated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: None,
        };
        assert!(matches!(
            handle_proposal_updated(&msg).unwrap_err(),
            HandlerError::MissingMetadata
        ));
    }

    #[test]
    fn test_missing_metadata_voted() {
        let msg = HermesProposalVoted {
            voter_id: make_test_uuid(0x0A),
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            vote: 1,
            meta: None,
        };
        assert!(matches!(
            handle_proposal_voted(&msg).unwrap_err(),
            HandlerError::MissingMetadata
        ));
    }

    #[test]
    fn test_missing_metadata_settings_updated() {
        let msg = HermesProposalSettingsUpdated {
            space_id: make_test_uuid(0x01),
            proposal_id: make_test_uuid(0x03),
            settings: None,
            meta: None,
        };
        assert!(matches!(
            handle_proposal_settings_updated(&msg).unwrap_err(),
            HandlerError::MissingMetadata
        ));
    }

    // -----------------------------------------------------------------------
    // Rejection (off-chain)
    // -----------------------------------------------------------------------

    #[test]
    fn test_idempotency_key_rejection() {
        let proposal_id = Uuid::from_bytes([0x01; 16]);
        let space_id = Uuid::from_bytes([0x02; 16]);
        let proposed_by = Uuid::from_bytes([0x03; 16]);

        let event = build_rejection_event(proposal_id, space_id, proposed_by, 1700000000);
        assert_eq!(
            event.idempotency_key,
            format!("{}:proposal_rejected", proposal_id)
        );
        assert_eq!(event.event_type, NotificationEventType::ProposalRejected);
        assert_eq!(event.payload.event_type, "proposal_rejected");
    }

    // -----------------------------------------------------------------------
    // Bounty events
    // -----------------------------------------------------------------------

    fn make_bounty_info() -> BountyRelationInfo {
        BountyRelationInfo {
            relation_id: Uuid::from_bytes([0x10; 16]),
            bounty_entity_id: Uuid::from_bytes([0x20; 16]),
            curator_entity_id: Uuid::from_bytes([0x25; 16]),
            curator_space_id: Uuid::from_bytes([0x30; 16]),
            bounty_space_id: Uuid::from_bytes([0x40; 16]),
            proposal_id: None,
            block_number: 50000,
            sequence: 7,
            timestamp: 1700010000,
        }
    }

    #[test]
    fn test_bounty_interest_event() {
        let info = make_bounty_info();
        let event = handle_bounty_interest(&info);

        assert_eq!(event.event_type, NotificationEventType::BountyInterest);
        assert_eq!(event.idempotency_key, "50000:7:bounty_interest");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["event_type"], "bounty_interest");
        assert_eq!(json["category"], "bounty");
        assert_eq!(json["version"], 1);
        assert_eq!(json["space_id"], info.bounty_space_id.to_string());
        assert_eq!(json["block_number"], 50000);
        assert_eq!(json["timestamp"], 1700010000);
        assert_eq!(json["bounty_entity_id"], info.bounty_entity_id.to_string());
        assert_eq!(json["relation_id"], info.relation_id.to_string());
        assert_eq!(json["curator_space_id"], info.curator_space_id.to_string());
        assert_eq!(
            json["interested_user_space_id"],
            info.curator_space_id.to_string()
        );
        // No governance fields
        assert!(json.get("voter_id").is_none());
        assert!(json.get("proposer_id").is_none());
    }

    #[test]
    fn test_bounty_allocated_event() {
        let mut info = make_bounty_info();
        info.proposal_id = Some(Uuid::from_bytes([0x50; 16]));
        let event = handle_bounty_allocated(&info);

        assert_eq!(event.event_type, NotificationEventType::BountyAllocated);
        assert_eq!(event.idempotency_key, "50000:7:bounty_allocated");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["event_type"], "bounty_allocated");
        assert_eq!(json["category"], "bounty");
        assert_eq!(
            json["proposal_id"],
            Uuid::from_bytes([0x50; 16]).to_string()
        );
        // interested_user_space_id absent for allocation
        assert!(json.get("interested_user_space_id").is_none());
    }

    #[test]
    fn test_bounty_payout_event() {
        let mut info = make_bounty_info();
        info.proposal_id = Some(Uuid::from_bytes([0x60; 16]));
        let event = handle_bounty_payout(&info);

        assert_eq!(event.event_type, NotificationEventType::BountyPayout);
        assert_eq!(event.idempotency_key, "50000:7:bounty_payout");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["event_type"], "bounty_payout");
        assert_eq!(json["category"], "bounty");
        assert_eq!(
            json["proposal_id"],
            Uuid::from_bytes([0x60; 16]).to_string()
        );
        assert!(json.get("interested_user_space_id").is_none());
    }

    // -----------------------------------------------------------------------
    // Bounty config: exact UUID verification
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_config_default_ids_are_non_nil_and_distinct() {
        let config = BountyConfig::default();
        assert_ne!(config.interest_type_id, Uuid::nil());
        assert_ne!(config.allocated_type_id, Uuid::nil());
        assert_ne!(config.payout_type_id, Uuid::nil());
        assert_ne!(config.interest_type_id, config.allocated_type_id);
        assert_ne!(config.interest_type_id, config.payout_type_id);
        assert_ne!(config.allocated_type_id, config.payout_type_id);
    }

    #[test]
    fn test_bounty_config_exact_uuids_match_curator_app() {
        // These must match curator-app packages/curator-utils/src/ids.ts exactly.
        // If any of these fail, the notification service will not detect bounty events.
        let config = BountyConfig::default();
        assert_eq!(
            config.interest_type_id,
            Uuid::parse_str("ff7e1b44-44a2-4191-8732-4e6c222afe07").expect("valid"),
            "INTERESTED_IN_PROPERTY_ID must match curator-app"
        );
        assert_eq!(
            config.allocated_type_id,
            Uuid::parse_str("cfeb6422-23c5-4df4-b3f9-375a489d9e22").expect("valid"),
            "ALLOCATED_PROPERTY_ID must match curator-app"
        );
        assert_eq!(
            config.payout_type_id,
            Uuid::parse_str("fddacaae-8513-8a43-ec1a-50ff71564d42").expect("valid"),
            "PAYOUT_RECIPIENT_PROPERTY_ID must match curator-app"
        );
    }

    // -----------------------------------------------------------------------
    // Bounty config: match_type routing
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_config_match_type_routes_correctly() {
        let config = BountyConfig::default();

        assert_eq!(
            config.match_type(&config.interest_type_id),
            Some(NotificationEventType::BountyInterest)
        );
        assert_eq!(
            config.match_type(&config.allocated_type_id),
            Some(NotificationEventType::BountyAllocated)
        );
        assert_eq!(
            config.match_type(&config.payout_type_id),
            Some(NotificationEventType::BountyPayout)
        );
    }

    #[test]
    fn test_bounty_config_match_type_returns_none_for_unknown() {
        let config = BountyConfig::default();
        assert_eq!(config.match_type(&Uuid::from_bytes([0xFF; 16])), None);
        assert_eq!(config.match_type(&Uuid::nil()), None);
    }

    #[test]
    fn test_bounty_config_match_type_returns_none_for_old_interest_id() {
        // The old hardcoded interest ID should NOT match after the fix
        let config = BountyConfig::default();
        let old_interest_id =
            Uuid::parse_str("2c765cae-c1b6-4cc3-a65d-693d0a67eaeb").expect("valid");
        assert_eq!(config.match_type(&old_interest_id), None);
    }

    #[test]
    fn test_bounty_config_env_var_override() {
        // Verify the env var override mechanism works by constructing manually
        // (we can't set env vars safely in parallel tests, but we can test the
        // parsing logic via BountyConfig::new struct construction)
        let custom = BountyConfig {
            interest_type_id: Uuid::from_bytes([0x01; 16]),
            allocated_type_id: Uuid::from_bytes([0x02; 16]),
            payout_type_id: Uuid::from_bytes([0x03; 16]),
        };
        assert_eq!(
            custom.match_type(&Uuid::from_bytes([0x01; 16])),
            Some(NotificationEventType::BountyInterest)
        );
        assert_eq!(
            custom.match_type(&Uuid::from_bytes([0x02; 16])),
            Some(NotificationEventType::BountyAllocated)
        );
        assert_eq!(
            custom.match_type(&Uuid::from_bytes([0x03; 16])),
            Some(NotificationEventType::BountyPayout)
        );
        // Default IDs should NOT match when overridden
        let default_config = BountyConfig::default();
        assert_eq!(custom.match_type(&default_config.interest_type_id), None);
    }

    // -----------------------------------------------------------------------
    // Bounty interest events
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_interest_payload_structure() {
        let info = make_bounty_info();
        let event = handle_bounty_interest(&info);

        assert_eq!(event.event_type, NotificationEventType::BountyInterest);
        assert_eq!(event.idempotency_key, "50000:7:bounty_interest");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["event_type"], "bounty_interest");
        assert_eq!(json["category"], "bounty");
        assert_eq!(json["version"], 1);
        assert_eq!(json["space_id"], info.bounty_space_id.to_string());
        assert_eq!(json["block_number"], 50000);
        assert_eq!(json["timestamp"], 1700010000);
        assert_eq!(json["bounty_entity_id"], info.bounty_entity_id.to_string());
        assert_eq!(json["relation_id"], info.relation_id.to_string());
        assert_eq!(json["curator_space_id"], info.curator_space_id.to_string());
        assert_eq!(json["bounty_space_id"], info.bounty_space_id.to_string());
        // interested_user_space_id equals curator_space_id for interest events
        assert_eq!(
            json["interested_user_space_id"],
            info.curator_space_id.to_string()
        );
    }

    #[test]
    fn test_bounty_interest_has_no_proposal_id() {
        let info = make_bounty_info();
        let event = handle_bounty_interest(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        // Interest is direct publish, no DAO proposal involved
        assert!(json.get("proposal_id").is_none());
    }

    #[test]
    fn test_bounty_interest_has_no_governance_fields() {
        let info = make_bounty_info();
        let event = handle_bounty_interest(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert!(json.get("voter_id").is_none());
        assert!(json.get("proposer_id").is_none());
        assert!(json.get("vote").is_none());
        assert!(json.get("actions").is_none());
    }

    // -----------------------------------------------------------------------
    // Bounty allocation events
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_allocated_payload_structure() {
        let mut info = make_bounty_info();
        info.proposal_id = Some(Uuid::from_bytes([0x50; 16]));
        let event = handle_bounty_allocated(&info);

        assert_eq!(event.event_type, NotificationEventType::BountyAllocated);
        assert_eq!(event.idempotency_key, "50000:7:bounty_allocated");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["event_type"], "bounty_allocated");
        assert_eq!(json["category"], "bounty");
        assert_eq!(json["version"], 1);
        assert_eq!(json["space_id"], info.bounty_space_id.to_string());
        assert_eq!(json["bounty_entity_id"], info.bounty_entity_id.to_string());
        assert_eq!(json["relation_id"], info.relation_id.to_string());
        assert_eq!(json["curator_space_id"], info.curator_space_id.to_string());
        assert_eq!(json["bounty_space_id"], info.bounty_space_id.to_string());
        assert_eq!(
            json["proposal_id"],
            Uuid::from_bytes([0x50; 16]).to_string()
        );
    }

    #[test]
    fn test_bounty_allocated_without_proposal() {
        // Allocation in EOA (non-DAO) space — no proposal involved
        let info = make_bounty_info(); // proposal_id defaults to None
        let event = handle_bounty_allocated(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert!(json.get("proposal_id").is_none());
    }

    #[test]
    fn test_bounty_allocated_has_no_interested_user() {
        let mut info = make_bounty_info();
        info.proposal_id = Some(Uuid::from_bytes([0x50; 16]));
        let event = handle_bounty_allocated(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        // interested_user_space_id is only for interest events
        assert!(json.get("interested_user_space_id").is_none());
    }

    #[test]
    fn test_bounty_allocated_has_no_governance_fields() {
        let mut info = make_bounty_info();
        info.proposal_id = Some(Uuid::from_bytes([0x50; 16]));
        let event = handle_bounty_allocated(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert!(json.get("voter_id").is_none());
        assert!(json.get("proposer_id").is_none());
        assert!(json.get("vote").is_none());
    }

    // -----------------------------------------------------------------------
    // Bounty payout events
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_payout_payload_structure() {
        let mut info = make_bounty_info();
        info.proposal_id = Some(Uuid::from_bytes([0x60; 16]));
        let event = handle_bounty_payout(&info);

        assert_eq!(event.event_type, NotificationEventType::BountyPayout);
        assert_eq!(event.idempotency_key, "50000:7:bounty_payout");

        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert_eq!(json["event_type"], "bounty_payout");
        assert_eq!(json["category"], "bounty");
        assert_eq!(json["version"], 1);
        assert_eq!(json["space_id"], info.bounty_space_id.to_string());
        assert_eq!(json["bounty_entity_id"], info.bounty_entity_id.to_string());
        assert_eq!(json["relation_id"], info.relation_id.to_string());
        assert_eq!(json["curator_space_id"], info.curator_space_id.to_string());
        assert_eq!(json["bounty_space_id"], info.bounty_space_id.to_string());
        assert_eq!(
            json["proposal_id"],
            Uuid::from_bytes([0x60; 16]).to_string()
        );
    }

    #[test]
    fn test_bounty_payout_without_proposal() {
        // Payout in EOA space — direct publish, no proposal
        let info = make_bounty_info();
        let event = handle_bounty_payout(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert!(json.get("proposal_id").is_none());
    }

    #[test]
    fn test_bounty_payout_has_no_interested_user() {
        let mut info = make_bounty_info();
        info.proposal_id = Some(Uuid::from_bytes([0x60; 16]));
        let event = handle_bounty_payout(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert!(json.get("interested_user_space_id").is_none());
    }

    // -----------------------------------------------------------------------
    // Bounty event type categorization
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_event_types_are_bounty_category() {
        assert_eq!(NotificationEventType::BountyInterest.category(), "bounty");
        assert_eq!(NotificationEventType::BountyAllocated.category(), "bounty");
        assert_eq!(NotificationEventType::BountyPayout.category(), "bounty");
    }

    #[test]
    fn test_bounty_event_type_as_str() {
        assert_eq!(
            NotificationEventType::BountyInterest.as_str(),
            "bounty_interest"
        );
        assert_eq!(
            NotificationEventType::BountyAllocated.as_str(),
            "bounty_allocated"
        );
        assert_eq!(
            NotificationEventType::BountyPayout.as_str(),
            "bounty_payout"
        );
    }

    // -----------------------------------------------------------------------
    // Bounty idempotency keys
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_idempotency_keys_are_unique_per_event_type() {
        let info = make_bounty_info();
        let interest = handle_bounty_interest(&info);
        let allocated = handle_bounty_allocated(&info);
        let payout = handle_bounty_payout(&info);

        // Same block/sequence but different event types must produce different keys
        assert_ne!(interest.idempotency_key, allocated.idempotency_key);
        assert_ne!(interest.idempotency_key, payout.idempotency_key);
        assert_ne!(allocated.idempotency_key, payout.idempotency_key);
    }

    #[test]
    fn test_bounty_idempotency_keys_differ_by_block() {
        let mut info1 = make_bounty_info();
        let mut info2 = make_bounty_info();
        info2.block_number = 50001;

        let event1 = handle_bounty_interest(&info1);
        let event2 = handle_bounty_interest(&info2);
        assert_ne!(event1.idempotency_key, event2.idempotency_key);
    }

    #[test]
    fn test_bounty_idempotency_keys_differ_by_sequence() {
        let mut info1 = make_bounty_info();
        let mut info2 = make_bounty_info();
        info2.sequence = 8;

        let event1 = handle_bounty_interest(&info1);
        let event2 = handle_bounty_interest(&info2);
        assert_ne!(event1.idempotency_key, event2.idempotency_key);
    }

    // -----------------------------------------------------------------------
    // Bounty optional name fields (best-effort)
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_name_fields_absent_by_default() {
        let info = make_bounty_info();
        let event = handle_bounty_interest(&info);
        let json = serde_json::to_value(&event.payload).expect("should serialize");
        assert!(json.get("bounty_name").is_none());
        assert!(json.get("curator_name").is_none());
    }

    // -----------------------------------------------------------------------
    // Cross-category isolation: bounty events have no governance fields
    // -----------------------------------------------------------------------

    #[test]
    fn test_governance_events_have_no_bounty_fields() {
        let msg = HermesProposalCreated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(12345, 1700000000)),
        };
        let event = handle_proposal_created(&msg).expect("should parse");
        let json = serde_json::to_value(&event.payload).expect("should serialize");

        assert!(json.get("bounty_entity_id").is_none());
        assert!(json.get("bounty_space_id").is_none());
        assert!(json.get("curator_space_id").is_none());
        assert!(json.get("interested_user_space_id").is_none());
        assert!(json.get("bounty_name").is_none());
        assert!(json.get("curator_name").is_none());
    }

    #[test]
    fn test_bounty_events_have_no_governance_specific_fields() {
        let info = make_bounty_info();
        for event in [
            handle_bounty_interest(&info),
            handle_bounty_allocated(&info),
            handle_bounty_payout(&info),
        ] {
            let json = serde_json::to_value(&event.payload).expect("should serialize");
            assert!(
                json.get("voter_id").is_none(),
                "bounty event should not have voter_id"
            );
            assert!(
                json.get("proposer_id").is_none(),
                "bounty event should not have proposer_id"
            );
            assert!(
                json.get("vote").is_none(),
                "bounty event should not have vote"
            );
            assert!(
                json.get("actions").is_none(),
                "bounty event should not have actions"
            );
            assert!(
                json.get("settings").is_none(),
                "bounty event should not have settings"
            );
            assert!(
                json.get("voting_mode").is_none(),
                "bounty event should not have voting_mode"
            );
        }
    }

    // -----------------------------------------------------------------------
    // extract_bounty_relations()
    // -----------------------------------------------------------------------

    /// Build a minimal valid GRC-20 edit with a single CreateRelation op,
    /// encode it, and wrap in a HermesEdit protobuf.
    fn make_hermes_edit_with_relation(
        relation_type: [u8; 16],
        from: [u8; 16],
        to: [u8; 16],
        space_id: [u8; 16],
        to_space: Option<[u8; 16]>,
    ) -> hermes_schema::pb::knowledge::HermesEdit {
        use std::borrow::Cow;

        let edit = grc_20::Edit {
            id: [0x99; 16],
            name: Cow::Borrowed("test edit"),
            authors: vec![[0xAA; 16]],
            created_at: 1700000000,
            ops: vec![grc_20::Op::CreateRelation(grc_20::CreateRelation {
                id: [0x77; 16],
                relation_type,
                from,
                from_is_value_ref: false,
                to,
                to_is_value_ref: false,
                from_space: None,
                from_version: None,
                to_space,
                to_version: None,
                entity: None,
                position: None,
                context: None,
            })],
        };
        let payload = grc_20::encode_edit(&edit).expect("encode should succeed");

        hermes_schema::pb::knowledge::HermesEdit {
            id: vec![0x88; 16],
            name: "test".into(),
            payload,
            authors: vec![vec![0xAA; 16]],
            language: None,
            space_id: space_id.to_vec(),
            is_canonical: true,
            meta: Some(BlockchainMetadata {
                block_number: 12345,
                created_at: 1700000000,
                created_by: vec![],
                cursor: String::new(),
                sequence: 3,
                is_last: false,
            }),
        }
    }

    #[test]
    fn test_extract_bounty_relations_interest() {
        let config = BountyConfig::default();
        let interest_bytes = config.interest_type_id.into_bytes();
        let hermes_edit = make_hermes_edit_with_relation(
            interest_bytes,
            [0xCC; 16], // from = curator entity
            [0xDD; 16], // to = bounty entity
            [0xEE; 16], // space_id = curator's personal space
            None,
        );

        let results = extract_bounty_relations(&hermes_edit, &config).expect("should extract");
        assert_eq!(results.len(), 1);
        let (info, event_type) = &results[0];
        assert_eq!(*event_type, NotificationEventType::BountyInterest);
        assert_eq!(info.bounty_entity_id, Uuid::from_bytes([0xDD; 16]));
        assert_eq!(info.curator_space_id, Uuid::from_bytes([0xEE; 16]));
        assert_eq!(info.bounty_space_id, Uuid::nil()); // needs DB lookup
        assert_eq!(info.block_number, 12345);
        assert_eq!(info.sequence, 3);
    }

    #[test]
    fn test_extract_bounty_relations_allocated() {
        let config = BountyConfig::default();
        let allocated_bytes = config.allocated_type_id.into_bytes();
        let hermes_edit = make_hermes_edit_with_relation(
            allocated_bytes,
            [0xDD; 16],       // from = bounty entity
            [0xCC; 16],       // to = person entity
            [0xEE; 16],       // space_id = bounty space
            Some([0xFF; 16]), // to_space = curator personal space
        );

        let results = extract_bounty_relations(&hermes_edit, &config).expect("should extract");
        assert_eq!(results.len(), 1);
        let (info, event_type) = &results[0];
        assert_eq!(*event_type, NotificationEventType::BountyAllocated);
        assert_eq!(info.bounty_entity_id, Uuid::from_bytes([0xDD; 16]));
        assert_eq!(info.curator_space_id, Uuid::from_bytes([0xFF; 16]));
        assert_eq!(info.bounty_space_id, Uuid::from_bytes([0xEE; 16]));
    }

    #[test]
    fn test_extract_bounty_relations_payout() {
        let config = BountyConfig::default();
        let payout_bytes = config.payout_type_id.into_bytes();
        let hermes_edit = make_hermes_edit_with_relation(
            payout_bytes,
            [0xDD; 16], // from = space/bounty
            [0xCC; 16], // to = recipient space
            [0xEE; 16], // space_id = bounty space
            Some([0xFF; 16]),
        );

        let results = extract_bounty_relations(&hermes_edit, &config).expect("should extract");
        assert_eq!(results.len(), 1);
        let (_info, event_type) = &results[0];
        assert_eq!(*event_type, NotificationEventType::BountyPayout);
    }

    #[test]
    fn test_extract_bounty_relations_non_bounty_returns_empty() {
        let config = BountyConfig::default();
        let random_type = [0x11; 16]; // not a bounty relation type
        let hermes_edit =
            make_hermes_edit_with_relation(random_type, [0xCC; 16], [0xDD; 16], [0xEE; 16], None);

        let results = extract_bounty_relations(&hermes_edit, &config).expect("should extract");
        assert!(results.is_empty());
    }

    #[test]
    fn test_extract_bounty_relations_mixed_ops() {
        use std::borrow::Cow;

        let config = BountyConfig::default();
        let interest_bytes = config.interest_type_id.into_bytes();

        // Build an edit with one bounty and one non-bounty CreateRelation
        let edit = grc_20::Edit {
            id: [0x99; 16],
            name: Cow::Borrowed("mixed"),
            authors: vec![[0xAA; 16]],
            created_at: 1700000000,
            ops: vec![
                grc_20::Op::CreateRelation(grc_20::CreateRelation {
                    id: [0x77; 16],
                    relation_type: interest_bytes,
                    from: [0xCC; 16],
                    from_is_value_ref: false,
                    to: [0xDD; 16],
                    to_is_value_ref: false,
                    from_space: None,
                    from_version: None,
                    to_space: None,
                    to_version: None,
                    entity: None,
                    position: None,
                    context: None,
                }),
                grc_20::Op::CreateRelation(grc_20::CreateRelation {
                    id: [0x78; 16],
                    relation_type: [0x11; 16], // non-bounty
                    from: [0xCC; 16],
                    from_is_value_ref: false,
                    to: [0xDD; 16],
                    to_is_value_ref: false,
                    from_space: None,
                    from_version: None,
                    to_space: None,
                    to_version: None,
                    entity: None,
                    position: None,
                    context: None,
                }),
                grc_20::Op::CreateEntity(grc_20::CreateEntity {
                    id: [0x79; 16],
                    values: vec![],
                    context: None,
                }),
            ],
        };
        let payload = grc_20::encode_edit(&edit).expect("encode should succeed");

        let hermes_edit = hermes_schema::pb::knowledge::HermesEdit {
            id: vec![0x88; 16],
            name: "mixed".into(),
            payload,
            authors: vec![vec![0xAA; 16]],
            language: None,
            space_id: vec![0xEE; 16],
            is_canonical: true,
            meta: Some(BlockchainMetadata {
                block_number: 100,
                created_at: 1700000000,
                created_by: vec![],
                cursor: String::new(),
                sequence: 0,
                is_last: false,
            }),
        };

        let results = extract_bounty_relations(&hermes_edit, &config).expect("should extract");
        assert_eq!(
            results.len(),
            1,
            "only the bounty relation should be extracted"
        );
        assert_eq!(results[0].1, NotificationEventType::BountyInterest);
    }

    #[test]
    fn test_extract_bounty_relations_missing_metadata() {
        let config = BountyConfig::default();
        let hermes_edit = hermes_schema::pb::knowledge::HermesEdit {
            id: vec![0x88; 16],
            name: "test".into(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: vec![0xEE; 16],
            is_canonical: true,
            meta: None,
        };

        let result = extract_bounty_relations(&hermes_edit, &config);
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("should fail"),
            crate::error::HandlerError::MissingMetadata
        ));
    }

    #[test]
    fn test_extract_bounty_relations_invalid_payload() {
        let config = BountyConfig::default();
        let hermes_edit = hermes_schema::pb::knowledge::HermesEdit {
            id: vec![0x88; 16],
            name: "bad".into(),
            payload: vec![0xFF, 0xFE, 0xFD], // garbage bytes
            authors: vec![],
            language: None,
            space_id: vec![0xEE; 16],
            is_canonical: true,
            meta: Some(BlockchainMetadata {
                block_number: 100,
                created_at: 1700000000,
                created_by: vec![],
                cursor: String::new(),
                sequence: 0,
                is_last: false,
            }),
        };

        let result = extract_bounty_relations(&hermes_edit, &config);
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("should fail"),
            crate::error::HandlerError::Grc20Decode(_)
        ));
    }

    #[test]
    fn test_extract_bounty_relations_allocated_with_to_space() {
        let config = BountyConfig::default();
        let curator_space = [0xCC; 16];
        let hermes_edit = make_hermes_edit_with_relation(
            *Uuid::parse_str(DEFAULT_ALLOCATED_TYPE_ID)
                .expect("valid")
                .as_bytes(),
            [0x20; 16], // from = bounty
            [0x30; 16], // to = curator entity
            [0x40; 16], // edit space = bounty space
            Some(curator_space),
        );

        let result = extract_bounty_relations(&hermes_edit, &config).expect("should succeed");
        assert_eq!(result.len(), 1);
        let (info, event_type) = &result[0];
        assert_eq!(*event_type, NotificationEventType::BountyAllocated);
        // curator_space_id should come from to_space, NOT be nil
        assert_eq!(info.curator_space_id, Uuid::from_bytes(curator_space));
        assert_ne!(info.curator_space_id, Uuid::nil());
        // curator_entity_id should be rel.to
        assert_eq!(info.curator_entity_id, Uuid::from_bytes([0x30; 16]));
    }

    #[test]
    fn test_extract_bounty_relations_payout_with_to_space() {
        let config = BountyConfig::default();
        let curator_space = [0xDD; 16];
        let hermes_edit = make_hermes_edit_with_relation(
            *Uuid::parse_str(DEFAULT_PAYOUT_TYPE_ID)
                .expect("valid")
                .as_bytes(),
            [0x20; 16],
            [0x30; 16],
            [0x40; 16],
            Some(curator_space),
        );

        let result = extract_bounty_relations(&hermes_edit, &config).expect("should succeed");
        assert_eq!(result.len(), 1);
        let (info, event_type) = &result[0];
        assert_eq!(*event_type, NotificationEventType::BountyPayout);
        assert_eq!(info.curator_space_id, Uuid::from_bytes(curator_space));
    }

    #[test]
    fn test_extract_bounty_relations_allocated_without_to_space() {
        let config = BountyConfig::default();
        let hermes_edit = make_hermes_edit_with_relation(
            *Uuid::parse_str(DEFAULT_ALLOCATED_TYPE_ID)
                .expect("valid")
                .as_bytes(),
            [0x20; 16],
            [0x30; 16],
            [0x40; 16],
            None, // no to_space — needs DB fallback
        );

        let result = extract_bounty_relations(&hermes_edit, &config).expect("should succeed");
        assert_eq!(result.len(), 1);
        let (info, _) = &result[0];
        // curator_space_id should be nil (needs DB lookup)
        assert_eq!(info.curator_space_id, Uuid::nil());
        // curator_entity_id should still be populated for the DB lookup
        assert_eq!(info.curator_entity_id, Uuid::from_bytes([0x30; 16]));
    }

    #[test]
    fn test_extract_bounty_relations_multiple_relations_in_one_edit() {
        use std::borrow::Cow;
        let config = BountyConfig::default();

        // Build an edit with interest + allocated + a non-bounty relation
        let interest_type = *Uuid::parse_str(DEFAULT_INTEREST_TYPE_ID)
            .expect("valid")
            .as_bytes();
        let allocated_type = *Uuid::parse_str(DEFAULT_ALLOCATED_TYPE_ID)
            .expect("valid")
            .as_bytes();
        let random_type = [0xEE; 16]; // non-bounty

        let edit = grc_20::Edit {
            id: [0x99; 16],
            name: Cow::Borrowed("multi-relation edit"),
            authors: vec![[0xAA; 16]],
            created_at: 1700000000,
            ops: vec![
                grc_20::Op::CreateRelation(grc_20::CreateRelation {
                    id: [0x01; 16],
                    relation_type: interest_type,
                    from: [0x10; 16], // curator
                    from_is_value_ref: false,
                    to: [0x20; 16], // bounty
                    to_is_value_ref: false,
                    from_space: None,
                    from_version: None,
                    to_space: None,
                    to_version: None,
                    entity: None,
                    position: None,
                    context: None,
                }),
                grc_20::Op::CreateRelation(grc_20::CreateRelation {
                    id: [0x02; 16],
                    relation_type: allocated_type,
                    from: [0x20; 16], // bounty
                    from_is_value_ref: false,
                    to: [0x10; 16], // curator
                    to_is_value_ref: false,
                    from_space: None,
                    from_version: None,
                    to_space: Some([0xCC; 16]),
                    to_version: None,
                    entity: None,
                    position: None,
                    context: None,
                }),
                grc_20::Op::CreateRelation(grc_20::CreateRelation {
                    id: [0x03; 16],
                    relation_type: random_type, // non-bounty
                    from: [0x30; 16],
                    from_is_value_ref: false,
                    to: [0x40; 16],
                    to_is_value_ref: false,
                    from_space: None,
                    from_version: None,
                    to_space: None,
                    to_version: None,
                    entity: None,
                    position: None,
                    context: None,
                }),
            ],
        };
        let payload = grc_20::encode_edit(&edit).expect("encode should succeed");

        let hermes_edit = hermes_schema::pb::knowledge::HermesEdit {
            id: vec![0x88; 16],
            name: "multi".into(),
            payload,
            authors: vec![vec![0xAA; 16]],
            language: None,
            space_id: vec![0x50; 16],
            is_canonical: true,
            meta: Some(BlockchainMetadata {
                block_number: 100,
                created_at: 1700000000,
                created_by: vec![],
                cursor: String::new(),
                sequence: 0,
                is_last: false,
            }),
        };

        let result = extract_bounty_relations(&hermes_edit, &config).expect("should succeed");
        // Should find 2 bounty relations, not the non-bounty one
        assert_eq!(result.len(), 2);

        let types: Vec<_> = result.iter().map(|(_, t)| t.clone()).collect();
        assert!(types.contains(&NotificationEventType::BountyInterest));
        assert!(types.contains(&NotificationEventType::BountyAllocated));

        // Verify interest has correct direction
        let (interest_info, _) = result
            .iter()
            .find(|(_, t)| *t == NotificationEventType::BountyInterest)
            .expect("interest present");
        assert_eq!(interest_info.bounty_entity_id, Uuid::from_bytes([0x20; 16])); // to = bounty
        assert_eq!(
            interest_info.curator_entity_id,
            Uuid::from_bytes([0x10; 16])
        ); // from = curator

        // Verify allocated has correct direction and to_space
        let (alloc_info, _) = result
            .iter()
            .find(|(_, t)| *t == NotificationEventType::BountyAllocated)
            .expect("allocated present");
        assert_eq!(alloc_info.bounty_entity_id, Uuid::from_bytes([0x20; 16])); // from = bounty
        assert_eq!(alloc_info.curator_entity_id, Uuid::from_bytes([0x10; 16])); // to = curator
        assert_eq!(alloc_info.curator_space_id, Uuid::from_bytes([0xCC; 16])); // to_space
    }

    #[test]
    fn test_extract_bounty_relations_edit_with_only_non_bounty_ops() {
        use std::borrow::Cow;
        let config = BountyConfig::default();

        // CreateEntity op (not a relation at all)
        let edit = grc_20::Edit {
            id: [0x99; 16],
            name: Cow::Borrowed("entity-only edit"),
            authors: vec![],
            created_at: 1700000000,
            ops: vec![grc_20::Op::CreateEntity(grc_20::CreateEntity {
                id: [0x01; 16],
                values: vec![],
                context: None,
            })],
        };
        let payload = grc_20::encode_edit(&edit).expect("encode should succeed");

        let hermes_edit = hermes_schema::pb::knowledge::HermesEdit {
            id: vec![0x88; 16],
            name: "entity".into(),
            payload,
            authors: vec![],
            language: None,
            space_id: vec![0x50; 16],
            is_canonical: true,
            meta: Some(BlockchainMetadata {
                block_number: 100,
                created_at: 1700000000,
                created_by: vec![],
                cursor: String::new(),
                sequence: 0,
                is_last: false,
            }),
        };

        let result = extract_bounty_relations(&hermes_edit, &config).expect("should succeed");
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // governance_proposal_id (drives targeted proposer/voter recipients)
    // -----------------------------------------------------------------------

    #[test]
    fn governance_proposal_id_extracts_from_governance_event() {
        let msg = HermesProposalCreated {
            space_id: make_test_uuid(0x01),
            proposer_id: make_test_uuid(0x02),
            proposal_id: make_test_uuid(0x03),
            voting_mode: 0,
            actions: vec![],
            settings: None,
            meta: Some(make_metadata(1, 1)),
        };
        let event = handle_proposal_created(&msg).expect("should parse");
        let expected = Uuid::from_slice(&make_test_uuid(0x03)).expect("valid uuid");
        assert_eq!(event.governance_proposal_id(), Some(expected));
    }

    #[test]
    fn governance_proposal_id_is_none_for_bounty_event() {
        let info = BountyRelationInfo {
            relation_id: Uuid::nil(),
            bounty_entity_id: Uuid::nil(),
            curator_entity_id: Uuid::nil(),
            curator_space_id: Uuid::nil(),
            bounty_space_id: Uuid::nil(),
            proposal_id: None,
            block_number: 1,
            sequence: 0,
            timestamp: 1,
        };
        let event = handle_bounty_interest(&info);
        assert_eq!(event.governance_proposal_id(), None);
    }
}
