//! Kafka emission module for sending transformed events.
//!
//! This module provides a unified interface for emitting events to Kafka.
//! Each event type implements the `KafkaEmit` trait which defines how to
//! encode and send the event.

use anyhow::Result;
use prost::Message;

use hermes_kafka::{BaseProducer, BaseRecord, Header, OwnedHeaders};
use hermes_schema::pb::{
    knowledge::HermesEdit,
    space::{hermes_create_space, hermes_space_trust_extension, HermesCreateSpace, HermesSpaceTrustExtension},
};

/// Kafka topics for each event type.
pub mod topics {
    pub const SPACE_CREATIONS: &str = "space.creations";
    pub const TRUST_EXTENSIONS: &str = "space.trust.extensions";
    pub const EDITS: &str = "knowledge.edits";
}

/// Trait for types that can be emitted to Kafka.
pub trait KafkaEmit {
    /// The Kafka topic this event type is emitted to.
    fn topic(&self) -> &'static str;

    /// The key used for Kafka partitioning.
    fn key(&self) -> Vec<u8>;

    /// Encode the event to protobuf bytes.
    fn encode_payload(&self) -> Result<Vec<u8>>;

    /// Build Kafka headers for this event.
    fn headers(&self) -> OwnedHeaders;

    /// Emit this event to Kafka.
    fn emit(&self, producer: &BaseProducer) -> Result<()> {
        let payload = self.encode_payload()?;
        let key = self.key();

        let record = BaseRecord::to(self.topic())
            .key(&key)
            .payload(&payload)
            .headers(self.headers());

        producer.send(record).map_err(|(e, _)| anyhow::anyhow!(e))?;
        Ok(())
    }
}

// =============================================================================
// HermesCreateSpace implementation
// =============================================================================

impl KafkaEmit for HermesCreateSpace {
    fn topic(&self) -> &'static str {
        topics::SPACE_CREATIONS
    }

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn encode_payload(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.encode(&mut buf)?;
        Ok(buf)
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

// =============================================================================
// HermesSpaceTrustExtension implementation
// =============================================================================

impl KafkaEmit for HermesSpaceTrustExtension {
    fn topic(&self) -> &'static str {
        topics::TRUST_EXTENSIONS
    }

    fn key(&self) -> Vec<u8> {
        self.source_space_id.clone()
    }

    fn encode_payload(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.encode(&mut buf)?;
        Ok(buf)
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

// =============================================================================
// HermesEdit implementation
// =============================================================================

impl KafkaEmit for HermesEdit {
    fn topic(&self) -> &'static str {
        topics::EDITS
    }

    fn key(&self) -> Vec<u8> {
        self.space_id.as_bytes().to_vec()
    }

    fn encode_payload(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.encode(&mut buf)?;
        Ok(buf)
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
// Batch emission helper
// =============================================================================

/// Emit a batch of events to Kafka.
///
/// Returns the number of events successfully emitted.
pub fn emit_batch<T: KafkaEmit>(producer: &BaseProducer, events: &[T]) -> Result<u64> {
    let mut count = 0;
    for event in events {
        event.emit(producer)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_topic() {
        let space = HermesCreateSpace {
            space_id: vec![1; 16],
            topic_id: vec![2; 32],
            payload: None,
            meta: None,
        };
        assert_eq!(space.topic(), topics::SPACE_CREATIONS);
    }

    #[test]
    fn test_trust_topic() {
        let trust = HermesSpaceTrustExtension {
            source_space_id: vec![1; 16],
            extension: None,
            meta: None,
        };
        assert_eq!(trust.topic(), topics::TRUST_EXTENSIONS);
    }

    #[test]
    fn test_edit_topic() {
        let edit = HermesEdit {
            id: vec![1; 16],
            name: "Test".into(),
            ops: vec![],
            authors: vec![],
            language: None,
            space_id: "test_space".into(),
            is_canonical: true,
            meta: None,
        };
        assert_eq!(edit.topic(), topics::EDITS);
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
