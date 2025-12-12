//! Kafka emission module for sending transformed events.
//!
//! The `Emitter` wraps a Kafka producer and provides a generic `emit` method
//! for any type that implements `KafkaEvent + prost::Message`.

use anyhow::Result;
use prost::Message;

use hermes_kafka::{BaseProducer, BaseRecord, Header, OwnedHeaders};
use hermes_schema::pb::{
    knowledge::HermesEdit,
    space::{
        hermes_create_space, hermes_space_trust_extension, HermesCreateSpace,
        HermesSpaceTrustExtension,
    },
};

// =============================================================================
// Topics
// =============================================================================

/// Kafka topics for each event type.
pub mod topics {
    pub const SPACE_CREATIONS: &str = "space.creations";
    pub const TRUST_EXTENSIONS: &str = "space.trust.extensions";
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

        let key = event.key();
        let record = BaseRecord::to(T::TOPIC)
            .key(&key)
            .payload(&payload)
            .headers(event.headers());

        self.producer
            .send(record)
            .map_err(|(e, _)| anyhow::anyhow!(e))?;
        Ok(())
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
}
