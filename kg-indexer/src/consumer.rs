use prost::Message;
use rdkafka::{
    config::ClientConfig,
    consumer::{Consumer, DefaultConsumerContext, StreamConsumer},
    message::Headers,
    Offset, TopicPartitionList,
};
use std::env;
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::error::IndexerError;

/// Controls how the consumer handles Kafka offsets on startup.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OffsetResetMode {
    /// Use committed offsets from Kafka (default behavior).
    #[default]
    Stored,
    /// Seek to beginning of all partitions, reprocess everything.
    Earliest,
    /// Seek to end of all partitions, only process new messages.
    Latest,
}

impl FromStr for OffsetResetMode {
    type Err = IndexerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stored" => Ok(OffsetResetMode::Stored),
            "earliest" => Ok(OffsetResetMode::Earliest),
            "latest" => Ok(OffsetResetMode::Latest),
            _ => Err(IndexerError::config(format!(
                "Invalid KAFKA_OFFSET_RESET_MODE '{}'. Valid values: stored, earliest, latest",
                s
            ))),
        }
    }
}

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

        // Topics to consume (subset of what hermes-pipeline produces)
        let topics = vec![
            "knowledge.edits".to_string(),
            "space.creations".to_string(),
            "space.membership".to_string(),
            "space.trust.extensions".to_string(),
            "space.governance".to_string(),
        ];

        info!(
            brokers = %brokers,
            group_id = %group_id,
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

    /// Seek all assigned partitions to the beginning.
    /// Must be called after subscribe() and after partition assignment.
    pub async fn seek_to_beginning(&self) -> Result<(), IndexerError> {
        warn!("KAFKA_OFFSET_RESET_MODE=earliest: Seeking to beginning of all partitions");

        // Wait for partition assignment with timeout
        let assignment = self.wait_for_assignment().await?;

        for elem in assignment.elements() {
            self.consumer
                .seek(elem.topic(), elem.partition(), Offset::Beginning, None)
                .map_err(|e| {
                    IndexerError::kafka(format!(
                        "Failed to seek {} partition {} to beginning: {}",
                        elem.topic(),
                        elem.partition(),
                        e
                    ))
                })?;

            info!(
                topic = %elem.topic(),
                partition = elem.partition(),
                "Seeked to beginning"
            );
        }

        Ok(())
    }

    /// Seek all assigned partitions to the end.
    /// Must be called after subscribe() and after partition assignment.
    pub async fn seek_to_end(&self) -> Result<(), IndexerError> {
        warn!("KAFKA_OFFSET_RESET_MODE=latest: Seeking to end of all partitions");

        // Wait for partition assignment with timeout
        let assignment = self.wait_for_assignment().await?;

        for elem in assignment.elements() {
            self.consumer
                .seek(elem.topic(), elem.partition(), Offset::End, None)
                .map_err(|e| {
                    IndexerError::kafka(format!(
                        "Failed to seek {} partition {} to end: {}",
                        elem.topic(),
                        elem.partition(),
                        e
                    ))
                })?;

            info!(
                topic = %elem.topic(),
                partition = elem.partition(),
                "Seeked to end"
            );
        }

        Ok(())
    }

    /// Wait for partition assignment after subscribe.
    /// This is async since StreamConsumer requires async polling.
    pub async fn wait_for_assignment(&self) -> Result<TopicPartitionList, IndexerError> {
        let timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();

        loop {
            // Check if we have assignments yet
            let assignment = self
                .consumer
                .assignment()
                .map_err(|e| IndexerError::kafka(format!("Failed to get assignment: {}", e)))?;

            if !assignment.elements().is_empty() {
                info!(
                    partitions = assignment.count(),
                    "Partition assignment received"
                );
                return Ok(assignment);
            }

            if start.elapsed() > timeout {
                return Err(IndexerError::kafka(
                    "Timeout waiting for partition assignment".to_string(),
                ));
            }

            // Poll with timeout to trigger rebalance
            // Using tokio timeout + recv to drive the consumer
            match tokio::time::timeout(Duration::from_millis(100), self.consumer.recv()).await {
                Ok(Ok(_msg)) => {
                    // Got a message, but we're just trying to get assignments
                    // The message will be lost, but that's ok since we're seeking anyway
                }
                Ok(Err(_)) | Err(_) => {
                    // Timeout or kafka error, continue polling
                }
            }
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
    Edit(hermes_schema::pb::knowledge::HermesEdit),
    CreateSpace(hermes_schema::pb::space::HermesCreateSpace),
    RoleGranted(hermes_schema::pb::membership::HermesRoleGranted),
    RoleRevoked(hermes_schema::pb::membership::HermesRoleRevoked),
    TrustExtension(hermes_schema::pb::space::HermesSpaceTrustExtension),
    ProposalCreated(hermes_schema::pb::governance::HermesProposalCreated),
    ProposalVoted(hermes_schema::pb::governance::HermesProposalVoted),
    ProposalExecuted(hermes_schema::pb::governance::HermesProposalExecuted),
}

impl KgMessage {
    /// Get the blockchain metadata from the message.
    pub fn meta(&self) -> Option<&BlockchainMetadata> {
        match self {
            KgMessage::Edit(e) => e.meta.as_ref(),
            KgMessage::CreateSpace(s) => s.meta.as_ref(),
            KgMessage::RoleGranted(r) => r.meta.as_ref(),
            KgMessage::RoleRevoked(r) => r.meta.as_ref(),
            KgMessage::TrustExtension(t) => t.meta.as_ref(),
            KgMessage::ProposalCreated(p) => p.meta.as_ref(),
            KgMessage::ProposalVoted(v) => v.meta.as_ref(),
            KgMessage::ProposalExecuted(e) => e.meta.as_ref(),
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
    match topic {
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
                _ => Err(IndexerError::decode(format!(
                    "unknown governance event type: {:?}",
                    event_type
                ))),
            }
        }
        _ => Err(IndexerError::decode(format!("unknown topic: {}", topic))),
    }
}
