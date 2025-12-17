//! Kafka emission module for sending transformed events.
//!
//! The `Emitter` wraps a Kafka producer and provides a generic `emit` method
//! for any type that implements `KafkaEvent + prost::Message`.

use anyhow::Result;
use hermes_instrumentation::debug_span;
use prost::Message;

use hermes_kafka::{BaseProducer, BaseRecord, Header, OwnedHeaders};
use hermes_schema::pb::{
    governance::{HermesProposalCreated, HermesProposalExecuted, HermesProposalVoted},
    knowledge::HermesEdit,
    membership::{HermesRoleGranted, HermesRoleRevoked, HermesSpaceLeft, MembershipRole},
    moderation::{
        HermesContentFlagged, HermesContentUnflagged, HermesEditorFlagged, HermesEditorUnflagged,
    },
    space::{
        HermesCreateSpace, HermesSpaceTrustExtension, hermes_create_space,
        hermes_space_trust_extension,
    },
    topics::HermesTopicDeclared,
    voting::{HermesVoteCast, VoteDirection},
};

// =============================================================================
// Topics
// =============================================================================

/// Kafka topics for each event type.
pub mod topics {
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
            Some(hermes_create_space::Payload::PersonalSpace(_)) => "PERSONAL",
            Some(hermes_create_space::Payload::DefaultDaoSpace(_)) => "DEFAULT_DAO",
            None => "UNKNOWN",
        };

        OwnedHeaders::new().insert(Header {
            key: "space-type",
            value: Some(space_type),
        })
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
            None => "UNKNOWN",
        };

        OwnedHeaders::new().insert(Header {
            key: "extension-type",
            value: Some(extension_type),
        })
    }
}

impl KafkaEvent for HermesEdit {
    const TOPIC: &'static str = topics::EDITS;

    fn key(&self) -> Vec<u8> {
        self.space_id.as_bytes().to_vec()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new()
            .insert(Header {
                key: "edit-name",
                value: Some(&self.name),
            })
            .insert(Header {
                key: "ops-count",
                value: Some(&self.ops.len().to_string()),
            })
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

// =============================================================================
// Emitter
// =============================================================================

/// Emitter wraps a Kafka producer and provides generic event emission.
pub struct Emitter {
    producer: BaseProducer,
}

impl Emitter {
    /// Create a new emitter wrapping the given Kafka producer.
    pub fn new(producer: BaseProducer) -> Self {
        Self { producer }
    }

    /// Emit any event that implements `KafkaEvent + Message`.
    pub fn emit<T: KafkaEvent + Message>(&self, event: &T) -> Result<()> {
        let mut payload = Vec::new();
        event.encode(&mut payload)?;

        debug_span!("kafka.send", topic = T::TOPIC, payload_size = payload.len()).in_scope(|| {
            let key = event.key();
            let record = BaseRecord::to(T::TOPIC)
                .key(&key)
                .payload(&payload)
                .headers(event.headers());

            self.producer
                .send(record)
                .map_err(|(e, _)| anyhow::anyhow!(e))
        })
    }

    /// Emit a batch of events.
    pub fn emit_batch<T: KafkaEvent + Message>(&self, events: &[T]) -> Result<u64> {
        let mut count = 0;
        for event in events {
            self.emit(event)?;
            count += 1;
        }
        Ok(count)
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
            topic_id: vec![],
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
            ops: vec![],
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
    }

    #[test]
    fn test_proposal_created_key() {
        use hermes_schema::pb::governance::VotingMode;
        let event = HermesProposalCreated {
            space_id: vec![0xAB; 16],
            proposal_id: vec![0xCD; 32],
            actions: vec![],
            voting_mode: VotingMode::Fast as i32,
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
            proposal_id: vec![0xCD; 32],
            vote: ProposalVoteOption::Yes as i32,
            meta: None,
        };
        // Should key by space_id, not voter_id
        assert_eq!(event.key(), vec![0xAB; 16]);
    }

    #[test]
    fn test_proposal_executed_key() {
        let event = HermesProposalExecuted {
            space_id: vec![0xAB; 16],
            proposal_id: vec![0xCD; 32],
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
    fn test_voting_topic() {
        assert_eq!(HermesVoteCast::TOPIC, "curation.votes");
    }

    #[test]
    fn test_role_granted_key() {
        let event = HermesRoleGranted {
            space_id: vec![0xAB; 16],
            account: vec![0xCD; 20],
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
}
