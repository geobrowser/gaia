//! Domain models for notification events.

use hermes_schema::pb::governance::{
    HermesProposalCreated, HermesProposalExecuted, HermesProposalSettingsUpdated,
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
}

/// Result of handling a governance event: payload + idempotency key.
#[derive(Debug)]
pub struct NotificationEvent {
    pub event_type: NotificationEventType,
    pub idempotency_key: String,
    pub payload: NotificationPayload,
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
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: Some(proposer_id.to_string()),
                voter_id: None,
                vote: None,
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
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: Some(proposer_id.to_string()),
                voter_id: None,
                vote: None,
            }),
        },
    })
}

/// Map a protobuf vote option to a string for the webhook payload.
fn vote_option_to_string(vote: i32) -> String {
    match ProposalVoteOption::try_from(vote) {
        Ok(ProposalVoteOption::VoteOptionYes) => "yes".to_string(),
        Ok(ProposalVoteOption::VoteOptionNo) => "no".to_string(),
        Ok(ProposalVoteOption::VoteOptionAbstain) => "abstain".to_string(),
        _ => "unknown".to_string(),
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
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: None,
                voter_id: Some(voter_id.to_string()),
                vote: Some(vote_option_to_string(msg.vote)),
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
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: None,
                voter_id: None,
                vote: None,
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
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: None,
                voter_id: None,
                vote: None,
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
            data: NotificationData::Governance(GovernanceData {
                proposal_id: proposal_id.to_string(),
                proposer_id: Some(proposed_by.to_string()),
                voter_id: None,
                vote: None,
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// Bounty configuration and handlers
// ---------------------------------------------------------------------------

/// Well-known relation type UUIDs for bounty events.
/// Hardcoded from the GRC-20 protocol; env vars override if set.
const DEFAULT_INTEREST_TYPE_ID: &str = "2c765cae-c1b6-4cc3-a65d-693d0a67eaeb";
// Placeholder UUIDs — update when the protocol defines them
const DEFAULT_ALLOCATED_TYPE_ID: &str = "00000000-0000-0000-0000-000000000000";
const DEFAULT_PAYOUT_TYPE_ID: &str = "00000000-0000-0000-0000-000000000000";

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
            data: NotificationData::Bounty(BountyData {
                bounty_entity_id: info.bounty_entity_id.to_string(),
                relation_id: info.relation_id.to_string(),
                curator_space_id: info.curator_space_id.to_string(),
                bounty_space_id: info.bounty_space_id.to_string(),
                proposal_id: None,
                interested_user_space_id: Some(info.curator_space_id.to_string()),
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
            data: NotificationData::Bounty(BountyData {
                bounty_entity_id: info.bounty_entity_id.to_string(),
                relation_id: info.relation_id.to_string(),
                curator_space_id: info.curator_space_id.to_string(),
                bounty_space_id: info.bounty_space_id.to_string(),
                proposal_id: info.proposal_id.map(|p| p.to_string()),
                interested_user_space_id: None,
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
            data: NotificationData::Bounty(BountyData {
                bounty_entity_id: info.bounty_entity_id.to_string(),
                relation_id: info.relation_id.to_string(),
                curator_space_id: info.curator_space_id.to_string(),
                bounty_space_id: info.bounty_space_id.to_string(),
                proposal_id: info.proposal_id.map(|p| p.to_string()),
                interested_user_space_id: None,
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

    #[test]
    fn test_bounty_config_default() {
        let config = BountyConfig::default();
        // Should parse hardcoded UUIDs without panicking
        assert_ne!(config.interest_type_id, Uuid::nil());
    }

    #[test]
    fn test_bounty_config_match_type() {
        let config = BountyConfig::default();

        assert_eq!(
            config.match_type(&config.interest_type_id),
            Some(NotificationEventType::BountyInterest)
        );
        // allocated and payout use placeholder UUIDs — test with distinct values
        let config = BountyConfig {
            interest_type_id: Uuid::from_bytes([0x01; 16]),
            allocated_type_id: Uuid::from_bytes([0x02; 16]),
            payout_type_id: Uuid::from_bytes([0x03; 16]),
        };
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
        // Unknown type returns None
        assert_eq!(config.match_type(&Uuid::from_bytes([0xFF; 16])), None);
    }
}
