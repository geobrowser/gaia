//! Kafka utilities for the spaces transformer.
//!
//! Provides topic constants and send functions for space-related events.
//! Uses hermes-kafka for shared producer configuration.

use anyhow::Result;
use prost::Message;

use hermes_kafka::{BaseProducer, BaseRecord, Header, OwnedHeaders};
use hermes_schema::pb::space::{HermesCreateSpace, HermesSpaceTrustExtension};

// Re-export create_producer from hermes-kafka for convenience
pub use hermes_kafka::create_producer;

/// Topic for space creation events
pub const TOPIC_SPACE_CREATIONS: &str = "space.creations";

/// Topic for trust extension events (both additions and removals)
pub const TOPIC_TRUST_EXTENSIONS: &str = "space.trust.extensions";

/// Send a space creation event to Kafka.
///
/// Uses the space_id as the message key for partitioning.
/// Includes a header with the space type (PERSONAL or DEFAULT_DAO).
pub fn send_space_creation(producer: &BaseProducer, space: &HermesCreateSpace) -> Result<()> {
    let mut payload = Vec::new();
    space.encode(&mut payload)?;

    let space_type = match &space.payload {
        Some(hermes_schema::pb::space::hermes_create_space::Payload::PersonalSpace(_)) => {
            "PERSONAL"
        }
        Some(hermes_schema::pb::space::hermes_create_space::Payload::DefaultDaoSpace(_)) => {
            "DEFAULT_DAO"
        }
        None => "UNKNOWN",
    };

    let record = BaseRecord::to(TOPIC_SPACE_CREATIONS)
        .key(&space.space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "space-type",
            value: Some(space_type),
        }));

    producer.send(record).map_err(|(e, _)| anyhow::anyhow!(e))?;
    Ok(())
}

/// Send a trust extension event to Kafka.
///
/// Uses the source_space_id as the message key for partitioning.
/// Includes a header with the extension type (VERIFIED, RELATED, or SUBTOPIC).
pub fn send_trust_extension(
    producer: &BaseProducer,
    trust_extension: &HermesSpaceTrustExtension,
) -> Result<()> {
    let mut payload = Vec::new();
    trust_extension.encode(&mut payload)?;

    let extension_type = match &trust_extension.extension {
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Verified(_)) => {
            "VERIFIED"
        }
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Related(_)) => {
            "RELATED"
        }
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Subtopic(_)) => {
            "SUBTOPIC"
        }
        None => "UNKNOWN",
    };

    let record = BaseRecord::to(TOPIC_TRUST_EXTENSIONS)
        .key(&trust_extension.source_space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "extension-type",
            value: Some(extension_type),
        }));

    producer.send(record).map_err(|(e, _)| anyhow::anyhow!(e))?;
    Ok(())
}
