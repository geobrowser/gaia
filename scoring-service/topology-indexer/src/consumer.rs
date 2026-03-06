//! Kafka consumer for the topology.canonical topic.

use hermes_kafka::get_topic_prefix;
use prost::Message;
use rdkafka::{
    config::ClientConfig,
    consumer::{Consumer, DefaultConsumerContext, StreamConsumer},
    TopicPartitionList,
};
use std::env;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::IndexerError;

/// Base topic for canonical graph diffs
const TOPOLOGY_TOPIC: &str = "topology.canonical";

/// Parsed change from a CanonicalGraphDiff message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyChange {
    pub space_id: Uuid,
    pub change_type: ChangeType,
    /// Distance from root (present for Added/Moved, absent for Removed).
    pub distance: Option<u32>,
}

/// Type of topology change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Added,
    Removed,
    Moved,
}

/// Result of parsing a CanonicalGraphDiff message.
#[derive(Debug)]
pub struct ParsedDiff {
    pub root_id: Uuid,
    pub changes: Vec<TopologyChange>,
}

/// Kafka consumer for topology events.
pub struct KafkaConsumer {
    consumer: StreamConsumer<DefaultConsumerContext>,
    topic: String,
}

impl KafkaConsumer {
    /// Create a new Kafka consumer.
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IndexerError> {
        let mut config = ClientConfig::new();
        config
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .set("session.timeout.ms", "6000");

        // Optional SASL/SSL configuration
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

        // Optional custom CA certificate
        if let Ok(ca_pem) = env::var("KAFKA_SSL_CA_PEM") {
            config.set("ssl.ca.pem", &ca_pem);
        }

        let consumer: StreamConsumer = config.create()?;

        let prefix = get_topic_prefix();
        let topic = format!("{}{}", prefix, TOPOLOGY_TOPIC);

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic_prefix = %prefix,
            topic = %topic,
            "Created Kafka consumer"
        );

        Ok(Self { consumer, topic })
    }

    /// Subscribe to the topology.canonical topic.
    pub fn subscribe(&self) -> Result<(), IndexerError> {
        self.consumer.subscribe(&[&self.topic])?;
        info!(topic = %self.topic, "Subscribed to Kafka topic");
        Ok(())
    }

    /// Get a message stream from the consumer.
    pub fn stream(&self) -> rdkafka::consumer::MessageStream<'_, DefaultConsumerContext> {
        self.consumer.stream()
    }

    /// Commit a message offset.
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

/// Parse a CanonicalGraphDiff message from Kafka payload.
pub fn parse_diff(payload: &[u8]) -> Result<Option<ParsedDiff>, IndexerError> {
    use hermes_schema::pb::topology::{CanonicalGraphDiff, ChangeType as ProtoChangeType};

    let diff = CanonicalGraphDiff::decode(payload)?;

    // Validate root_id (must be exactly 16 bytes for UUID)
    let root_id = match diff.root_id.as_slice().try_into() {
        Ok(bytes) => Uuid::from_bytes(bytes),
        Err(_) => {
            warn!(
                root_id_len = diff.root_id.len(),
                "Invalid root_id length, skipping message"
            );
            return Ok(None);
        }
    };

    let mut changes = Vec::with_capacity(diff.changes.len());

    for node_change in &diff.changes {
        // Validate space_id
        let space_id: [u8; 16] = match node_change.space_id.as_slice().try_into() {
            Ok(bytes) => bytes,
            Err(_) => {
                warn!(
                    space_id_len = node_change.space_id.len(),
                    "Invalid space_id length, skipping node change"
                );
                continue;
            }
        };

        // Map change type
        let change_type = match ProtoChangeType::try_from(node_change.change_type) {
            Ok(ProtoChangeType::Added) => ChangeType::Added,
            Ok(ProtoChangeType::Removed) => ChangeType::Removed,
            Ok(ProtoChangeType::Moved) => ChangeType::Moved,
            Ok(ProtoChangeType::Unspecified) | Err(_) => {
                warn!(
                    change_type = node_change.change_type,
                    "Unrecognized change type, skipping node change"
                );
                continue;
            }
        };

        changes.push(TopologyChange {
            space_id: Uuid::from_bytes(space_id),
            change_type,
            distance: node_change.distance,
        });
    }

    if changes.is_empty() {
        return Ok(None);
    }

    Ok(Some(ParsedDiff { root_id, changes }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::topology::{
        CanonicalGraphDiff, ChangeType as ProtoChangeType, NodeChange,
    };
    use prost::Message;

    fn make_uuid_bytes(id: u128) -> Vec<u8> {
        id.to_be_bytes().to_vec()
    }

    #[test]
    fn test_parse_diff_added() {
        let root_id = make_uuid_bytes(1);
        let space_id = make_uuid_bytes(2);

        let diff = CanonicalGraphDiff {
            root_id: root_id.clone(),
            changes: vec![NodeChange {
                space_id: space_id.clone(),
                change_type: ProtoChangeType::Added as i32,
                distance: Some(3),
                parent_edge: None,
            }],
            meta: None,
        };

        let payload = diff.encode_to_vec();
        let parsed = parse_diff(&payload)
            .expect("parse_diff should not fail")
            .expect("parsed diff should not be None");

        assert_eq!(parsed.root_id, Uuid::from_bytes(1u128.to_be_bytes()));
        assert_eq!(parsed.changes.len(), 1);
        assert_eq!(parsed.changes[0].change_type, ChangeType::Added);
        assert_eq!(parsed.changes[0].distance, Some(3));
        assert_eq!(
            parsed.changes[0].space_id,
            Uuid::from_bytes(2u128.to_be_bytes())
        );
    }

    #[test]
    fn test_parse_diff_removed() {
        let root_id = make_uuid_bytes(1);
        let space_id = make_uuid_bytes(2);

        let diff = CanonicalGraphDiff {
            root_id,
            changes: vec![NodeChange {
                space_id,
                change_type: ProtoChangeType::Removed as i32,
                distance: None,
                parent_edge: None,
            }],
            meta: None,
        };

        let payload = diff.encode_to_vec();
        let parsed = parse_diff(&payload)
            .expect("parse_diff should not fail")
            .expect("parsed diff should not be None");

        assert_eq!(parsed.changes[0].change_type, ChangeType::Removed);
        assert_eq!(parsed.changes[0].distance, None);
    }

    #[test]
    fn test_parse_diff_moved() {
        let root_id = make_uuid_bytes(1);
        let space_id = make_uuid_bytes(3);

        let diff = CanonicalGraphDiff {
            root_id,
            changes: vec![NodeChange {
                space_id,
                change_type: ProtoChangeType::Moved as i32,
                distance: Some(5),
                parent_edge: None,
            }],
            meta: None,
        };

        let payload = diff.encode_to_vec();
        let parsed = parse_diff(&payload)
            .expect("parse_diff should not fail")
            .expect("parsed diff should not be None");

        assert_eq!(parsed.changes[0].change_type, ChangeType::Moved);
        assert_eq!(parsed.changes[0].distance, Some(5));
    }

    #[test]
    fn test_parse_diff_skips_unspecified_change_type() {
        let root_id = make_uuid_bytes(1);

        let diff = CanonicalGraphDiff {
            root_id,
            changes: vec![NodeChange {
                space_id: make_uuid_bytes(2),
                change_type: ProtoChangeType::Unspecified as i32,
                distance: None,
                parent_edge: None,
            }],
            meta: None,
        };

        let payload = diff.encode_to_vec();
        let parsed = parse_diff(&payload).expect("parse_diff should not fail");
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_diff_invalid_root_id() {
        let diff = CanonicalGraphDiff {
            root_id: vec![0u8; 5], // invalid length
            changes: vec![NodeChange {
                space_id: make_uuid_bytes(2),
                change_type: ProtoChangeType::Added as i32,
                distance: Some(1),
                parent_edge: None,
            }],
            meta: None,
        };

        let payload = diff.encode_to_vec();
        let parsed = parse_diff(&payload).expect("parse_diff should not fail");
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_diff_multiple_changes() {
        let root_id = make_uuid_bytes(1);

        let diff = CanonicalGraphDiff {
            root_id,
            changes: vec![
                NodeChange {
                    space_id: make_uuid_bytes(2),
                    change_type: ProtoChangeType::Added as i32,
                    distance: Some(1),
                    parent_edge: None,
                },
                NodeChange {
                    space_id: make_uuid_bytes(3),
                    change_type: ProtoChangeType::Moved as i32,
                    distance: Some(2),
                    parent_edge: None,
                },
                NodeChange {
                    space_id: make_uuid_bytes(4),
                    change_type: ProtoChangeType::Removed as i32,
                    distance: None,
                    parent_edge: None,
                },
            ],
            meta: None,
        };

        let payload = diff.encode_to_vec();
        let parsed = parse_diff(&payload)
            .expect("parse_diff should not fail")
            .expect("parsed diff should not be None");

        assert_eq!(parsed.changes.len(), 3);
        assert_eq!(parsed.changes[0].change_type, ChangeType::Added);
        assert_eq!(parsed.changes[1].change_type, ChangeType::Moved);
        assert_eq!(parsed.changes[2].change_type, ChangeType::Removed);
    }
}
