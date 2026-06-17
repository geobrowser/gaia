//! Kafka emission module for sending transformed events.
//!
//! The `Emitter` wraps a Kafka producer and provides a generic `emit` method
//! for any type that implements `KafkaEvent + prost::Message`.

use anyhow::Result;
use hermes_instrumentation::{Span, debug, error, info};
use opentelemetry::global;
use opentelemetry::propagation::Injector;
use prost::Message;
use std::sync::OnceLock;
use std::time::Instant;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use hermes_kafka::{FutureProducer, FutureRecord, Header, OwnedHeaders, Producer};
use hermes_schema::pb::{
    block_summary::HermesBlockSummary,
    blockchain_metadata::BlockchainMetadata,
    governance::{
        HermesProposalCreated, HermesProposalExecuted, HermesProposalSettingsUpdated,
        HermesProposalUpdated, HermesProposalVoted, HermesVotingSettingsUpdated,
    },
    knowledge::HermesEdit,
    membership::{HermesRoleGranted, HermesRoleRevoked, HermesSpaceLeft, MembershipRole},
    moderation::{
        HermesContentFlagged, HermesContentUnflagged, HermesEditorFlagged, HermesEditorUnflagged,
    },
    space::{
        HermesCreateSpace, HermesSpaceTrustExtension, hermes_create_space,
        hermes_space_trust_extension,
    },
    topics::{HermesTopicDeclared, HermesTopicRemoved},
    voting::{HermesVoteCast, VoteDirection},
};

// =============================================================================
// Topics
// =============================================================================

/// Kafka topics for each event type.
pub mod topics {
    pub const BLOCK_SUMMARY: &str = "hermes.blocks";
    pub const SPACE_CREATIONS: &str = "space.creations";
    pub const TRUST_EXTENSIONS: &str = "space.trust.extensions";
    pub const MEMBERSHIP: &str = "space.membership";
    pub const MODERATION: &str = "space.moderation";
    pub const TOPICS: &str = "space.topics";
    pub const GOVERNANCE: &str = "space.governance";
    pub const VOTING: &str = "curation.votes";
    pub const EDITS: &str = "knowledge.edits";
}

// =============================================================================
// KafkaEvent trait
// =============================================================================

/// Trait for types that can be emitted to Kafka.
///
/// Each implementing type declares its topic as an associated constant,
/// providing a compile-time mapping from protobuf type to Kafka topic.
pub trait KafkaEvent {
    /// The Kafka topic this event type is emitted to.
    const TOPIC: &'static str;

    /// The key used for Kafka partitioning.
    fn key(&self) -> Vec<u8>;

    /// Build Kafka headers for this event.
    fn headers(&self) -> OwnedHeaders;
}

pub trait HasMeta {
    fn meta(&self) -> Option<&BlockchainMetadata>;

    fn event_id(&self, topic: &str) -> Option<String> {
        self.meta().map(|meta| event_id_for(meta, topic))
    }
}

// =============================================================================
// KafkaEvent implementations
// =============================================================================

impl KafkaEvent for HermesCreateSpace {
    const TOPIC: &'static str = topics::SPACE_CREATIONS;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        let space_type = match &self.payload {
            Some(hermes_create_space::Payload::EoaSpace(_)) => "EOA",
            Some(hermes_create_space::Payload::DefaultDaoSpace(_)) => "DEFAULT_DAO",
            None => "UNKNOWN",
        };

        OwnedHeaders::new().insert(Header {
            key: "space-type",
            value: Some(space_type),
        })
    }
}

impl KafkaEvent for HermesBlockSummary {
    const TOPIC: &'static str = topics::BLOCK_SUMMARY;

    fn key(&self) -> Vec<u8> {
        self.block_number.to_be_bytes().to_vec()
    }

    fn headers(&self) -> OwnedHeaders {
        let event_id = format!("block_summary:{}:{}", self.block_number, self.cursor);
        OwnedHeaders::new()
            .insert(Header {
                key: "event-type",
                value: Some("BLOCK_SUMMARY"),
            })
            .insert(Header {
                key: "event-id",
                value: Some(event_id.as_str()),
            })
    }
}

impl HasMeta for HermesBlockSummary {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        None
    }

    fn event_id(&self, _topic: &str) -> Option<String> {
        Some(format!(
            "block_summary:{}:{}",
            self.block_number, self.cursor
        ))
    }
}

impl HasMeta for HermesCreateSpace {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesSpaceTrustExtension {
    const TOPIC: &'static str = topics::TRUST_EXTENSIONS;

    fn key(&self) -> Vec<u8> {
        self.source_space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        let extension_type = match &self.extension {
            Some(hermes_space_trust_extension::Extension::Verified(_)) => "VERIFIED",
            Some(hermes_space_trust_extension::Extension::Related(_)) => "RELATED",
            Some(hermes_space_trust_extension::Extension::Subtopic(_)) => "SUBTOPIC",
            Some(hermes_space_trust_extension::Extension::VerifiedRemoval(_)) => "VERIFIED_REMOVAL",
            Some(hermes_space_trust_extension::Extension::RelatedRemoval(_)) => "RELATED_REMOVAL",
            Some(hermes_space_trust_extension::Extension::SubtopicRemoval(_)) => "SUBTOPIC_REMOVAL",
            None => "UNKNOWN",
        };

        OwnedHeaders::new().insert(Header {
            key: "extension-type",
            value: Some(extension_type),
        })
    }
}

impl HasMeta for HermesSpaceTrustExtension {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesEdit {
    const TOPIC: &'static str = topics::EDITS;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "edit-name",
            value: Some(&self.name),
        })
    }
}

impl HasMeta for HermesEdit {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesProposalCreated {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_CREATED"),
        })
    }
}

impl HasMeta for HermesProposalCreated {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesProposalUpdated {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_UPDATED"),
        })
    }
}

impl HasMeta for HermesProposalUpdated {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesProposalVoted {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone() // Key by proposal's space for ordering
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_VOTED"),
        })
    }
}

impl HasMeta for HermesProposalVoted {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesProposalExecuted {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_EXECUTED"),
        })
    }
}

impl HasMeta for HermesProposalExecuted {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesProposalSettingsUpdated {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_SETTINGS_UPDATED"),
        })
    }
}

impl HasMeta for HermesProposalSettingsUpdated {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesVotingSettingsUpdated {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("VOTING_SETTINGS_UPDATED"),
        })
    }
}

impl HasMeta for HermesVotingSettingsUpdated {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

// =============================================================================
// Membership events
// =============================================================================

impl KafkaEvent for HermesRoleGranted {
    const TOPIC: &'static str = topics::MEMBERSHIP;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        let role = match MembershipRole::try_from(self.role) {
            Ok(MembershipRole::Editor) => "EDITOR",
            Ok(MembershipRole::Member) => "MEMBER",
            Err(_) => "UNKNOWN",
        };
        OwnedHeaders::new()
            .insert(Header {
                key: "event-type",
                value: Some("ROLE_GRANTED"),
            })
            .insert(Header {
                key: "role",
                value: Some(role),
            })
    }
}

impl HasMeta for HermesRoleGranted {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesRoleRevoked {
    const TOPIC: &'static str = topics::MEMBERSHIP;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        let role = match MembershipRole::try_from(self.role) {
            Ok(MembershipRole::Editor) => "EDITOR",
            Ok(MembershipRole::Member) => "MEMBER",
            Err(_) => "UNKNOWN",
        };
        OwnedHeaders::new()
            .insert(Header {
                key: "event-type",
                value: Some("ROLE_REVOKED"),
            })
            .insert(Header {
                key: "role",
                value: Some(role),
            })
    }
}

impl HasMeta for HermesRoleRevoked {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesSpaceLeft {
    const TOPIC: &'static str = topics::MEMBERSHIP;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("SPACE_LEFT"),
        })
    }
}

impl HasMeta for HermesSpaceLeft {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

// =============================================================================
// Moderation events
// =============================================================================

impl KafkaEvent for HermesEditorFlagged {
    const TOPIC: &'static str = topics::MODERATION;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("EDITOR_FLAGGED"),
        })
    }
}

impl HasMeta for HermesEditorFlagged {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesEditorUnflagged {
    const TOPIC: &'static str = topics::MODERATION;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("EDITOR_UNFLAGGED"),
        })
    }
}

impl HasMeta for HermesEditorUnflagged {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesContentFlagged {
    const TOPIC: &'static str = topics::MODERATION;

    fn key(&self) -> Vec<u8> {
        self.target_space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("CONTENT_FLAGGED"),
        })
    }
}

impl HasMeta for HermesContentFlagged {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesContentUnflagged {
    const TOPIC: &'static str = topics::MODERATION;

    fn key(&self) -> Vec<u8> {
        self.target_space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("CONTENT_UNFLAGGED"),
        })
    }
}

impl HasMeta for HermesContentUnflagged {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

// =============================================================================
// Topic events
// =============================================================================

impl KafkaEvent for HermesTopicDeclared {
    const TOPIC: &'static str = topics::TOPICS;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("TOPIC_DECLARED"),
        })
    }
}

impl HasMeta for HermesTopicDeclared {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

impl KafkaEvent for HermesTopicRemoved {
    const TOPIC: &'static str = topics::TOPICS;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("TOPIC_REMOVED"),
        })
    }
}

impl HasMeta for HermesTopicRemoved {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

// =============================================================================
// Voting events
// =============================================================================

impl KafkaEvent for HermesVoteCast {
    const TOPIC: &'static str = topics::VOTING;

    fn key(&self) -> Vec<u8> {
        // Key by object_id for partitioning votes on the same object together
        self.object_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        let direction = match VoteDirection::try_from(self.direction) {
            Ok(VoteDirection::Up) => "UP",
            Ok(VoteDirection::Down) => "DOWN",
            Ok(VoteDirection::None) => "NONE",
            Err(_) => "UNKNOWN",
        };
        OwnedHeaders::new()
            .insert(Header {
                key: "event-type",
                value: Some("VOTE_CAST"),
            })
            .insert(Header {
                key: "direction",
                value: Some(direction),
            })
    }
}

impl HasMeta for HermesVoteCast {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.meta.as_ref()
    }
}

struct HeaderInjector {
    headers: OwnedHeaders,
}

impl HeaderInjector {
    fn new(headers: OwnedHeaders) -> Self {
        Self { headers }
    }

    fn into_headers(self) -> OwnedHeaders {
        self.headers
    }
}

impl Injector for HeaderInjector {
    fn set(&mut self, key: &str, value: String) {
        let headers = std::mem::take(&mut self.headers);
        self.headers = headers.insert(Header {
            key,
            value: Some(value.as_str()),
        });
    }
}

fn event_id_for(meta: &BlockchainMetadata, topic: &str) -> String {
    format!(
        "{}:{}:{}:{}",
        topic, meta.block_number, meta.sequence, meta.cursor
    )
}

fn attach_event_id<T: HasMeta>(headers: OwnedHeaders, event: &T, topic: &str) -> OwnedHeaders {
    if let Some(meta) = event.meta() {
        let event_id = event_id_for(meta, topic);
        headers.insert(Header {
            key: "event-id",
            value: Some(event_id.as_str()),
        })
    } else {
        headers
    }
}

fn inject_trace_headers(headers: OwnedHeaders) -> OwnedHeaders {
    let span = Span::current();
    let context = span.context();
    let mut injector = HeaderInjector::new(headers);
    global::get_text_map_propagator(|prop| prop.inject_context(&context, &mut injector));
    injector.into_headers()
}

fn log_event_ids_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("LOG_EVENT_IDS")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    })
}

// Re-export topic utilities from hermes-kafka
use hermes_kafka::{get_topic_prefix, kafka_send_timeout, prefixed_topic};

// =============================================================================
// Emitter
// =============================================================================

/// Emitter wraps a Kafka producer and provides generic event emission.
pub struct Emitter {
    producer: FutureProducer,
    topic_prefix: &'static str,
}

impl Emitter {
    /// Create a new emitter wrapping the given Kafka producer.
    ///
    /// Reads `ENVIRONMENT` from environment to support environment isolation.
    /// If set to "staging", all topics will be prefixed with "staging.".
    pub fn new(producer: FutureProducer) -> Self {
        Self::new_with_prefix(producer, get_topic_prefix())
    }

    /// Create an emitter with an explicit topic prefix. Bypasses the
    /// `ENVIRONMENT` env-var lookup that `Emitter::new` performs via
    /// `get_topic_prefix`. Useful for tests so they don't have to mutate
    /// the process-wide ENVIRONMENT variable to satisfy the prefix
    /// `OnceLock` cache (which is initialized exactly once for the
    /// lifetime of the process and would otherwise leak between tests).
    pub fn new_with_prefix(producer: FutureProducer, topic_prefix: &'static str) -> Self {
        info!(
            topic_prefix = %topic_prefix,
            "Kafka topic prefix configured"
        );
        Self {
            producer,
            topic_prefix,
        }
    }

    /// Emit any event that implements `KafkaEvent + Message`.
    pub async fn emit<T: KafkaEvent + Message + HasMeta>(&self, event: &T) -> Result<()> {
        let mut payload = Vec::new();
        event.encode(&mut payload)?;

        // Apply topic prefix for environment isolation
        let topic = prefixed_topic(self.topic_prefix, T::TOPIC);

        let headers = event.headers();
        let headers = attach_event_id(headers, event, &topic);
        let headers = inject_trace_headers(headers);
        let event_id = event
            .event_id(&topic)
            .unwrap_or_else(|| "unknown".to_string());

        if log_event_ids_enabled() {
            info!(
                event = "hermes_pipeline.event_id",
                topic = %topic,
                event_id = %event_id,
                "Emitting event"
            );
        }

        let key = event.key();
        let record = FutureRecord::to(&topic)
            .key(&key)
            .payload(&payload)
            .headers(headers);

        let send_start = Instant::now();
        let result = self.producer.send(record, kafka_send_timeout()).await;

        match result {
            Ok((partition, offset)) => {
                debug!(
                    event = "hermes_pipeline.event_delivered",
                    stage = "kafka.send",
                    topic = %topic,
                    event_id = %event_id,
                    payload_size = payload.len(),
                    partition = partition,
                    offset = offset,
                    duration_ms = send_start.elapsed().as_millis(),
                    "Kafka send delivered"
                );
                Ok(())
            }
            Err((err, _)) => {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "kafka.send",
                    topic = %topic,
                    event_id = %event_id,
                    error = %err,
                    "Kafka send failed"
                );
                Err(anyhow::anyhow!(err))
            }
        }
    }

    /// Emit a batch of events.
    pub async fn emit_batch<T: KafkaEvent + Message + HasMeta>(&self, events: &[T]) -> Result<u64> {
        let mut count = 0;
        for event in events {
            self.emit(event).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Flush all pending messages to Kafka.
    ///
    /// This should be called before shutting down the application to ensure
    /// all queued messages are delivered to Kafka.
    pub fn flush(&self, timeout: std::time::Duration) {
        let _ = self.producer.flush(timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_topic() {
        assert_eq!(HermesCreateSpace::TOPIC, "space.creations");
    }

    #[test]
    fn test_trust_topic() {
        assert_eq!(HermesSpaceTrustExtension::TOPIC, "space.trust.extensions");
    }

    #[test]
    fn test_edit_topic() {
        assert_eq!(HermesEdit::TOPIC, "knowledge.edits");
    }

    #[test]
    fn test_space_key() {
        let space = HermesCreateSpace {
            space_id: vec![0xAB; 16],
            payload: None,
            meta: None,
        };
        assert_eq!(space.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_edit_key() {
        let edit = HermesEdit {
            id: vec![],
            name: "".into(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: "my_space_id".into(),
            is_canonical: true,
            meta: None,
        };
        assert_eq!(edit.key(), b"my_space_id".to_vec());
    }

    #[test]
    fn test_governance_topics() {
        assert_eq!(HermesProposalCreated::TOPIC, "space.governance");
        assert_eq!(HermesProposalVoted::TOPIC, "space.governance");
        assert_eq!(HermesProposalExecuted::TOPIC, "space.governance");
        assert_eq!(HermesProposalSettingsUpdated::TOPIC, "space.governance");
        assert_eq!(HermesVotingSettingsUpdated::TOPIC, "space.governance");
    }

    #[test]
    fn test_voting_settings_updated_key() {
        let event = HermesVotingSettingsUpdated {
            space_id: vec![0xAB; 16],
            partial_percentage_support_threshold: 0,
            universal_percentage_support_threshold: 0,
            flat_support_threshold: 0,
            quorum: 0,
            duration: 0,
            disable_fast_path_access_for_new_members: false,
            execution_grace_period: 0,
            meta: None,
        };
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_proposal_created_key() {
        use hermes_schema::pb::governance::VotingMode;
        let event = HermesProposalCreated {
            space_id: vec![0xAB; 16],
            proposer_id: vec![0x11; 16],
            proposal_id: vec![0xCD; 16],
            voting_mode: VotingMode::Fast as i32,
            actions: vec![],
            settings: None,
            meta: None,
        };
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_proposal_voted_key() {
        use hermes_schema::pb::governance::ProposalVoteOption;
        let event = HermesProposalVoted {
            voter_id: vec![0x11; 16],
            space_id: vec![0xAB; 16],
            proposal_id: vec![0xCD; 16],
            vote: ProposalVoteOption::VoteOptionYes as i32,
            meta: None,
            proposal_version: 1,
        };
        // Should key by space_id, not voter_id
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_proposal_executed_key() {
        let event = HermesProposalExecuted {
            space_id: vec![0xAB; 16],
            proposal_id: vec![0xCD; 16],
            meta: None,
        };
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_membership_topics() {
        assert_eq!(HermesRoleGranted::TOPIC, "space.membership");
        assert_eq!(HermesRoleRevoked::TOPIC, "space.membership");
        assert_eq!(HermesSpaceLeft::TOPIC, "space.membership");
    }

    #[test]
    fn test_moderation_topics() {
        assert_eq!(HermesEditorFlagged::TOPIC, "space.moderation");
        assert_eq!(HermesEditorUnflagged::TOPIC, "space.moderation");
        assert_eq!(HermesContentFlagged::TOPIC, "space.moderation");
        assert_eq!(HermesContentUnflagged::TOPIC, "space.moderation");
    }

    #[test]
    fn test_topic_declared_topic() {
        assert_eq!(HermesTopicDeclared::TOPIC, "space.topics");
    }

    #[test]
    fn test_topic_removed_topic() {
        assert_eq!(HermesTopicRemoved::TOPIC, "space.topics");
    }

    #[test]
    fn test_topic_removed_key() {
        let event = HermesTopicRemoved {
            space_id: vec![0xAB; 16],
            topic_id: vec![0xCD; 16],
            meta: None,
        };
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_voting_topic() {
        assert_eq!(HermesVoteCast::TOPIC, "curation.votes");
    }

    #[test]
    fn test_role_granted_key() {
        let event = HermesRoleGranted {
            space_id: vec![0xAB; 16],
            member_space_id: vec![0xEF; 16],
            role: MembershipRole::Editor as i32,
            meta: None,
        };
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_vote_cast_key() {
        let event = HermesVoteCast {
            voter_id: vec![0x11; 16],
            object_type: vec![0x00, 0x00, 0x00, 0x01],
            object_id: vec![0xAB; 16],
            direction: VoteDirection::Up as i32,
            version: 1,
            group_id: vec![0; 16],
            space_pov: vec![0; 16],
            meta: None,
        };
        // Should key by object_id for partitioning
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    // Topic prefix tests are in hermes-kafka crate
}
