//! Kafka consumer for the curation.votes topic.

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

/// Base topic for curation votes
const VOTES_TOPIC: &str = "curation.votes";

/// Kafka consumer for vote events.
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

        // Topic prefix for environment isolation (e.g., "staging." or empty for production)
        let prefix = get_topic_prefix();
        let topic = format!("{}{}", prefix, VOTES_TOPIC);

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic_prefix = %prefix,
            topic = %topic,
            "Created Kafka consumer"
        );

        Ok(Self { consumer, topic })
    }

    /// Subscribe to the curation.votes topic (with environment prefix if configured).
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

/// Parse a vote message from Kafka payload.
pub fn parse_vote(
    payload: &[u8],
) -> Result<hermes_schema::pb::voting::HermesVoteCast, IndexerError> {
    let vote = hermes_schema::pb::voting::HermesVoteCast::decode(payload)?;
    Ok(vote)
}
