//! Kafka stream provider for consuming vote events from Hermes.
//!
//! This module provides a Kafka-based implementation of the `ConsumeActionsStream` trait,
//! enabling the actions indexer to consume vote events from the Hermes Kafka stream
//! instead of directly from substreams.
//!
//! ## Error Handling Strategy
//!
//! - **Transient errors** (Kafka connection, network issues): Retry with exponential backoff
//! - **Permanent errors** (malformed data, invalid protobuf): Log and skip the message
//! - **Channel errors**: Fatal, propagate immediately

use async_trait::async_trait;
use futures03::StreamExt;
use hermes_kafka::{Consumer, Message, StreamConsumer};
use crate::errors::{ConversionError, StreamError};
use hermes_schema::pb::voting::HermesVoteCast;
use prost::Message as ProstMessage;
use std::time::Duration;
use tokio::sync::mpsc;
use crate::errors::KafkaError;

use crate::consumer::{BlockDataMessage, ConsumeActionsStream, StreamMessage};
use crate::errors::ConsumerError;

use super::conversion::hermes_vote_to_action_raw;
use super::ConsumerConfig;

/// Maximum number of consecutive transient errors before giving up
const MAX_CONSECUTIVE_ERRORS: u32 = 10;

/// Initial backoff delay for retries
const INITIAL_BACKOFF_MS: u64 = 100;

/// Maximum backoff delay for retries
const MAX_BACKOFF_MS: u64 = 30_000;

/// Categorizes whether an error is transient (can retry) or permanent (should skip).
#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    /// Transient error - retry with backoff (e.g., network issues, broker unavailable)
    Transient,
    /// Permanent error - skip and log (e.g., malformed data, invalid protobuf)
    Permanent,
}

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

    /// Categorizes a Kafka error as transient or permanent.
    ///
    /// All Kafka-level errors (network, broker, timeout) are treated as transient
    /// because the consumer can retry and Kafka is designed for resilience.
    /// Permanent errors come from message content (invalid protobuf, bad data),
    /// which are handled separately in the message processing logic.
    #[allow(unused)]
    fn categorize_kafka_error(_error: &rdkafka::error::KafkaError) -> ErrorCategory {
        // All Kafka-level errors are transient - the consumer can retry
        ErrorCategory::Transient
    }

    /// Categorizes a consumer error as transient or permanent.
    ///
    /// This function explicitly handles all error variants to ensure
    /// new errors are properly categorized at compile time.
    fn categorize_consumer_error(error: &ConsumerError) -> ErrorCategory {
        match error {
            ConsumerError::Config(_) => ErrorCategory::Permanent,
            ConsumerError::Stream(_) => ErrorCategory::Permanent,
            ConsumerError::Conversion(_) => ErrorCategory::Permanent,
            ConsumerError::Kafka(_) => ErrorCategory::Transient,
        }
    }

    /// Calculates the backoff duration for a retry attempt using exponential backoff.
    fn calculate_backoff(attempt: u32) -> Duration {
        let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow(attempt.min(10));
        Duration::from_millis(backoff_ms.min(MAX_BACKOFF_MS))
    }
}

#[async_trait]
impl ConsumeActionsStream for KafkaStreamProvider {
    /// Streams action events from Kafka through a channel.
    ///
    /// This method:
    /// 1. Creates a Kafka consumer and subscribes to the configured topic
    /// 2. Polls for messages in a loop using async stream
    /// 3. Decodes `HermesVoteCast` protobuf messages from message payload
    /// 4. Converts to `ActionRaw` and sends through the channel as `BlockData`
    /// 5. Commits offsets after successful channel send (at-least-once delivery)
    ///
    /// ## Error Handling
    ///
    /// - **Transient errors**: Retried with exponential backoff, resets on success
    /// - **Permanent errors**: Logged, skipped, and offset committed to prevent redelivery
    /// - **Channel errors**: Fatal, immediately terminate the stream
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
        
        let now = chrono::Utc::now();
        println!("{} - KafkaStreamProvider: Connected to broker at {}", now.to_rfc3339(), self.config.broker);
        println!("{} - KafkaStreamProvider: Subscribed to topic '{}'", now.to_rfc3339(), self.config.topic);
        println!("{} - KafkaStreamProvider: Consumer group '{}'", now.to_rfc3339(), self.config.group_id);

        // Create the message stream from the consumer
        let mut message_stream = consumer.stream();
        
        // Track consecutive transient errors for backoff
        let mut consecutive_errors: u32 = 0;
        let mut messages_processed: u64 = 0;
        let mut messages_skipped: u64 = 0;
        
        // Consumption loop
        while let Some(message_result) = message_stream.next().await {
            match message_result {
                Ok(borrowed_message) => {
                    // Reset consecutive error count on successful message receipt
                    consecutive_errors = 0;
                    
                    // Get the message payload
                    let payload = match borrowed_message.payload() {
                        Some(payload) => payload,
                        None => {
                            // Empty payload - permanent error, skip and commit
                            eprintln!(
                                "KafkaStreamProvider: Empty payload at partition {} offset {}, skipping",
                                borrowed_message.partition(),
                                borrowed_message.offset()
                            );
                            messages_skipped += 1;
                            // Commit to prevent redelivery of empty message
                            let _ = consumer.commit_message(&borrowed_message, rdkafka::consumer::CommitMode::Async);
                            continue;
                        }
                    };

                    // Decode the HermesVoteCast protobuf message
                    let vote_cast = match HermesVoteCast::decode(payload) {
                        Ok(vote) => vote,
                        Err(e) => {
                            // Permanent error - malformed protobuf, skip and commit
                            eprintln!(
                                "KafkaStreamProvider: Failed to decode HermesVoteCast at partition {} offset {}: {}. Skipping.",
                                borrowed_message.partition(),
                                borrowed_message.offset(),
                                e
                            );
                            messages_skipped += 1;
                            // Send error notification but don't fail
                            let _ = sender.send(StreamMessage::Error(
                                ConsumerError::Conversion(ConversionError::InvalidDataField(format!(
                                    "protobuf decode error at offset {}: {}",
                                    borrowed_message.offset(),
                                    e
                                )))
                            )).await;
                            // Commit to prevent redelivery
                            let _ = consumer.commit_message(&borrowed_message, rdkafka::consumer::CommitMode::Async);
                            continue;
                        }
                    };

                    // Convert to ActionRaw
                    let action_raw = match hermes_vote_to_action_raw(&vote_cast) {
                        Ok(action) => action,
                        Err(e) => {
                            let error_category = Self::categorize_consumer_error(&e);
                            
                            if error_category == ErrorCategory::Permanent {
                                // Permanent error - skip and commit
                                eprintln!(
                                    "KafkaStreamProvider: Permanent conversion error at partition {} offset {}: {}. Skipping.",
                                    borrowed_message.partition(),
                                    borrowed_message.offset(),
                                    e
                                );
                                messages_skipped += 1;
                                let _ = sender.send(StreamMessage::Error(e)).await;
                                // Commit to prevent redelivery
                                let _ = consumer.commit_message(&borrowed_message, rdkafka::consumer::CommitMode::Async);
                            } else {
                                // Transient error - don't commit, will be retried
                                eprintln!(
                                    "KafkaStreamProvider: Transient conversion error at partition {} offset {}: {}",
                                    borrowed_message.partition(),
                                    borrowed_message.offset(),
                                    e
                                );
                                let _ = sender.send(StreamMessage::Error(e)).await;
                            }
                            continue;
                        }
                    };

                    // Extract cursor and block number from the vote metadata
                    let (cursor, block_number) = match &vote_cast.meta {
                        Some(meta) => (meta.cursor.clone(), meta.block_number as i64),
                        None => {
                            // Use Kafka offset as fallback cursor
                            let offset_cursor = format!(
                                "{}:{}:{}",
                                borrowed_message.topic(),
                                borrowed_message.partition(),
                                borrowed_message.offset()
                            );
                            (offset_cursor, 0)
                        }
                    };

                    // Send the action through the channel
                    sender.send(StreamMessage::BlockData(BlockDataMessage {
                        actions: vec![action_raw],
                        cursor,
                        block_number,
                    }))
                    .await
                    .map_err(|e| ConsumerError::Stream(StreamError::ChannelSend(e.to_string())))?;

                    // Commit the offset after successful send (at-least-once delivery)
                    if let Err(e) = consumer.commit_message(&borrowed_message, rdkafka::consumer::CommitMode::Async) {
                        eprintln!(
                            "KafkaStreamProvider: Failed to commit offset at partition {} offset {}: {}",
                            borrowed_message.partition(),
                            borrowed_message.offset(),
                            e
                        );
                        // Continue processing - worst case is duplicate on restart
                    }
                    
                    messages_processed += 1;
                    
                    // Log progress periodically
                    if messages_processed % 1000 == 0 {
                        let now = chrono::Utc::now();
                        println!(
                            "{} - KafkaStreamProvider: Processed {} messages, skipped {}",
                            now.to_rfc3339(),
                            messages_processed,
                            messages_skipped
                        );
                    }
                }
                Err(e) => {
                    let error_category = Self::categorize_kafka_error(&e);
                    consecutive_errors += 1;
                    
                    eprintln!(
                        "KafkaStreamProvider: Kafka consume error (attempt {}/{}): {}",
                        consecutive_errors,
                        MAX_CONSECUTIVE_ERRORS,
                        e
                    );
                    
                    // Send error notification
                    let _ = sender.send(StreamMessage::Error(
                        ConsumerError::Kafka(KafkaError::Consume(e.to_string()))
                    )).await;
                    
                    if error_category == ErrorCategory::Transient {
                        if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                            eprintln!(
                                "KafkaStreamProvider: Max consecutive errors ({}) reached, terminating",
                                MAX_CONSECUTIVE_ERRORS
                            );
                            return Err(ConsumerError::Kafka(KafkaError::Consume(format!(
                                "Max consecutive errors reached: {}",
                                e
                            ))))?;
                        }
                        
                        // Apply exponential backoff before next poll
                        let backoff = Self::calculate_backoff(consecutive_errors);
                        eprintln!(
                            "KafkaStreamProvider: Applying backoff of {:?} before retry",
                            backoff
                        );
                        tokio::time::sleep(backoff).await;
                    }
                    // Continue the loop to retry
                }
            }
        }

        // Stream ended (consumer was closed or disconnected)
        let now = chrono::Utc::now();
        println!(
            "{} - KafkaStreamProvider: Stream ended. Total processed: {}, skipped: {}",
            now.to_rfc3339(),
            messages_processed,
            messages_skipped
        );
        
        sender.send(StreamMessage::StreamEnd)
            .await
            .map_err(|e| ConsumerError::Stream(StreamError::ChannelSend(e.to_string())))?;

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
