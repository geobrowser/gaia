//! Kafka consumer for space topic events from the space.topics topic.
//!
//! Consumes HermesTopicDeclared messages and forwards space topic events to the ingest.

use hermes_instrumentation::{Instrument, debug, error, info, info_span, instrument, warn};
use hermes_kafka::get_topic_prefix;
use prost::Message;
use rdkafka::{
    TopicPartitionList,
    consumer::{Consumer, StreamConsumer},
    message::Message as KafkaMessage,
};
use std::env;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::consumer::messages::{SpaceTopicEvent, StreamMessage};
use crate::errors::IngestError;
use crate::orchestrator::SpaceTopicProcessingBatch;

use hermes_schema::pb::topics::HermesTopicDeclared;

/// Pending space topic messages for batching.
struct PendingSpaceTopicMessage {
    events: Vec<SpaceTopicEvent>,
}

/// Kafka consumer for space topic events.
pub struct SpaceTopicsConsumer {
    consumer: StreamConsumer,
    topic: String,
    batch_size: usize,
    batch_timeout: Duration,
}

impl SpaceTopicsConsumer {
    /// The Kafka topic for space topics.
    const SPACE_TOPICS_TOPIC: &'static str = "space.topics";

    /// Default batch size for Kafka message batching.
    const DEFAULT_BATCH_SIZE: usize = 10;

    /// Default batch timeout in milliseconds.
    const DEFAULT_BATCH_TIMEOUT_MS: u64 = 1000;

    /// Create a new space topics consumer.
    ///
    /// Configuration is read from environment variables:
    /// - ENVIRONMENT: Environment name for topic prefix ("staging" or "production")
    /// - SPACE_TOPICS_KAFKA_TOPIC: Base topic name (default: "space.topics")
    /// - SPACE_TOPICS_BATCH_SIZE: Batch size (default: 10)
    /// - SPACE_TOPICS_BATCH_TIMEOUT_MS: Batch timeout in milliseconds (default: 1000)
    ///
    /// # Arguments
    ///
    /// * `brokers` - Kafka broker addresses (comma-separated)
    /// * `group_id` - Consumer group ID (will append "-space-topics" suffix)
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IngestError> {
        let prefix = get_topic_prefix();
        let base_topic = env::var("SPACE_TOPICS_KAFKA_TOPIC")
            .unwrap_or_else(|_| Self::SPACE_TOPICS_TOPIC.to_string());
        let topic = format!("{}{}", prefix, base_topic);

        let batch_size = env::var("SPACE_TOPICS_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_SIZE);

        let batch_timeout_ms = env::var("SPACE_TOPICS_BATCH_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_TIMEOUT_MS);

        Self::with_config(brokers, group_id, topic, batch_size, batch_timeout_ms)
    }

    /// Create a new space topics consumer with custom configuration.
    pub fn with_config(
        brokers: &str,
        group_id: &str,
        topic: String,
        batch_size: usize,
        batch_timeout_ms: u64,
    ) -> Result<Self, IngestError> {
        let client_config = super::kafka_config::create_client_config(brokers, group_id);

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic = %topic,
            batch_size = batch_size,
            batch_timeout_ms = batch_timeout_ms,
            "Created space topics consumer"
        );

        let consumer: StreamConsumer = client_config
            .create()
            .map_err(|e| IngestError::kafka(e.to_string()))?;

        Ok(Self {
            consumer,
            topic,
            batch_size,
            batch_timeout: Duration::from_millis(batch_timeout_ms),
        })
    }

    /// Subscribe to the space topics topic.
    pub fn subscribe(&self) -> Result<(), IngestError> {
        self.consumer
            .subscribe(&[&self.topic])
            .map_err(|e| IngestError::kafka(e.to_string()))?;

        info!(topic = %self.topic, "Subscribed to space topics Kafka topic");
        Ok(())
    }

    /// Start consuming messages and send them through the channel.
    #[instrument(skip(self, processor_tx, ack_receiver, shutdown))]
    pub async fn run(
        &self,
        processor_tx: mpsc::Sender<SpaceTopicProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        use futures::StreamExt;

        let mut message_stream = self.consumer.stream();
        let mut batch: Vec<PendingSpaceTopicMessage> = Vec::with_capacity(self.batch_size);
        let mut pending_offsets: Vec<(String, i32, i64)> = Vec::new();
        let mut flush_timer = tokio::time::interval(self.batch_timeout);
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        flush_timer.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("Space topics consumer received shutdown signal");
                    break;
                }
                ack_msg = ack_receiver.recv() => {
                    match ack_msg {
                        Some(StreamMessage::Acknowledgment { offsets, success, error }) => {
                            if success {
                                let max_offset = offsets.iter().map(|(_, _, o)| *o).max().unwrap_or(0);
                                if let Err(e) = self.commit_offsets(&offsets).await {
                                    error!(error = %e, offset_count = offsets.len(), max_offset, "Failed to commit space topics offsets after ACK");
                                } else {
                                    debug!(
                                        offset_count = offsets.len(),
                                        max_offset,
                                        "ACK: committed space topics offsets"
                                    );
                                }
                            } else {
                                let max_offset = offsets.iter().map(|(_, _, o)| *o).max().unwrap_or(0);
                                error!(
                                    offset_count = offsets.len(),
                                    max_offset,
                                    error = error.as_deref().unwrap_or("Unknown error"),
                                    "NACK: shutting down consumer to prevent data loss"
                                );
                                return Err(IngestError::LoaderError(
                                    format!("Batch processing failed: {}", error.as_deref().unwrap_or("Unknown error"))
                                ));
                            }
                        }
                        Some(StreamMessage::End) | None => {
                            info!("Space topics acknowledgment channel closed");
                            break;
                        }
                        _ => {}
                    }
                }
                message = message_stream.next() => {
                    match message {
                        Some(Ok(msg)) => {
                            debug!(
                                topic = %msg.topic(),
                                partition = msg.partition(),
                                offset = msg.offset(),
                                "Received space topics message from Kafka"
                            );
                            match self.parse_message(&msg) {
                                Ok(Some(pending)) => {
                                    batch.push(pending);
                                    pending_offsets.push((msg.topic().to_string(), msg.partition(), msg.offset()));

                                    if batch.len() >= self.batch_size {
                                        let offsets_to_send = pending_offsets.clone();
                                        self.flush_batch(&batch, &offsets_to_send, &processor_tx).await?;
                                        batch.clear();
                                        pending_offsets.clear();
                                    }
                                }
                                Ok(None) => {
                                    // Empty message, commit immediately
                                    debug!(
                                        topic = %msg.topic(),
                                        partition = msg.partition(),
                                        offset = msg.offset(),
                                        "Space topics message parsed but no events extracted"
                                    );

                                    let mut tpl = TopicPartitionList::new();
                                    tpl.add_partition_offset(
                                        msg.topic(),
                                        msg.partition(),
                                        rdkafka::Offset::Offset(msg.offset() + 1)
                                    )
                                    .map_err(|e| IngestError::kafka(e.to_string()))?;
                                    self.consumer
                                        .commit(&tpl, rdkafka::consumer::CommitMode::Async)
                                        .map_err(|e| IngestError::kafka(e.to_string()))?;
                                }
                                Err(e) => {
                                    error!(
                                        topic = %msg.topic(),
                                        partition = msg.partition(),
                                        offset = msg.offset(),
                                        error = %e,
                                        "Failed to parse space topics message"
                                    );
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!(error = %e, "Kafka error in space topics consumer");
                        }
                        None => {
                            info!("Space topics Kafka stream ended");
                            if !batch.is_empty() {
                                let offsets_to_send = pending_offsets.clone();
                                self.flush_batch(&batch, &offsets_to_send, &processor_tx).await?;
                            }
                            break;
                        }
                    }
                }
                _ = flush_timer.tick() => {
                    if !batch.is_empty() {
                        debug!(count = batch.len(), "Flushing space topics batch due to timeout");
                        let offsets_to_send = pending_offsets.clone();
                        self.flush_batch(&batch, &offsets_to_send, &processor_tx).await?;
                        batch.clear();
                        pending_offsets.clear();
                    }
                }
            }
        }

        Ok(())
    }

    /// Flush a batch of pending space topic messages to the channel.
    async fn flush_batch(
        &self,
        batch: &[PendingSpaceTopicMessage],
        offsets: &[(String, i32, i64)],
        processor_tx: &mpsc::Sender<SpaceTopicProcessingBatch>,
    ) -> Result<(), IngestError> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut all_events = Vec::new();
        for pending in batch {
            all_events.extend(pending.events.clone());
        }

        if !all_events.is_empty() {
            let event_count = all_events.len();
            let first_offset = offsets.first().map(|(_, _, o)| *o).unwrap_or(0);
            let last_offset = offsets.last().map(|(_, _, o)| *o).unwrap_or(0);

            async {
                debug!(
                    event_count = event_count,
                    message_count = batch.len(),
                    "Sending batch of space topic events to processor"
                );
                processor_tx
                    .send(SpaceTopicProcessingBatch {
                        events: all_events,
                        offsets: offsets.to_vec(),
                        event_count,
                    })
                    .await
                    .map_err(|e| IngestError::ChannelError(e.to_string()))
            }
            .instrument(info_span!(
                "search_indexer.consume_space_topics_batch",
                batch_size = batch.len(),
                event_count = event_count,
                offset_start = first_offset,
                offset_end = last_offset
            ))
            .await?;
        }

        Ok(())
    }

    /// Commit offsets for a batch of messages.
    async fn commit_offsets(&self, offsets: &[(String, i32, i64)]) -> Result<(), IngestError> {
        if offsets.is_empty() {
            return Ok(());
        }

        let mut tpl = TopicPartitionList::new();
        for (topic, partition, offset) in offsets {
            tpl.add_partition_offset(topic, *partition, rdkafka::Offset::Offset(offset + 1))
                .map_err(|e| IngestError::kafka(e.to_string()))?;
        }

        self.consumer
            .commit(&tpl, rdkafka::consumer::CommitMode::Async)
            .map_err(|e| IngestError::kafka(e.to_string()))?;

        Ok(())
    }

    /// Parse a Kafka message into a space topic event.
    fn parse_message(
        &self,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Result<Option<PendingSpaceTopicMessage>, IngestError> {
        let payload = match msg.payload() {
            Some(p) => p,
            None => {
                warn!("Received space topics message with empty payload");
                return Ok(None);
            }
        };

        let topic_declared = HermesTopicDeclared::decode(payload).map_err(|e| {
            IngestError::parse(format!("Failed to decode HermesTopicDeclared: {}", e))
        })?;

        if topic_declared.space_id.len() != 16 {
            warn!(
                space_id_len = topic_declared.space_id.len(),
                "Invalid space_id length in HermesTopicDeclared, expected 16 bytes"
            );
            return Ok(None);
        }

        if topic_declared.topic_id.len() != 16 {
            warn!(
                topic_id_len = topic_declared.topic_id.len(),
                "Invalid topic_id length in HermesTopicDeclared, expected 16 bytes"
            );
            return Ok(None);
        }

        let space_bytes: [u8; 16] = topic_declared
            .space_id
            .as_slice()
            .try_into()
            .map_err(|_| IngestError::parse("Failed to convert space_id bytes".to_string()))?;
        let topic_bytes: [u8; 16] = topic_declared
            .topic_id
            .as_slice()
            .try_into()
            .map_err(|_| IngestError::parse("Failed to convert topic_id bytes".to_string()))?;

        let space_id = Uuid::from_bytes(space_bytes);
        let topic_entity_id = Uuid::from_bytes(topic_bytes);

        debug!(
            space_id = %space_id,
            topic_entity_id = %topic_entity_id,
            "Parsed HermesTopicDeclared"
        );

        Ok(Some(PendingSpaceTopicMessage {
            events: vec![SpaceTopicEvent {
                space_id,
                topic_entity_id,
            }],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(SpaceTopicsConsumer::SPACE_TOPICS_TOPIC, "space.topics");
        assert_eq!(SpaceTopicsConsumer::DEFAULT_BATCH_SIZE, 10);
        assert_eq!(SpaceTopicsConsumer::DEFAULT_BATCH_TIMEOUT_MS, 1000);
    }
}
