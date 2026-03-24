use hermes_kafka::{get_topic_prefix, strip_topic_prefix};
use prost::Message;
use rdkafka::{
    config::ClientConfig,
    consumer::{Consumer, DefaultConsumerContext, StreamConsumer},
    message::Headers,
    TopicPartitionList,
};
use std::env;
use tracing::{debug, info};

use crate::error::IndexerError;

pub struct KafkaConsumer {
    consumer: StreamConsumer<DefaultConsumerContext>,
    topics: Vec<String>,
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

        // Optional SASL/SSL configuration
        if let Ok(username) = env::var("KAFKA_USERNAME") {
            config.set("security.protocol", "SASL_SSL");
            config.set("sasl.mechanisms", "PLAIN");
            config.set("sasl.username", &username);

            if let Ok(password) = env::var("KAFKA_PASSWORD") {
                config.set("sasl.password", &password);
            }
        }

        // Optional custom CA certificate
        if let Ok(ca_pem) = env::var("KAFKA_SSL_CA_PEM") {
            config.set("ssl.ca.pem", &ca_pem);
        }

        let consumer: StreamConsumer = config
            .create()
            .map_err(|e| IndexerError::kafka(e.to_string()))?;

        // Topic prefix for environment isolation (e.g., "staging." or empty for production)
        let prefix = get_topic_prefix();

        // Topics to consume (subset of what hermes-pipeline produces)
        let topics = vec![
            format!("{}hermes.blocks", prefix),
            format!("{}knowledge.edits", prefix),
            format!("{}space.creations", prefix),
            format!("{}space.membership", prefix),
            format!("{}space.trust.extensions", prefix),
            format!("{}space.topics", prefix),
            format!("{}space.governance", prefix),
        ];

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic_prefix = %prefix,
            topics = ?topics,
            "Created Kafka consumer"
        );

        Ok(Self { consumer, topics })
    }

    pub fn subscribe(&self) -> Result<(), IndexerError> {
        let topics: Vec<&str> = self.topics.iter().map(|s| s.as_str()).collect();
        self.consumer
            .subscribe(&topics)
            .map_err(|e| IndexerError::kafka(e.to_string()))?;

        info!(topics = ?self.topics, "Subscribed to Kafka topics");
        Ok(())
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
        tpl.add_partition_offset(topic, partition, rdkafka::Offset::Offset(offset + 1))
            .map_err(|e| IndexerError::kafka(e.to_string()))?;

        self.consumer
            .commit(&tpl, rdkafka::consumer::CommitMode::Async)
            .map_err(|e| IndexerError::kafka(e.to_string()))?;

        debug!(topic = %topic, partition = partition, offset = offset, "Committed offset");
        Ok(())
    }
}

use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;

/// Represents a message from any of the consumed topics
pub enum KgMessage {
    BlockSummary(hermes_schema::pb::block_summary::HermesBlockSummary),
    Edit(hermes_schema::pb::knowledge::HermesEdit),
    CreateSpace(hermes_schema::pb::space::HermesCreateSpace),
    RoleGranted(hermes_schema::pb::membership::HermesRoleGranted),
    RoleRevoked(hermes_schema::pb::membership::HermesRoleRevoked),
    TrustExtension(hermes_schema::pb::space::HermesSpaceTrustExtension),
    TopicDeclared(hermes_schema::pb::topics::HermesTopicDeclared),
    ProposalCreated(hermes_schema::pb::governance::HermesProposalCreated),
    ProposalUpdated(hermes_schema::pb::governance::HermesProposalUpdated),
    ProposalVoted(hermes_schema::pb::governance::HermesProposalVoted),
    ProposalExecuted(hermes_schema::pb::governance::HermesProposalExecuted),
    ProposalSettingsUpdated(hermes_schema::pb::governance::HermesProposalSettingsUpdated),
}

impl KgMessage {
    /// Get the blockchain metadata from the message.
    pub fn meta(&self) -> Option<&BlockchainMetadata> {
        match self {
            KgMessage::BlockSummary(_) => None,
            KgMessage::Edit(e) => e.meta.as_ref(),
            KgMessage::CreateSpace(s) => s.meta.as_ref(),
            KgMessage::RoleGranted(r) => r.meta.as_ref(),
            KgMessage::RoleRevoked(r) => r.meta.as_ref(),
            KgMessage::TrustExtension(t) => t.meta.as_ref(),
            KgMessage::TopicDeclared(t) => t.meta.as_ref(),
            KgMessage::ProposalCreated(p) => p.meta.as_ref(),
            KgMessage::ProposalUpdated(p) => p.meta.as_ref(),
            KgMessage::ProposalVoted(v) => v.meta.as_ref(),
            KgMessage::ProposalExecuted(e) => e.meta.as_ref(),
            KgMessage::ProposalSettingsUpdated(s) => s.meta.as_ref(),
        }
    }

    /// Get the block number from the message metadata.
    pub fn block_number(&self) -> Option<u64> {
        self.meta().map(|m| m.block_number)
    }

    /// Get the sequence number from the message metadata.
    pub fn sequence(&self) -> u32 {
        self.meta().map(|m| m.sequence).unwrap_or(0)
    }

    /// Check if this is the last message in the block.
    pub fn is_last(&self) -> bool {
        self.meta().map(|m| m.is_last).unwrap_or(false)
    }
}

/// Get the event-type header value from Kafka headers
pub fn get_event_type(headers: Option<&rdkafka::message::BorrowedHeaders>) -> Option<String> {
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

/// Parse a Kafka message based on its topic and headers
pub fn parse_message(
    topic: &str,
    payload: &[u8],
    event_type: Option<&str>,
) -> Result<KgMessage, IndexerError> {
    match strip_topic_prefix(topic) {
        "hermes.blocks" => {
            let summary = hermes_schema::pb::block_summary::HermesBlockSummary::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesBlockSummary: {}", e)))?;
            Ok(KgMessage::BlockSummary(summary))
        }
        "knowledge.edits" => {
            let edit = hermes_schema::pb::knowledge::HermesEdit::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesEdit: {}", e)))?;
            Ok(KgMessage::Edit(edit))
        }
        "space.creations" => {
            let space = hermes_schema::pb::space::HermesCreateSpace::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesCreateSpace: {}", e)))?;
            Ok(KgMessage::CreateSpace(space))
        }
        "space.membership" => {
            // Use event-type header to determine message type
            match event_type {
                Some("ROLE_GRANTED") => {
                    let granted = hermes_schema::pb::membership::HermesRoleGranted::decode(payload)
                        .map_err(|e| IndexerError::decode(format!("HermesRoleGranted: {}", e)))?;
                    Ok(KgMessage::RoleGranted(granted))
                }
                Some("ROLE_REVOKED") => {
                    let revoked = hermes_schema::pb::membership::HermesRoleRevoked::decode(payload)
                        .map_err(|e| IndexerError::decode(format!("HermesRoleRevoked: {}", e)))?;
                    Ok(KgMessage::RoleRevoked(revoked))
                }
                _ => Err(IndexerError::decode(format!(
                    "unknown membership event type: {:?}",
                    event_type
                ))),
            }
        }
        "space.trust.extensions" => {
            let extension = hermes_schema::pb::space::HermesSpaceTrustExtension::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesSpaceTrustExtension: {}", e)))?;
            Ok(KgMessage::TrustExtension(extension))
        }
        "space.topics" => {
            let declared = hermes_schema::pb::topics::HermesTopicDeclared::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesTopicDeclared: {}", e)))?;
            Ok(KgMessage::TopicDeclared(declared))
        }
        "space.governance" => {
            // Use event-type header to determine message type
            match event_type {
                Some("PROPOSAL_CREATED") => {
                    let created =
                        hermes_schema::pb::governance::HermesProposalCreated::decode(payload)
                            .map_err(|e| {
                                IndexerError::decode(format!("HermesProposalCreated: {}", e))
                            })?;
                    Ok(KgMessage::ProposalCreated(created))
                }
                Some("PROPOSAL_UPDATED") => {
                    let updated =
                        hermes_schema::pb::governance::HermesProposalUpdated::decode(payload)
                            .map_err(|e| {
                                IndexerError::decode(format!("HermesProposalUpdated: {}", e))
                            })?;
                    Ok(KgMessage::ProposalUpdated(updated))
                }
                Some("PROPOSAL_VOTED") => {
                    let voted = hermes_schema::pb::governance::HermesProposalVoted::decode(payload)
                        .map_err(|e| IndexerError::decode(format!("HermesProposalVoted: {}", e)))?;
                    Ok(KgMessage::ProposalVoted(voted))
                }
                Some("PROPOSAL_EXECUTED") => {
                    let executed =
                        hermes_schema::pb::governance::HermesProposalExecuted::decode(payload)
                            .map_err(|e| {
                                IndexerError::decode(format!("HermesProposalExecuted: {}", e))
                            })?;
                    Ok(KgMessage::ProposalExecuted(executed))
                }
                Some("PROPOSAL_SETTINGS_UPDATED") => {
                    let settings_updated =
                        hermes_schema::pb::governance::HermesProposalSettingsUpdated::decode(
                            payload,
                        )
                        .map_err(|e| {
                            IndexerError::decode(format!("HermesProposalSettingsUpdated: {}", e))
                        })?;
                    Ok(KgMessage::ProposalSettingsUpdated(settings_updated))
                }
                _ => Err(IndexerError::decode(format!(
                    "unknown governance event type: {:?}",
                    event_type
                ))),
            }
        }
        _ => Err(IndexerError::decode(format!("unknown topic: {}", topic))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_space_topics_message() {
        let msg = hermes_schema::pb::topics::HermesTopicDeclared {
            space_id: vec![1; 16],
            topic_id: vec![2; 16],
            meta: None,
        };
        let payload = msg.encode_to_vec();

        let parsed = parse_message("space.topics", &payload, Some("TOPIC_DECLARED")).unwrap();

        match parsed {
            KgMessage::TopicDeclared(event) => {
                assert_eq!(event.space_id, vec![1; 16]);
                assert_eq!(event.topic_id, vec![2; 16]);
            }
            _ => panic!("expected TopicDeclared"),
        }
    }
}
