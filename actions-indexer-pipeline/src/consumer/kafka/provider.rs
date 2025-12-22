//! Kafka stream provider for consuming vote events from Hermes.
//!
//! This module provides a Kafka-based implementation of the `ConsumeActionsStream` trait,
//! enabling the actions indexer to consume vote events from the Hermes Kafka stream
//! instead of directly from substreams.

use async_trait::async_trait;
use hermes_kafka::{Consumer, StreamConsumer};
use tokio::sync::mpsc;

use crate::consumer::{ConsumeActionsStream, StreamMessage};
use crate::errors::{ConsumerError, KafkaError, StreamError};

use super::ConsumerConfig;

/// Kafka stream provider for consuming action events from Hermes.
///
/// This provider connects to a Kafka topic (e.g., `curation.votes`) and consumes
/// `HermesVoteCast` protobuf messages, converting them to `ActionRaw` for processing
/// by the existing pipeline.
pub struct KafkaStreamProvider {
    config: ConsumerConfig,
}

impl KafkaStreamProvider {
    /// Creates a new `KafkaStreamProvider` with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Kafka consumer configuration including broker, group_id, and topic
    ///
    /// # Returns
    ///
    /// A new `KafkaStreamProvider` instance.
    pub fn new(config: ConsumerConfig) -> Self {
        Self { config }
    }

    /// Creates and subscribes a Kafka consumer to the configured topic.
    fn create_subscribed_consumer(&self) -> Result<StreamConsumer, ConsumerError> {
        let consumer = self
            .config
            .create_consumer()
            .map_err(|e| KafkaError::Connection(e.to_string()))?;

        consumer
            .subscribe(&[&self.config.topic])
            .map_err(|e| KafkaError::Subscription(e.to_string()))?;

        Ok(consumer)
    }
}

#[async_trait]
impl ConsumeActionsStream for KafkaStreamProvider {
    /// Streams action events from Kafka through a channel.
    ///
    /// This method:
    /// 1. Creates a Kafka consumer and subscribes to the configured topic
    /// 2. Polls for messages in a loop
    /// 3. Decodes `HermesVoteCast` protobuf messages (TODO: Task 4)
    /// 4. Converts to `ActionRaw` and sends through the channel (TODO: Task 5)
    ///
    /// # Arguments
    ///
    /// * `sender` - Channel sender for streaming messages to the orchestrator
    /// * `_cursor` - Ignored for Kafka (offset tracking is handled by consumer groups)
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `ConsumerError` if streaming fails.
    async fn stream_events(
        &self,
        sender: mpsc::Sender<StreamMessage>,
        _cursor: Option<String>,
    ) -> Result<(), ConsumerError> {
        let consumer = self.create_subscribed_consumer()?;

        println!(
            "KafkaStreamProvider: Connected to Kafka broker at {}",
            self.config.broker
        );
        println!(
            "KafkaStreamProvider: Subscribed to topic '{}'",
            self.config.topic
        );
        println!(
            "KafkaStreamProvider: Consumer group '{}'",
            self.config.group_id
        );

        // TODO (Task 5): Implement the actual consumption loop
        // For now, this is a skeleton that just sends StreamEnd
        // The full implementation will:
        // 1. Poll messages from Kafka
        // 2. Decode HermesVoteCast protobuf (Task 4)
        // 3. Convert to ActionRaw (Task 4)
        // 4. Send BlockData messages through the channel
        // 5. Commit offsets after successful processing (Task 6)

        // Keep consumer alive to prevent immediate drop
        drop(consumer);

        sender
            .send(StreamMessage::StreamEnd)
            .await
            .map_err(|e| StreamError::ChannelSend(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_kafka_stream_provider_new() {
        let config = ConsumerConfig::new(
            Url::parse("localhost:9092").unwrap(),
            "test-group",
            "test-topic",
        );
        let provider = KafkaStreamProvider::new(config);

        assert_eq!(
            provider.config.broker,
            Url::parse("localhost:9092").unwrap()
        );
        assert_eq!(provider.config.group_id, "test-group");
        assert_eq!(provider.config.topic, "test-topic");
    }

    #[test]
    fn test_kafka_stream_provider_with_credentials() {
        let config = ConsumerConfig::new(
            Url::parse("localhost:9092").unwrap(),
            "actions-indexer",
            "curation.votes",
        )
        .with_credentials("user".to_string(), "pass".to_string());
        let provider = KafkaStreamProvider::new(config);
        assert_eq!(provider.config.username, Some("user".to_string()));
        assert_eq!(provider.config.password, Some("pass".to_string()));
    }

    #[test]
    fn test_kafka_stream_provider_with_ssl_ca() {
        let config = ConsumerConfig::new(
            Url::parse("localhost:9092").unwrap(),
            "actions-indexer",
            "curation.votes",
        )
        .with_ssl_ca("-----BEGIN CERTIFICATE-----".to_string());
        let provider = KafkaStreamProvider::new(config);
        assert_eq!(
            provider.config.ssl_ca_pem,
            Some("-----BEGIN CERTIFICATE-----".to_string())
        );
    }
}
