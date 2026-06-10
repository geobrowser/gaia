//! Kafka consumer for the `knowledge.edits` topic.
//!
//! Mirrors the vote-indexer consumer: a manual-commit `StreamConsumer` with the
//! environment topic prefix applied. Edits are decoded in two steps —
//! `HermesEdit` (prost) then the raw GRC2/GRC2Z payload via the `grc-20` crate,
//! the same decoder the kg-indexer uses.

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

/// Base topic for knowledge edits.
const EDITS_TOPIC: &str = "knowledge.edits";

/// Kafka consumer for knowledge edits.
pub struct KafkaConsumer {
    consumer: StreamConsumer<DefaultConsumerContext>,
    topic: String,
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
        let topic = format!("{}{}", prefix, EDITS_TOPIC);

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic_prefix = %prefix,
            topic = %topic,
            "Created Kafka consumer"
        );

        Ok(Self { consumer, topic })
    }

    pub fn subscribe(&self) -> Result<(), IndexerError> {
        self.consumer.subscribe(&[&self.topic])?;
        info!(topic = %self.topic, "Subscribed to Kafka topic");
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
