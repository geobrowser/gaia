//! Domain models for notification events.

use hermes_schema::pb::governance::{
    proposal_action, HermesProposalCreated, HermesProposalExecuted, HermesProposalSettingsUpdated,
    HermesProposalUpdated, HermesProposalVoted, ProposalVoteOption,
};
use serde::Serialize;
use uuid::Uuid;

use crate::error::HandlerError;
use crate::ids;

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
    /// A new Bounty entity was created in a space (from `knowledge.edits`).
    BountyCreated,
    // Comment events
    /// A comment was posted on a proposal (from `knowledge.edits`).
    ProposalComment,
    /// A comment or reply in a (non-proposal) thread (from `knowledge.edits`).
    Comment,
    // Vote events
    /// An entity reached a configured upvote threshold (from the vote poller).
    EntityVotesThreshold,
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
            NotificationEventType::BountyCreated => "bounty_created",
            NotificationEventType::ProposalComment => "proposal_comment",
            NotificationEventType::Comment => "comment",
            NotificationEventType::EntityVotesThreshold => "entity_votes_threshold",
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
            | NotificationEventType::BountyPayout
            | NotificationEventType::BountyCreated => "bounty",
            NotificationEventType::ProposalComment | NotificationEventType::Comment => "comment",
            NotificationEventType::EntityVotesThreshold => "votes",
        }
    }

    /// Recipients to notify *in addition to* the event space's editors.
    ///
    /// Editors always receive the base governance event; these targeted
    /// recipients are delivered on top so the colleague's user-centric asks are
    /// covered. Filtering to a precise audience is done app-side, so we resolve
    /// and deliver the relevant superset.
    pub fn targeted_recipients(&self) -> TargetedRecipients {
        match self {
            // "your proposal was voted on / approved / rejected"
            NotificationEventType::ProposalVoted
            | NotificationEventType::ProposalExecuted
            | NotificationEventType::ProposalRejected => TargetedRecipients::Proposer,
            // "a new version of a proposal you voted on was submitted"
            NotificationEventType::ProposalUpdated => TargetedRecipients::Voters,
            // proposal_created / settings_updated and all bounty events: editors
            // (or the bounty's own single recipient) only.
            _ => TargetedRecipients::None,
        }
    }
}

/// Recipients to resolve *in addition to* a space's editors for a governance
/// event. The variant determines which DB resolver the consumer calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetedRecipients {
    /// The proposer (`proposals.proposed_by`).
    Proposer,
    /// Prior voters of the proposal (`proposal_votes.voter_id`).
    Voters,
    /// Editors only — no additional targeted recipients.
    None,
}

/// Merge targeted recipients into the editor set, returning a sorted,
/// de-duplicated recipient list.
///
/// Pure so the fan-out audience is unit-testable without a database. A user who
/// is both an editor and the proposer/a voter appears exactly once; the storage
/// layer's `ON CONFLICT (idempotency_key) DO NOTHING` is a second line of
/// defense against duplicates.
pub fn merge_recipients(mut editors: Vec<Uuid>, extra: Vec<Uuid>) -> Vec<Uuid> {
    editors.extend(extra);
    editors.sort();
    editors.dedup();
    editors
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
    BountyCreated(BountyCreatedData),
    Comment(CommentData),
    GeneralComment(GeneralCommentData),
    VoteThreshold(VoteThresholdData),
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
    /// Display name of the target member/editor, resolved from `target_address`
    /// — which carries the target's personal-space UUID (hermes' decode of
    /// `addMember(bytes16)`), so it resolves like any space name. Best-effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
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

/// Payload fields for a newly-created bounty (`bounty_created`).
#[derive(Debug, Clone, Serialize)]
pub struct BountyCreatedData {
    /// The new bounty entity.
    pub bounty_entity_id: String,
    /// The space the bounty was created in (recipients are its editors).
    pub bounty_space_id: String,
    /// Human-readable bounty name (best-effort, from KG values table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounty_name: Option<String>,
}

/// Payload fields for a comment on a proposal (`proposal_comment`).
#[derive(Debug, Clone, Serialize)]
pub struct CommentData {
    /// The comment entity that was created.
    pub comment_entity_id: String,
    /// The proposal the comment replies to.
    pub proposal_id: String,
    /// The commenter's personal space (the `HermesEdit.space_id` the comment was
    /// published from).
    pub commenter_space_id: String,
    /// Human-readable proposal name (best-effort, from proposals table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_name: Option<String>,
}

/// Payload fields for a comment/reply in a non-proposal thread (`comment`).
#[derive(Debug, Clone, Serialize)]
pub struct GeneralCommentData {
    /// The comment entity that was created.
    pub comment_entity_id: String,
    /// The entity the comment directly replies to (a comment, for a reply, or the
    /// commented-on entity for a top-level comment).
    pub parent_id: String,
    /// The thread root — the entity the whole thread hangs off of.
    pub root_id: String,
    /// The commenter's personal space (the `HermesEdit.space_id`).
    pub commenter_space_id: String,
    /// Human-readable name of the thread root (best-effort).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_name: Option<String>,
}

/// Payload fields for an entity reaching an upvote threshold (`entity_votes_threshold`).
#[derive(Debug, Clone, Serialize)]
pub struct VoteThresholdData {
    /// The entity that reached the threshold (the recipient is its creator).
    pub entity_id: String,
    /// The space the votes were counted in (also the payload `space_id`).
    pub vote_space_id: String,
    /// Current upvote total for the entity in that space.
    pub upvotes: i64,
    /// Current downvote total for the entity in that space.
    pub downvotes: i64,
    /// The configured threshold that was reached.
    pub threshold: i64,
    /// Human-readable entity name (best-effort, from KG values table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_name: Option<String>,
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
            NotificationData::Bounty(_)
            | NotificationData::BountyCreated(_)
            | NotificationData::Comment(_)
            | NotificationData::GeneralComment(_)
            | NotificationData::VoteThreshold(_) => None,
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

/// Build a notification event for an entity that reached an upvote threshold.
///
/// Synthesized by the vote poller (not from Kafka). The idempotency key includes
/// the entity, the vote space, and the threshold, so each entity fires at most
/// once per space per threshold value — raising the threshold later (a new env
/// value) re-arms it at the new level.
pub fn build_vote_threshold_event(
    entity_id: Uuid,
    vote_space_id: Uuid,
    upvotes: i64,
    downvotes: i64,
    threshold: i64,
) -> NotificationEvent {
    let idempotency_base = format!(
        "{}:{}:entity_votes_threshold:{}",
        entity_id, vote_space_id, threshold
    );

    NotificationEvent {
        event_type: NotificationEventType::EntityVotesThreshold,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::EntityVotesThreshold
                .as_str()
                .to_string(),
            category: NotificationEventType::EntityVotesThreshold
                .category()
                .to_string(),
            space_id: vote_space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: None,
            timestamp: None,
            space_name: None,
            data: NotificationData::VoteThreshold(VoteThresholdData {
                entity_id: entity_id.to_string(),
                vote_space_id: vote_space_id.to_string(),
                upvotes,
                downvotes,
                threshold,
                entity_name: None,
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
    // Include relation_id: a single edit can carry multiple interest relations,
    // all sharing (block, sequence). Without the relation id they'd hash to the
    // same per-user key and all but one would be silently dropped on insert.
    let idempotency_base = format!(
        "{}:{}:bounty_interest:{}",
        info.block_number, info.sequence, info.relation_id
    );

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
    // Include relation_id — a single edit can carry multiple allocation
    // relations sharing (block, sequence); see handle_bounty_interest.
    let idempotency_base = format!(
        "{}:{}:bounty_allocated:{}",
        info.block_number, info.sequence, info.relation_id
    );

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
    // Include relation_id — a single edit can carry multiple payout relations
    // sharing (block, sequence); see handle_bounty_interest.
    let idempotency_base = format!(
        "{}:{}:bounty_payout:{}",
        info.block_number, info.sequence, info.relation_id
    );

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

// ---------------------------------------------------------------------------
// Bounty created (Phase 3a) — a new Bounty entity created in a space
// ---------------------------------------------------------------------------

/// A new bounty detected in a `HermesEdit` (entity → Types → Bounty).
#[derive(Debug, Clone)]
pub struct BountyCreatedInfo {
    pub bounty_entity_id: Uuid,
    /// The space the bounty was created in (the edit's space). Recipients are
    /// this space's editors.
    pub space_id: Uuid,
    pub block_number: u64,
    pub sequence: u64,
    pub timestamp: u64,
}

/// Build a notification event for a newly-created bounty.
pub fn handle_bounty_created(info: &BountyCreatedInfo) -> NotificationEvent {
    // Include bounty_entity_id: extract_bounty_created can return several new
    // bounties from one edit, all sharing (block, sequence). The entity id makes
    // each logical event unique so none are dropped as false duplicates.
    let idempotency_base = format!(
        "{}:{}:bounty_created:{}",
        info.block_number, info.sequence, info.bounty_entity_id
    );

    NotificationEvent {
        event_type: NotificationEventType::BountyCreated,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::BountyCreated.as_str().to_string(),
            category: NotificationEventType::BountyCreated.category().to_string(),
            space_id: info.space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(info.block_number),
            timestamp: Some(info.timestamp),
            space_name: None,
            data: NotificationData::BountyCreated(BountyCreatedData {
                bounty_entity_id: info.bounty_entity_id.to_string(),
                bounty_space_id: info.space_id.to_string(),
                bounty_name: None,
            }),
        },
    }
}

/// Extract newly-created bounties from a `HermesEdit`.
///
/// Matches `CreateRelation` ops that type an entity as a Bounty
/// (`relation_type == Types`, `to == Bounty type`); the relation's `from` is the
/// new bounty entity.
pub fn extract_bounty_created(
    edit: &hermes_schema::pb::knowledge::HermesEdit,
) -> Result<Vec<BountyCreatedInfo>, crate::error::HandlerError> {
    use crate::error::HandlerError;

    let meta = edit.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = u64::from(meta.sequence);
    let timestamp = meta.created_at;
    let edit_space_id = Uuid::from_slice(&edit.space_id).map_err(HandlerError::Uuid)?;

    let decoded = grc_20::decode_edit(&edit.payload)
        .map_err(|e| HandlerError::Grc20Decode(format!("{}", e)))?;

    let types_rel = ids::types_relation_type();
    let bounty_type = ids::bounty_type();

    let mut results = Vec::new();
    for op in &decoded.ops {
        if let grc_20::Op::CreateRelation(rel) = op {
            if Uuid::from_bytes(rel.relation_type) == types_rel
                && Uuid::from_bytes(rel.to) == bounty_type
            {
                results.push(BountyCreatedInfo {
                    bounty_entity_id: Uuid::from_bytes(rel.from),
                    space_id: edit_space_id,
                    block_number,
                    sequence,
                    timestamp,
                });
            }
        }
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Proposal comments (Phase 2a) — a comment posted on a proposal
// ---------------------------------------------------------------------------

/// A comment (Comment entity that replies to a parent) detected in a
/// `HermesEdit`. The parent is a *candidate* proposal — the consumer confirms it
/// is a proposal (and resolves the recipient/space) via the DB.
#[derive(Debug, Clone)]
pub struct ProposalCommentInfo {
    pub comment_entity_id: Uuid,
    /// The entity the comment replies to (candidate proposal id).
    pub proposal_id: Uuid,
    /// The commenter's personal space (the edit's space).
    pub commenter_space_id: Uuid,
    /// The proposal's owning space — `nil` until resolved by the consumer
    /// (`find_proposal_proposer_and_space`).
    pub proposal_space_id: Uuid,
    pub block_number: u64,
    pub sequence: u64,
    pub timestamp: u64,
}

/// Build a notification event for a comment on a proposal.
pub fn handle_proposal_comment(info: &ProposalCommentInfo) -> NotificationEvent {
    // Include comment_entity_id: extract_proposal_comments can return multiple
    // comments from one edit, all sharing (block, sequence). The comment entity
    // id makes each logical event unique so none are dropped as false duplicates.
    let idempotency_base = format!(
        "{}:{}:proposal_comment:{}",
        info.block_number, info.sequence, info.comment_entity_id
    );

    NotificationEvent {
        event_type: NotificationEventType::ProposalComment,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::ProposalComment.as_str().to_string(),
            category: NotificationEventType::ProposalComment
                .category()
                .to_string(),
            space_id: info.proposal_space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(info.block_number),
            timestamp: Some(info.timestamp),
            space_name: None,
            data: NotificationData::Comment(CommentData {
                comment_entity_id: info.comment_entity_id.to_string(),
                proposal_id: info.proposal_id.to_string(),
                commenter_space_id: info.commenter_space_id.to_string(),
                proposal_name: None,
            }),
        },
    }
}

/// Extract proposal comments from a `HermesEdit`.
///
/// A comment is a `Comment`-typed entity (`Types → Comment`) with a `Reply to`
/// relation pointing at its parent. This returns one [`ProposalCommentInfo`] per
/// such reply; the consumer then checks whether the parent is actually a
/// proposal (general comments on non-proposal entities are a later phase).
pub fn extract_proposal_comments(
    edit: &hermes_schema::pb::knowledge::HermesEdit,
) -> Result<Vec<ProposalCommentInfo>, crate::error::HandlerError> {
    use crate::error::HandlerError;
    use std::collections::HashSet;

    let meta = edit.meta.as_ref().ok_or(HandlerError::MissingMetadata)?;
    let block_number = meta.block_number;
    let sequence = u64::from(meta.sequence);
    let timestamp = meta.created_at;
    let edit_space_id = Uuid::from_slice(&edit.space_id).map_err(HandlerError::Uuid)?;

    let decoded = grc_20::decode_edit(&edit.payload)
        .map_err(|e| HandlerError::Grc20Decode(format!("{}", e)))?;

    let types_rel = ids::types_relation_type();
    let comment_type = ids::comment_type();
    let reply_to = ids::reply_to_property();

    // Pass 1: entities typed as Comment in this edit (from → Types → Comment).
    let mut comment_entities: HashSet<[u8; 16]> = HashSet::new();
    for op in &decoded.ops {
        if let grc_20::Op::CreateRelation(rel) = op {
            if Uuid::from_bytes(rel.relation_type) == types_rel
                && Uuid::from_bytes(rel.to) == comment_type
            {
                comment_entities.insert(rel.from);
            }
        }
    }

    // Pass 2: Reply-to relations originating from a Comment entity → its parent.
    let mut results = Vec::new();
    for op in &decoded.ops {
        if let grc_20::Op::CreateRelation(rel) = op {
            if Uuid::from_bytes(rel.relation_type) == reply_to
                && comment_entities.contains(&rel.from)
            {
                results.push(ProposalCommentInfo {
                    comment_entity_id: Uuid::from_bytes(rel.from),
                    proposal_id: Uuid::from_bytes(rel.to),
                    commenter_space_id: edit_space_id,
                    proposal_space_id: Uuid::nil(), // resolved via DB in the consumer
                    block_number,
                    sequence,
                    timestamp,
                });
            }
        }
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// General comment threads (Phase 2b) — comments/replies not directly on a proposal
// ---------------------------------------------------------------------------

/// A comment in a (non-proposal) thread, with the thread context resolved by the
/// consumer. Recipients are the thread participants plus the root's creator.
#[derive(Debug, Clone)]
pub struct CommentThreadInfo {
    pub comment_entity_id: Uuid,
    /// The entity the comment directly replies to.
    pub parent_id: Uuid,
    /// The thread root (the "thing being commented on"), resolved by walking
    /// `Reply to` up from `parent_id`.
    pub root_id: Uuid,
    /// The commenter's personal space (the edit's space).
    pub commenter_space_id: Uuid,
    /// The root's home/owning space — used as the payload `space_id` and for
    /// name enrichment. Resolved by the consumer.
    pub root_space_id: Uuid,
    pub block_number: u64,
    pub sequence: u64,
    pub timestamp: u64,
}

/// Build a notification event for a comment/reply in a thread.
pub fn handle_comment(info: &CommentThreadInfo) -> NotificationEvent {
    // Include comment_entity_id: a single edit can carry multiple thread
    // comments sharing (block, sequence). The comment entity id (globally unique)
    // makes each logical event unique so none are dropped as false duplicates.
    let idempotency_base = format!(
        "{}:{}:comment:{}",
        info.block_number, info.sequence, info.comment_entity_id
    );

    NotificationEvent {
        event_type: NotificationEventType::Comment,
        idempotency_key: idempotency_base,
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: NotificationEventType::Comment.as_str().to_string(),
            category: NotificationEventType::Comment.category().to_string(),
            space_id: info.root_space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: Some(info.block_number),
            timestamp: Some(info.timestamp),
            space_name: None,
            data: NotificationData::GeneralComment(GeneralCommentData {
                comment_entity_id: info.comment_entity_id.to_string(),
                parent_id: info.parent_id.to_string(),
                root_id: info.root_id.to_string(),
                commenter_space_id: info.commenter_space_id.to_string(),
                root_name: None,
            }),
        },
    }
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
        assert_eq!(
            event.idempotency_key,
            format!("50000:7:bounty_interest:{}", info.relation_id)
        );

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
        assert_eq!(
            event.idempotency_key,
            format!("50000:7:bounty_allocated:{}", info.relation_id)
        );

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
        assert_eq!(
            event.idempotency_key,
            format!("50000:7:bounty_payout:{}", info.relation_id)
        );

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
        assert_eq!(
            event.idempotency_key,
            format!("50000:7:bounty_interest:{}", info.relation_id)
        );

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
        assert_eq!(
            event.idempotency_key,
            format!("50000:7:bounty_allocated:{}", info.relation_id)
        );

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
        assert_eq!(
            event.idempotency_key,
            format!("50000:7:bounty_payout:{}", info.relation_id)
        );

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
        let info1 = make_bounty_info();
        let mut info2 = make_bounty_info();
        info2.block_number = 50001;

        let event1 = handle_bounty_interest(&info1);
        let event2 = handle_bounty_interest(&info2);
        assert_ne!(event1.idempotency_key, event2.idempotency_key);
    }

    #[test]
    fn test_bounty_idempotency_keys_differ_by_sequence() {
        let info1 = make_bounty_info();
        let mut info2 = make_bounty_info();
        info2.sequence = 8;

        let event1 = handle_bounty_interest(&info1);
        let event2 = handle_bounty_interest(&info2);
        assert_ne!(event1.idempotency_key, event2.idempotency_key);
    }

    // -----------------------------------------------------------------------
    // Same-edit uniqueness: multiple logical events of the same type from one
    // HermesEdit share (block, sequence) but must NOT collide on the idempotency
    // key, or all-but-one would be silently dropped by the outbox ON CONFLICT.
    // -----------------------------------------------------------------------

    #[test]
    fn test_bounty_relations_same_edit_differ_by_relation_id() {
        // Two interest relations in one edit: same block/sequence, different
        // relation_id (and even the same bounty/curator) must yield distinct keys.
        let mut a = make_bounty_info();
        let mut b = make_bounty_info();
        a.relation_id = Uuid::from_bytes([0xA1; 16]);
        b.relation_id = Uuid::from_bytes([0xB2; 16]);

        for build in [
            handle_bounty_interest,
            handle_bounty_allocated,
            handle_bounty_payout,
        ] {
            let ka = build(&a).idempotency_key;
            let kb = build(&b).idempotency_key;
            assert_ne!(
                ka, kb,
                "same-edit relations must not share an idempotency key"
            );
            assert!(ka.contains(&a.relation_id.to_string()));
        }
    }

    #[test]
    fn test_bounty_created_same_edit_differ_by_entity() {
        // Two bounties created in one edit must not collide.
        let base = BountyCreatedInfo {
            bounty_entity_id: Uuid::from_bytes([0x01; 16]),
            space_id: Uuid::from_bytes([0x5E; 16]),
            block_number: 100,
            sequence: 0,
            timestamp: 1700000000,
        };
        let other = BountyCreatedInfo {
            bounty_entity_id: Uuid::from_bytes([0x02; 16]),
            ..base.clone()
        };
        let k1 = handle_bounty_created(&base).idempotency_key;
        let k2 = handle_bounty_created(&other).idempotency_key;
        assert_ne!(k1, k2);
        assert_eq!(
            k1,
            format!("100:0:bounty_created:{}", base.bounty_entity_id)
        );
    }

    #[test]
    fn test_proposal_comment_same_edit_differ_by_comment() {
        // Two proposal comments in one edit must not collide.
        let base = ProposalCommentInfo {
            comment_entity_id: Uuid::from_bytes([0xC1; 16]),
            proposal_id: Uuid::from_bytes([0x9A; 16]),
            commenter_space_id: Uuid::from_bytes([0x11; 16]),
            proposal_space_id: Uuid::from_bytes([0x5E; 16]),
            block_number: 100,
            sequence: 2,
            timestamp: 1700000000,
        };
        let other = ProposalCommentInfo {
            comment_entity_id: Uuid::from_bytes([0xC2; 16]),
            ..base.clone()
        };
        let k1 = handle_proposal_comment(&base).idempotency_key;
        let k2 = handle_proposal_comment(&other).idempotency_key;
        assert_ne!(k1, k2);
        assert_eq!(
            k1,
            format!("100:2:proposal_comment:{}", base.comment_entity_id)
        );
    }

    #[test]
    fn test_comment_thread_same_edit_differ_by_comment() {
        // Two thread comments in one edit must not collide.
        let base = CommentThreadInfo {
            comment_entity_id: Uuid::from_bytes([0xC1; 16]),
            parent_id: Uuid::from_bytes([0x91; 16]),
            root_id: Uuid::from_bytes([0x9A; 16]),
            commenter_space_id: Uuid::from_bytes([0x11; 16]),
            root_space_id: Uuid::from_bytes([0x5E; 16]),
            block_number: 100,
            sequence: 4,
            timestamp: 1700000000,
        };
        let other = CommentThreadInfo {
            comment_entity_id: Uuid::from_bytes([0xC2; 16]),
            ..base.clone()
        };
        let k1 = handle_comment(&base).idempotency_key;
        let k2 = handle_comment(&other).idempotency_key;
        assert_ne!(k1, k2);
        assert_eq!(k1, format!("100:4:comment:{}", base.comment_entity_id));
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

    // -----------------------------------------------------------------------
    // targeted_recipients + merge_recipients (recipient routing & dedup)
    // -----------------------------------------------------------------------

    #[test]
    fn targeted_recipients_routes_by_event_type() {
        use NotificationEventType::*;
        // "your proposal was voted on / approved / rejected" -> proposer
        assert_eq!(
            ProposalVoted.targeted_recipients(),
            TargetedRecipients::Proposer
        );
        assert_eq!(
            ProposalExecuted.targeted_recipients(),
            TargetedRecipients::Proposer
        );
        assert_eq!(
            ProposalRejected.targeted_recipients(),
            TargetedRecipients::Proposer
        );
        // "a new version of a proposal you voted on" -> prior voters
        assert_eq!(
            ProposalUpdated.targeted_recipients(),
            TargetedRecipients::Voters
        );
        // editors-only (or single-recipient bounty) events get no targeted extras
        assert_eq!(
            ProposalCreated.targeted_recipients(),
            TargetedRecipients::None
        );
        assert_eq!(
            ProposalSettingsUpdated.targeted_recipients(),
            TargetedRecipients::None
        );
        assert_eq!(
            BountyInterest.targeted_recipients(),
            TargetedRecipients::None
        );
        assert_eq!(
            BountyAllocated.targeted_recipients(),
            TargetedRecipients::None
        );
    }

    #[test]
    fn merge_recipients_dedups_and_sorts() {
        let a = Uuid::from_bytes([0x01; 16]);
        let b = Uuid::from_bytes([0x02; 16]);
        let c = Uuid::from_bytes([0x03; 16]);
        // editors = [b, a]; extra = proposer b (already an editor) + new voter c.
        let merged = merge_recipients(vec![b, a], vec![b, c]);
        // sorted, and b (editor ∩ targeted) appears exactly once.
        assert_eq!(merged, vec![a, b, c]);
    }

    #[test]
    fn merge_recipients_dedups_editor_duplicates_with_empty_extra() {
        let a = Uuid::from_bytes([0x01; 16]);
        let merged = merge_recipients(vec![a, a], vec![]);
        assert_eq!(merged, vec![a]);
    }

    // -----------------------------------------------------------------------
    // Phase 3a / 2a: bounty-created + proposal-comment extraction
    // -----------------------------------------------------------------------

    /// Build a HermesEdit whose GRC-20 payload contains the given relations,
    /// each as `(relation_type, from, to)`.
    fn make_hermes_edit_with_relations(
        relations: &[([u8; 16], [u8; 16], [u8; 16])],
        space_id: [u8; 16],
    ) -> hermes_schema::pb::knowledge::HermesEdit {
        use std::borrow::Cow;

        let ops = relations
            .iter()
            .enumerate()
            .map(|(i, (relation_type, from, to))| {
                grc_20::Op::CreateRelation(grc_20::CreateRelation {
                    id: [i as u8 + 1; 16],
                    relation_type: *relation_type,
                    from: *from,
                    from_is_value_ref: false,
                    to: *to,
                    to_is_value_ref: false,
                    from_space: None,
                    from_version: None,
                    to_space: None,
                    to_version: None,
                    entity: None,
                    position: None,
                    context: None,
                })
            })
            .collect();

        let edit = grc_20::Edit {
            id: [0x99; 16],
            name: Cow::Borrowed("test edit"),
            authors: vec![[0xAA; 16]],
            created_at: 1700000000,
            ops,
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
            meta: Some(make_metadata(12345, 1700000000)),
        }
    }

    #[test]
    fn extract_bounty_created_matches_types_to_bounty() {
        let types = ids::types_relation_type().into_bytes();
        let bounty_type = ids::bounty_type().into_bytes();
        // from = the new bounty entity, to = Bounty type.
        let edit = make_hermes_edit_with_relations(
            &[(types, [0xB0; 16], bounty_type)],
            [0x5E; 16], // edit space
        );
        let out = extract_bounty_created(&edit).expect("extract");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bounty_entity_id, Uuid::from_bytes([0xB0; 16]));
        assert_eq!(out[0].space_id, Uuid::from_bytes([0x5E; 16]));
        assert_eq!(out[0].block_number, 12345);
    }

    #[test]
    fn extract_bounty_created_ignores_other_types_and_relations() {
        let types = ids::types_relation_type().into_bytes();
        // A Types relation to a *non-bounty* type, plus an unrelated relation.
        let edit = make_hermes_edit_with_relations(
            &[
                (types, [0xB0; 16], [0xAA; 16]),      // Types -> some other type
                ([0xCC; 16], [0xB0; 16], [0xDD; 16]), // non-Types relation
            ],
            [0x5E; 16],
        );
        let out = extract_bounty_created(&edit).expect("extract");
        assert!(out.is_empty());
    }

    #[test]
    fn extract_proposal_comments_matches_comment_reply() {
        let types = ids::types_relation_type().into_bytes();
        let comment_type = ids::comment_type().into_bytes();
        let reply_to = ids::reply_to_property().into_bytes();
        let comment_entity = [0xC0; 16];
        let proposal = [0x9A; 16];
        // Comment entity typed as Comment, replying to the proposal.
        let edit = make_hermes_edit_with_relations(
            &[
                (types, comment_entity, comment_type),
                (reply_to, comment_entity, proposal),
            ],
            [0x5E; 16], // commenter's personal space
        );
        let out = extract_proposal_comments(&edit).expect("extract");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].comment_entity_id, Uuid::from_bytes(comment_entity));
        assert_eq!(out[0].proposal_id, Uuid::from_bytes(proposal));
        assert_eq!(out[0].commenter_space_id, Uuid::from_bytes([0x5E; 16]));
        assert_eq!(out[0].proposal_space_id, Uuid::nil()); // resolved later via DB
    }

    #[test]
    fn extract_proposal_comments_ignores_reply_from_non_comment() {
        let reply_to = ids::reply_to_property().into_bytes();
        // A Reply-to relation whose `from` is NOT typed as a Comment in this edit.
        let edit =
            make_hermes_edit_with_relations(&[(reply_to, [0xC0; 16], [0x9A; 16])], [0x5E; 16]);
        let out = extract_proposal_comments(&edit).expect("extract");
        assert!(out.is_empty());
    }

    #[test]
    fn handle_bounty_created_payload_structure() {
        let info = BountyCreatedInfo {
            bounty_entity_id: Uuid::from_bytes([0xB0; 16]),
            space_id: Uuid::from_bytes([0x5E; 16]),
            block_number: 100,
            sequence: 0,
            timestamp: 1700000000,
        };
        let event = handle_bounty_created(&info);
        assert_eq!(event.event_type, NotificationEventType::BountyCreated);
        let json = serde_json::to_value(&event.payload).expect("serialize");
        assert_eq!(json["event_type"], "bounty_created");
        assert_eq!(json["category"], "bounty");
        assert_eq!(
            json["bounty_entity_id"],
            Uuid::from_bytes([0xB0; 16]).to_string()
        );
        assert_eq!(json["space_id"], Uuid::from_bytes([0x5E; 16]).to_string());
        // governance/comment fields absent
        assert!(json.get("proposal_id").is_none());
        assert!(json.get("comment_entity_id").is_none());
    }

    #[test]
    fn handle_proposal_comment_payload_structure() {
        let info = ProposalCommentInfo {
            comment_entity_id: Uuid::from_bytes([0xC0; 16]),
            proposal_id: Uuid::from_bytes([0x9A; 16]),
            commenter_space_id: Uuid::from_bytes([0x11; 16]),
            proposal_space_id: Uuid::from_bytes([0x5E; 16]),
            block_number: 100,
            sequence: 2,
            timestamp: 1700000000,
        };
        let event = handle_proposal_comment(&info);
        assert_eq!(event.event_type, NotificationEventType::ProposalComment);
        let json = serde_json::to_value(&event.payload).expect("serialize");
        assert_eq!(json["event_type"], "proposal_comment");
        assert_eq!(json["category"], "comment");
        assert_eq!(json["space_id"], Uuid::from_bytes([0x5E; 16]).to_string()); // proposal's space
        assert_eq!(
            json["proposal_id"],
            Uuid::from_bytes([0x9A; 16]).to_string()
        );
        assert_eq!(
            json["comment_entity_id"],
            Uuid::from_bytes([0xC0; 16]).to_string()
        );
        assert_eq!(
            json["commenter_space_id"],
            Uuid::from_bytes([0x11; 16]).to_string()
        );
    }

    #[test]
    fn handle_comment_payload_structure() {
        let info = CommentThreadInfo {
            comment_entity_id: Uuid::from_bytes([0xC0; 16]),
            parent_id: Uuid::from_bytes([0x91; 16]), // parent (a comment or entity)
            root_id: Uuid::from_bytes([0x9A; 16]),
            commenter_space_id: Uuid::from_bytes([0x11; 16]),
            root_space_id: Uuid::from_bytes([0x5E; 16]),
            block_number: 100,
            sequence: 4,
            timestamp: 1700000000,
        };
        let event = handle_comment(&info);
        assert_eq!(event.event_type, NotificationEventType::Comment);
        let json = serde_json::to_value(&event.payload).expect("serialize");
        assert_eq!(json["event_type"], "comment");
        assert_eq!(json["category"], "comment");
        assert_eq!(json["space_id"], Uuid::from_bytes([0x5E; 16]).to_string()); // root's space
        assert_eq!(json["root_id"], Uuid::from_bytes([0x9A; 16]).to_string());
        assert_eq!(
            json["comment_entity_id"],
            Uuid::from_bytes([0xC0; 16]).to_string()
        );
        assert_eq!(
            json["commenter_space_id"],
            Uuid::from_bytes([0x11; 16]).to_string()
        );
        // not a governance/proposal payload
        assert!(json.get("proposal_id").is_none());
    }

    // -----------------------------------------------------------------------
    // Adversarial: try to BREAK idempotency by packing multiple same-type
    // events into one HermesEdit payload (they share block+sequence). These go
    // through the real decode -> extract -> handle path, not hand-built Info
    // structs, so they guard the fix against regressions at the seam where the
    // old {block}:{seq}:{event_type} key collided.
    // -----------------------------------------------------------------------

    /// Wrap a set of GRC-20 ops into a HermesEdit at a fixed block/sequence.
    fn make_edit_with_ops(
        ops: Vec<grc_20::Op<'static>>,
        space_id: [u8; 16],
        block_number: u64,
        sequence: u32,
    ) -> hermes_schema::pb::knowledge::HermesEdit {
        use std::borrow::Cow;
        let edit = grc_20::Edit {
            id: [0x77; 16],
            name: Cow::Borrowed("adversarial edit"),
            authors: vec![[0xAA; 16]],
            created_at: 1700000000,
            ops,
        };
        let payload = grc_20::encode_edit(&edit).expect("encode should succeed");
        hermes_schema::pb::knowledge::HermesEdit {
            id: vec![0x77; 16],
            name: "adversarial".into(),
            payload,
            authors: vec![vec![0xAA; 16]],
            language: None,
            space_id: space_id.to_vec(),
            is_canonical: true,
            meta: Some(BlockchainMetadata {
                block_number,
                created_at: 1700000000,
                created_by: vec![],
                cursor: String::new(),
                sequence,
                is_last: false,
            }),
        }
    }

    fn create_relation(
        id: [u8; 16],
        relation_type: [u8; 16],
        from: [u8; 16],
        to: [u8; 16],
    ) -> grc_20::Op<'static> {
        grc_20::Op::CreateRelation(grc_20::CreateRelation {
            id,
            relation_type,
            from,
            from_is_value_ref: false,
            to,
            to_is_value_ref: false,
            from_space: None,
            from_version: None,
            to_space: None,
            to_version: None,
            entity: None,
            position: None,
            context: None,
        })
    }

    #[test]
    fn idempotency_break_many_bounty_interest_in_one_edit() {
        // Three interest relations in ONE edit for the SAME bounty by the SAME
        // curator — the worst case: everything identical except relation_id.
        let config = BountyConfig::default();
        let interest = *Uuid::parse_str(DEFAULT_INTEREST_TYPE_ID)
            .expect("valid")
            .as_bytes();
        let bounty = [0x20; 16];
        let curator = [0x10; 16];
        let ops = vec![
            create_relation([0x01; 16], interest, curator, bounty),
            create_relation([0x02; 16], interest, curator, bounty),
            create_relation([0x03; 16], interest, curator, bounty),
        ];
        let edit = make_edit_with_ops(ops, [0x40; 16], 100, 0);

        let relations = extract_bounty_relations(&edit, &config).expect("extract");
        assert_eq!(relations.len(), 3, "all three interest relations extracted");

        let keys: std::collections::HashSet<String> = relations
            .iter()
            .map(|(info, _)| handle_bounty_interest(info).idempotency_key)
            .collect();
        assert_eq!(
            keys.len(),
            3,
            "three same-edit interest relations must yield three distinct keys"
        );
        // All share the old prefix — proving relation_id is what disambiguates.
        assert!(keys.iter().all(|k| k.starts_with("100:0:bounty_interest:")));
    }

    #[test]
    fn idempotency_break_many_bounties_created_in_one_edit() {
        let types = crate::ids::types_relation_type().into_bytes();
        let bounty_type = crate::ids::bounty_type().into_bytes();
        // Four new bounties typed in one edit.
        let ops: Vec<grc_20::Op> = (0u8..4)
            .map(|i| create_relation([0xF0 + i; 16], types, [0xB0 + i; 16], bounty_type))
            .collect();
        let edit = make_edit_with_ops(ops, [0x40; 16], 200, 1);

        let created = extract_bounty_created(&edit).expect("extract");
        assert_eq!(created.len(), 4);

        let keys: std::collections::HashSet<String> = created
            .iter()
            .map(|info| handle_bounty_created(info).idempotency_key)
            .collect();
        assert_eq!(
            keys.len(),
            4,
            "four bounties created in one edit must yield four distinct keys"
        );
        assert!(keys.iter().all(|k| k.starts_with("200:1:bounty_created:")));
    }

    #[test]
    fn idempotency_break_many_comments_in_one_edit() {
        let types = crate::ids::types_relation_type().into_bytes();
        let comment_type = crate::ids::comment_type().into_bytes();
        let reply_to = crate::ids::reply_to_property().into_bytes();
        let parent = [0x9A; 16];
        // Two distinct comment entities, each typed as Comment and replying to
        // the same parent, in one edit.
        let c1 = [0xC1; 16];
        let c2 = [0xC2; 16];
        let ops = vec![
            create_relation([0x01; 16], types, c1, comment_type),
            create_relation([0x02; 16], reply_to, c1, parent),
            create_relation([0x03; 16], types, c2, comment_type),
            create_relation([0x04; 16], reply_to, c2, parent),
        ];
        let edit = make_edit_with_ops(ops, [0x50; 16], 300, 2);

        let comments = extract_proposal_comments(&edit).expect("extract");
        assert_eq!(comments.len(), 2, "both comments extracted");

        // proposal_comment path
        let pc_keys: std::collections::HashSet<String> = comments
            .iter()
            .map(|info| handle_proposal_comment(info).idempotency_key)
            .collect();
        assert_eq!(
            pc_keys.len(),
            2,
            "two comments -> two proposal_comment keys"
        );
        assert!(pc_keys
            .iter()
            .all(|k| k.starts_with("300:2:proposal_comment:")));

        // general comment-thread path (Phase 2b / #706) — same edit, must also
        // produce distinct keys.
        let thread_keys: std::collections::HashSet<String> = comments
            .iter()
            .map(|info| {
                let cinfo = CommentThreadInfo {
                    comment_entity_id: info.comment_entity_id,
                    parent_id: info.proposal_id,
                    root_id: Uuid::from_bytes(parent),
                    commenter_space_id: info.commenter_space_id,
                    root_space_id: Uuid::from_bytes([0x5E; 16]),
                    block_number: info.block_number,
                    sequence: info.sequence,
                    timestamp: info.timestamp,
                };
                handle_comment(&cinfo).idempotency_key
            })
            .collect();
        assert_eq!(thread_keys.len(), 2, "two comments -> two comment keys");
        assert!(thread_keys.iter().all(|k| k.starts_with("300:2:comment:")));
    }

    // -----------------------------------------------------------------------
    // Entity vote-threshold (vote poller)
    // -----------------------------------------------------------------------

    #[test]
    fn entity_votes_threshold_type_strings() {
        assert_eq!(
            NotificationEventType::EntityVotesThreshold.as_str(),
            "entity_votes_threshold"
        );
        assert_eq!(
            NotificationEventType::EntityVotesThreshold.category(),
            "votes"
        );
    }

    #[test]
    fn build_vote_threshold_event_payload_and_key() {
        let entity = Uuid::from_bytes([0xE1; 16]);
        let space = Uuid::from_bytes([0x5E; 16]);
        let event = build_vote_threshold_event(entity, space, 12, 3, 10);

        assert_eq!(
            event.event_type,
            NotificationEventType::EntityVotesThreshold
        );
        assert_eq!(
            event.idempotency_key,
            format!("{}:{}:entity_votes_threshold:10", entity, space)
        );

        let json = serde_json::to_value(&event.payload).expect("serialize");
        assert_eq!(json["event_type"], "entity_votes_threshold");
        assert_eq!(json["category"], "votes");
        assert_eq!(json["space_id"], space.to_string()); // vote space
        assert_eq!(json["entity_id"], entity.to_string());
        assert_eq!(json["vote_space_id"], space.to_string());
        assert_eq!(json["upvotes"], 12);
        assert_eq!(json["downvotes"], 3);
        assert_eq!(json["threshold"], 10);
        // user_space_id stamped later by storage during per-user fan-out
        assert!(json.get("user_space_id").is_none());
    }

    #[test]
    fn vote_threshold_keys_differ_by_entity_space_and_threshold() {
        let e1 = Uuid::from_bytes([0x01; 16]);
        let e2 = Uuid::from_bytes([0x02; 16]);
        let s1 = Uuid::from_bytes([0xA1; 16]);
        let s2 = Uuid::from_bytes([0xA2; 16]);

        let base = build_vote_threshold_event(e1, s1, 10, 0, 10).idempotency_key;
        // same inputs -> same key (idempotent: fires once per entity/space/threshold)
        assert_eq!(
            base,
            build_vote_threshold_event(e1, s1, 99, 1, 10).idempotency_key
        );
        // different entity / space / threshold -> distinct keys
        assert_ne!(
            base,
            build_vote_threshold_event(e2, s1, 10, 0, 10).idempotency_key
        );
        assert_ne!(
            base,
            build_vote_threshold_event(e1, s2, 10, 0, 10).idempotency_key
        );
        assert_ne!(
            base,
            build_vote_threshold_event(e1, s1, 10, 0, 25).idempotency_key
        );
    }
}
