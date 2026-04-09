//! Kafka consumers for the space.governance and knowledge.edits topics.

use hermes_kafka::get_topic_prefix;
use prost::Message;
use rdkafka::{
    config::ClientConfig,
    consumer::{Consumer, DefaultConsumerContext, StreamConsumer},
    message::Headers,
    TopicPartitionList,
};
use std::env;
use tracing::{debug, error, info};

use crate::error::IndexerError;

/// Base topic for governance events
const GOVERNANCE_TOPIC: &str = "space.governance";

/// Base topic for knowledge edit events
const KNOWLEDGE_EDITS_TOPIC: &str = "knowledge.edits";

/// Kafka consumer for governance events.
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
            .set("session.timeout.ms", "6000")
            // Memory bounds: limit fetch and message sizes to prevent OOM
            .set("fetch.max.bytes", "52428800") // 50MB total fetch
            .set("max.partition.fetch.bytes", "1048576") // 1MB per partition
            .set("message.max.bytes", "10485760") // 10MB max message
            .set("max.poll.interval.ms", "300000"); // 5min max processing time

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

        // Topic prefix for environment isolation
        let prefix = get_topic_prefix();
        let topic = format!("{}{}", prefix, GOVERNANCE_TOPIC);

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic_prefix = %prefix,
            topic = %topic,
            "Created Kafka consumer"
        );

        Ok(Self { consumer, topic })
    }

    /// Subscribe to the governance topic.
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

    /// Flush any pending async offset commits synchronously.
    ///
    /// Called during graceful shutdown to ensure all successfully processed
    /// offsets are persisted before the consumer is dropped.
    pub fn flush_commits(&self) {
        if let Err(e) = self
            .consumer
            .commit_consumer_state(rdkafka::consumer::CommitMode::Sync)
        {
            error!(error = %e, "Failed to flush commits during shutdown");
        }
    }
}

/// Get the event-type header value from Kafka headers.
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

/// Parse a PROPOSAL_CREATED message from Kafka payload.
pub fn parse_proposal_created(
    payload: &[u8],
) -> Result<hermes_schema::pb::governance::HermesProposalCreated, IndexerError> {
    let msg = hermes_schema::pb::governance::HermesProposalCreated::decode(payload)?;
    Ok(msg)
}

/// Parse a PROPOSAL_UPDATED message from Kafka payload.
pub fn parse_proposal_updated(
    payload: &[u8],
) -> Result<hermes_schema::pb::governance::HermesProposalUpdated, IndexerError> {
    let msg = hermes_schema::pb::governance::HermesProposalUpdated::decode(payload)?;
    Ok(msg)
}

/// Parse a PROPOSAL_VOTED message from Kafka payload.
pub fn parse_proposal_voted(
    payload: &[u8],
) -> Result<hermes_schema::pb::governance::HermesProposalVoted, IndexerError> {
    let msg = hermes_schema::pb::governance::HermesProposalVoted::decode(payload)?;
    Ok(msg)
}

/// Parse a PROPOSAL_EXECUTED message from Kafka payload.
pub fn parse_proposal_executed(
    payload: &[u8],
) -> Result<hermes_schema::pb::governance::HermesProposalExecuted, IndexerError> {
    let msg = hermes_schema::pb::governance::HermesProposalExecuted::decode(payload)?;
    Ok(msg)
}

/// Parse a PROPOSAL_SETTINGS_UPDATED message from Kafka payload.
pub fn parse_proposal_settings_updated(
    payload: &[u8],
) -> Result<hermes_schema::pb::governance::HermesProposalSettingsUpdated, IndexerError> {
    let msg = hermes_schema::pb::governance::HermesProposalSettingsUpdated::decode(payload)?;
    Ok(msg)
}

/// Parse a HermesEdit message from knowledge.edits Kafka payload.
pub fn parse_hermes_edit(
    payload: &[u8],
) -> Result<hermes_schema::pb::knowledge::HermesEdit, IndexerError> {
    let msg = hermes_schema::pb::knowledge::HermesEdit::decode(payload)?;
    Ok(msg)
}

/// Kafka consumer for knowledge edit events.
///
/// Follows the same manual-commit pattern as `KafkaConsumer` but subscribes
/// to the `knowledge.edits` topic.
pub struct KnowledgeEditsConsumer {
    consumer: StreamConsumer<DefaultConsumerContext>,
    topic: String,
}

impl KnowledgeEditsConsumer {
    /// Create a new knowledge edits consumer.
    ///
    /// Uses a separate consumer group (configured via KAFKA_GROUP_ID_KNOWLEDGE_EDITS)
    /// to avoid conflicting with the governance consumer's offsets.
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IndexerError> {
        let mut config = ClientConfig::new();
        config
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .set("session.timeout.ms", "6000")
            .set("fetch.max.bytes", "52428800")
            .set("max.partition.fetch.bytes", "1048576")
            .set("message.max.bytes", "10485760")
            .set("max.poll.interval.ms", "300000");

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
        let topic = format!("{}{}", prefix, KNOWLEDGE_EDITS_TOPIC);

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic = %topic,
            "Created knowledge edits consumer"
        );

        Ok(Self { consumer, topic })
    }

    /// Subscribe to the knowledge.edits topic.
    pub fn subscribe(&self) -> Result<(), IndexerError> {
        self.consumer.subscribe(&[&self.topic])?;
        info!(topic = %self.topic, "Subscribed to knowledge.edits topic");
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

        debug!(topic = %topic, partition = partition, offset = offset, "Committed knowledge edits offset");
        Ok(())
    }

    /// Flush any pending async offset commits synchronously.
    pub fn flush_commits(&self) {
        if let Err(e) = self
            .consumer
            .commit_consumer_state(rdkafka::consumer::CommitMode::Sync)
        {
            error!(error = %e, "Failed to flush knowledge edits commits during shutdown");
        }
    }
}
