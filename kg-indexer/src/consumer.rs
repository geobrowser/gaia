use prost::Message;
use rdkafka::{
    TopicPartitionList,
    config::ClientConfig,
    consumer::{Consumer, DefaultConsumerContext, StreamConsumer},
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

        // Topics to consume
        let topics = vec![
            "knowledge.edits".to_string(),
            "space.creations".to_string(),
            "space.membership".to_string(),
            "space.trust.extensions".to_string(),
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

/// Represents a message from any of the consumed topics
pub enum KgMessage {
    Edit(hermes_schema::pb::knowledge::HermesEdit),
    CreateSpace(hermes_schema::pb::space::HermesCreateSpace),
    RoleGranted(hermes_schema::pb::membership::HermesRoleGranted),
    RoleRevoked(hermes_schema::pb::membership::HermesRoleRevoked),
    TrustExtension(hermes_schema::pb::space::HermesSpaceTrustExtension),
}

/// Parse a Kafka message based on its topic
pub fn parse_message(
    topic: &str,
    payload: &[u8],
) -> Result<KgMessage, IndexerError> {
    match topic {
        "knowledge.edits" => {
            let edit = hermes_schema::pb::knowledge::HermesEdit::decode(payload)
                .map_err(|e| IndexerError::parse(format!("Failed to decode HermesEdit: {}", e)))?;
            Ok(KgMessage::Edit(edit))
        }
        "space.creations" => {
            let space = hermes_schema::pb::space::HermesCreateSpace::decode(payload)
                .map_err(|e| IndexerError::parse(format!("Failed to decode HermesCreateSpace: {}", e)))?;
            Ok(KgMessage::CreateSpace(space))
        }
        "space.membership" => {
            // Try to decode as RoleGranted first, then RoleRevoked
            if let Ok(granted) = hermes_schema::pb::membership::HermesRoleGranted::decode(payload) {
                return Ok(KgMessage::RoleGranted(granted));
            }
            if let Ok(revoked) = hermes_schema::pb::membership::HermesRoleRevoked::decode(payload) {
                return Ok(KgMessage::RoleRevoked(revoked));
            }
            Err(IndexerError::parse("Failed to decode membership message"))
        }
        "space.trust.extensions" => {
            let extension = hermes_schema::pb::space::HermesSpaceTrustExtension::decode(payload)
                .map_err(|e| IndexerError::parse(format!("Failed to decode HermesSpaceTrustExtension: {}", e)))?;
            Ok(KgMessage::TrustExtension(extension))
        }
        _ => Err(IndexerError::parse(format!("Unknown topic: {}", topic))),
    }
}
