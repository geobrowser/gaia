//! Kafka consumer for the `knowledge.edits` and `space.membership` topics.
//!
//! Mirrors the vote-indexer consumer: a manual-commit `StreamConsumer` with the
//! environment topic prefix applied. Edits are decoded in two steps —
//! `HermesEdit` (prost) then the raw GRC2/GRC2Z payload via the `grc-20` crate,
//! the same decoder the kg-indexer uses. Membership messages are dispatched on
//! the `event-type` Kafka header, same as the kg-indexer.

use hermes_kafka::get_topic_prefix;
use prost::Message;
use rdkafka::{
    config::ClientConfig,
    consumer::{Consumer, DefaultConsumerContext, StreamConsumer},
    TopicPartitionList,
};
use std::env;
use tracing::{debug, info};

use crate::error::IndexerError;
use crate::membership::MembershipEvent;

/// Base topic for knowledge edits.
const EDITS_TOPIC: &str = "knowledge.edits";

/// Base topic for membership events (role granted/revoked, space left).
const MEMBERSHIP_TOPIC: &str = "space.membership";

/// Which subscribed topic a message arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicKind {
    Edits,
    Membership,
}

/// Kafka consumer for knowledge edits + membership events.
pub struct KafkaConsumer {
    consumer: StreamConsumer<DefaultConsumerContext>,
    edits_topic: String,
    membership_topic: String,
}

impl KafkaConsumer {
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IndexerError> {
        let mut config = ClientConfig::new();
        config
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .set("session.timeout.ms", "6000");

        if let Ok(username) = env::var("KAFKA_USERNAME") {
            config.set("security.protocol", "SASL_SSL");
            config.set("sasl.mechanisms", "PLAIN");
            config.set("sasl.username", &username);

            let password = env::var("KAFKA_PASSWORD").map_err(|_| {
                IndexerError::Config(
                    "KAFKA_PASSWORD must be set when KAFKA_USERNAME is configured".into(),
                )
            })?;
            config.set("sasl.password", &password);
        }

        if let Ok(ca_pem) = env::var("KAFKA_SSL_CA_PEM") {
            config.set("ssl.ca.pem", &ca_pem);
        }

        let consumer: StreamConsumer = config.create()?;

        let prefix = get_topic_prefix();
        let edits_topic = format!("{}{}", prefix, EDITS_TOPIC);
        let membership_topic = format!("{}{}", prefix, MEMBERSHIP_TOPIC);

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic_prefix = %prefix,
            edits_topic = %edits_topic,
            membership_topic = %membership_topic,
            "Created Kafka consumer"
        );

        Ok(Self {
            consumer,
            edits_topic,
            membership_topic,
        })
    }

    pub fn subscribe(&self) -> Result<(), IndexerError> {
        self.consumer
            .subscribe(&[&self.edits_topic, &self.membership_topic])?;
        info!(
            edits_topic = %self.edits_topic,
            membership_topic = %self.membership_topic,
            "Subscribed to Kafka topics"
        );
        Ok(())
    }

    /// Which of the subscribed topics a message's topic is.
    pub fn topic_kind(&self, topic: &str) -> Option<TopicKind> {
        if topic == self.edits_topic {
            Some(TopicKind::Edits)
        } else if topic == self.membership_topic {
            Some(TopicKind::Membership)
        } else {
            None
        }
    }

    pub fn stream(&self) -> rdkafka::consumer::MessageStream<'_, DefaultConsumerContext> {
        self.consumer.stream()
    }

    pub fn commit_message(
        &self,
        topic: &str,
        partition: i32,
        offset: i64,
    ) -> Result<(), IndexerError> {
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(topic, partition, rdkafka::Offset::Offset(offset + 1))?;
        self.consumer
            .commit(&tpl, rdkafka::consumer::CommitMode::Async)?;
        debug!(topic = %topic, partition = partition, offset = offset, "Committed offset");
        Ok(())
    }
}

/// Decode the `HermesEdit` envelope from a Kafka payload.
pub fn parse_edit(
    payload: &[u8],
) -> Result<hermes_schema::pb::knowledge::HermesEdit, IndexerError> {
    Ok(hermes_schema::pb::knowledge::HermesEdit::decode(payload)?)
}

/// Decode the raw GRC2/GRC2Z payload bytes into a `grc_20::Edit` (handles both
/// compressed and uncompressed forms, same as the kg-indexer).
pub fn decode_grc20(payload: &[u8]) -> Result<grc_20::Edit<'_>, IndexerError> {
    grc_20::decode_edit(payload).map_err(|e| IndexerError::decode(format!("grc20: {e}")))
}

/// The `event-type` Kafka header, which selects the protobuf message type on
/// the membership topic (same convention as the kg-indexer).
pub fn get_event_type(headers: Option<&rdkafka::message::BorrowedHeaders>) -> Option<String> {
    use rdkafka::message::Headers;
    headers.and_then(|h| {
        for header in h.iter() {
            if header.key == "event-type" {
                if let Some(value) = header.value {
                    return String::from_utf8(value.to_vec()).ok();
                }
            }
        }
        None
    })
}

/// Decode a `space.membership` message, dispatching on the `event-type` header.
pub fn parse_membership_event(
    payload: &[u8],
    event_type: Option<&str>,
) -> Result<MembershipEvent, IndexerError> {
    match event_type {
        Some("ROLE_GRANTED") => Ok(MembershipEvent::RoleGranted(
            hermes_schema::pb::membership::HermesRoleGranted::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesRoleGranted: {e}")))?,
        )),
        Some("ROLE_REVOKED") => Ok(MembershipEvent::RoleRevoked(
            hermes_schema::pb::membership::HermesRoleRevoked::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesRoleRevoked: {e}")))?,
        )),
        Some("SPACE_LEFT") => Ok(MembershipEvent::SpaceLeft(
            hermes_schema::pb::membership::HermesSpaceLeft::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesSpaceLeft: {e}")))?,
        )),
        other => Err(IndexerError::decode(format!(
            "unknown membership event type: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::membership::{HermesRoleGranted, MembershipRole};
    use uuid::Uuid;

    #[test]
    fn membership_dispatches_on_event_type_header() {
        let granted = HermesRoleGranted {
            space_id: Uuid::from_u128(1).as_bytes().to_vec(),
            member_space_id: Uuid::from_u128(2).as_bytes().to_vec(),
            role: MembershipRole::Member as i32,
            meta: None,
        };
        let payload = granted.encode_to_vec();

        assert!(matches!(
            parse_membership_event(&payload, Some("ROLE_GRANTED")),
            Ok(MembershipEvent::RoleGranted(_))
        ));
        assert!(matches!(
            parse_membership_event(&payload, Some("ROLE_REVOKED")),
            Ok(MembershipEvent::RoleRevoked(_))
        ));
        // missing or unknown header -> decode error, not a crash
        assert!(parse_membership_event(&payload, None).is_err());
        assert!(parse_membership_event(&payload, Some("BOGUS")).is_err());
    }
}
