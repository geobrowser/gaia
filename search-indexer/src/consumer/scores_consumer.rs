//! Kafka consumer for score updates from the curation.scores topic.
//!
//! Consumes HermesScoresBatch messages and forwards score events to the ingest.

use prost::Message;
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    message::Message as KafkaMessage,
    TopicPartitionList,
};
use std::env;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

use crate::consumer::messages::{ScoreEvent, StreamMessage};
use crate::errors::IngestError;
use crate::orchestrator::ScoreProcessingBatch;

use hermes_schema::pb::scoring::HermesScoresBatch;

/// Pending score messages for batching.
struct PendingScoreMessage {
    events: Vec<ScoreEvent>,
}

/// Kafka consumer for score events.
pub struct ScoresConsumer {
    consumer: StreamConsumer,
    topic: String,
    batch_size: usize,
    batch_timeout: Duration,
}

impl ScoresConsumer {
    /// The Kafka topic for curation scores.
    const SCORES_TOPIC: &'static str = "curation.scores";

    /// Default batch size for Kafka message batching.
    const DEFAULT_BATCH_SIZE: usize = 50;

    /// Default batch timeout in milliseconds.
    const DEFAULT_BATCH_TIMEOUT_MS: u64 = 1000;

    /// Create a new scores consumer.
    ///
    /// Configuration is read from environment variables:
    /// - SCORES_KAFKA_TOPIC: Topic name (default: "curation.scores")
    /// - SCORES_BATCH_SIZE: Batch size (default: 50)
    /// - SCORES_BATCH_TIMEOUT_MS: Batch timeout in milliseconds (default: 1000)
    ///
    /// # Arguments
    ///
    /// * `brokers` - Kafka broker addresses (comma-separated)
    /// * `group_id` - Consumer group ID (will append "-scores" suffix)
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IngestError> {
        let topic =
            env::var("SCORES_KAFKA_TOPIC").unwrap_or_else(|_| Self::SCORES_TOPIC.to_string());

        let batch_size = env::var("SCORES_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_SIZE);

        let batch_timeout_ms = env::var("SCORES_BATCH_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_TIMEOUT_MS);

        Self::with_config(brokers, group_id, topic, batch_size, batch_timeout_ms)
    }

    /// Create a new scores consumer with custom configuration.
    pub fn with_config(
        brokers: &str,
        group_id: &str,
        topic: String,
        batch_size: usize,
        batch_timeout_ms: u64,
    ) -> Result<Self, IngestError> {
        // Use a separate consumer group for scores
        let scores_group_id = format!("{}-scores", group_id);

        let client_config = super::kafka_config::create_client_config(brokers, &scores_group_id);

        info!(
            brokers = %brokers,
            group_id = %scores_group_id,
            topic = %topic,
            batch_size = batch_size,
            batch_timeout_ms = batch_timeout_ms,
            "Created scores consumer"
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

    /// Subscribe to the scores topic.
    pub fn subscribe(&self) -> Result<(), IngestError> {
        self.consumer
            .subscribe(&[&self.topic])
            .map_err(|e| IngestError::kafka(e.to_string()))?;

        info!(topic = %self.topic, "Subscribed to scores Kafka topic");
        Ok(())
    }

    /// Start consuming messages and send them through the channel.
    #[instrument(skip(self, processor_tx, ack_receiver, shutdown))]
    pub async fn run(
        &self,
        processor_tx: mpsc::Sender<ScoreProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        use futures::StreamExt;

        let mut message_stream = self.consumer.stream();
        let mut batch: Vec<PendingScoreMessage> = Vec::with_capacity(self.batch_size);
        let mut pending_offsets: Vec<(String, i32, i64)> = Vec::new();
        let mut flush_timer = tokio::time::interval(self.batch_timeout);
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        flush_timer.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("Scores consumer received shutdown signal");
                    break;
                }
                ack_msg = ack_receiver.recv() => {
                    match ack_msg {
                        Some(StreamMessage::Acknowledgment { offsets, success, error }) => {
                            if success {
                                if let Err(e) = self.commit_offsets(&offsets).await {
                                    error!(error = %e, "Failed to commit scores offsets");
                                } else {
                                    debug!(offset_count = offsets.len(), "Committed scores offsets");
                                }
                            } else {
                                error!(
                                    offset_count = offsets.len(),
                                    error = error.as_deref().unwrap_or("Unknown error"),
                                    "Not committing scores offsets due to processing failure"
                                );
                            }
                        }
                        Some(StreamMessage::End) | None => {
                            info!("Scores acknowledgment channel closed");
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
                                "Received scores message from Kafka"
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
                                    // Empty batch, commit immediately
                                    debug!(
                                        topic = %msg.topic(),
                                        partition = msg.partition(),
                                        offset = msg.offset(),
                                        "Scores message parsed but no events extracted"
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
                                        "Failed to parse scores message"
                                    );
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!(error = %e, "Kafka error in scores consumer");
                        }
                        None => {
                            info!("Scores Kafka stream ended");
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
                        debug!(count = batch.len(), "Flushing scores batch due to timeout");
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

    /// Flush a batch of pending score messages to the channel.
    async fn flush_batch(
        &self,
        batch: &[PendingScoreMessage],
        offsets: &[(String, i32, i64)],
        processor_tx: &mpsc::Sender<ScoreProcessingBatch>,
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
            info!(
                event_count = event_count,
                message_count = batch.len(),
                "Sending batch of score events to processor"
            );
            processor_tx
                .send(ScoreProcessingBatch {
                    events: all_events,
                    offsets: offsets.to_vec(),
                    event_count,
                })
                .await
                .map_err(|e| IngestError::ChannelError(e.to_string()))?;
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

    /// Parse a Kafka message into score events.
    fn parse_message(
        &self,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Result<Option<PendingScoreMessage>, IngestError> {
        let payload = match msg.payload() {
            Some(p) => p,
            None => {
                warn!("Received scores message with empty payload");
                return Ok(None);
            }
        };

        let scores_batch = HermesScoresBatch::decode(payload).map_err(|e| {
            IngestError::parse(format!("Failed to decode HermesScoresBatch: {}", e))
        })?;

        debug!(
            entity_scores_count = scores_batch.entity_scores.len(),
            perspective_scores_count = scores_batch.perspective_scores.len(),
            space_scores_count = scores_batch.space_scores.len(),
            batch_sequence = scores_batch.batch_sequence,
            is_final = scores_batch.is_final,
            "Parsed HermesScoresBatch"
        );

        let mut events = Vec::new();

        // Process entity global scores
        for entity_score in &scores_batch.entity_scores {
            if entity_score.entity_id.len() == 16 {
                let bytes: [u8; 16] =
                    entity_score.entity_id.as_slice().try_into().map_err(|_| {
                        IngestError::parse("Failed to convert entity_id bytes".to_string())
                    })?;
                let entity_id = Uuid::from_bytes(bytes);
                events.push(ScoreEvent::entity_global_score(
                    entity_id,
                    entity_score.score,
                    entity_score.updated_at,
                ));
            } else {
                warn!(
                    entity_id_len = entity_score.entity_id.len(),
                    "Invalid entity_id length in EntityScore, expected 16 bytes"
                );
            }
        }

        // Process space scores
        for space_score in &scores_batch.space_scores {
            if space_score.space_id.len() == 16 {
                let bytes: [u8; 16] = space_score.space_id.as_slice().try_into().map_err(|_| {
                    IngestError::parse("Failed to convert space_id bytes".to_string())
                })?;
                let space_id = Uuid::from_bytes(bytes);
                events.push(ScoreEvent::space_score(
                    space_id,
                    space_score.score,
                    space_score.updated_at,
                ));
            } else {
                warn!(
                    space_id_len = space_score.space_id.len(),
                    "Invalid space_id length in SpaceScore, expected 16 bytes"
                );
            }
        }

        // Process perspective scores (entity-space scores)
        for perspective_score in &scores_batch.perspective_scores {
            if perspective_score.entity_id.len() == 16 && perspective_score.space_id.len() == 16 {
                let entity_bytes: [u8; 16] = perspective_score
                    .entity_id
                    .as_slice()
                    .try_into()
                    .map_err(|_| {
                        IngestError::parse("Failed to convert entity_id bytes".to_string())
                    })?;
                let space_bytes: [u8; 16] = perspective_score
                    .space_id
                    .as_slice()
                    .try_into()
                    .map_err(|_| {
                        IngestError::parse("Failed to convert space_id bytes".to_string())
                    })?;
                let entity_id = Uuid::from_bytes(entity_bytes);
                let space_id = Uuid::from_bytes(space_bytes);
                events.push(ScoreEvent::entity_space_score(
                    entity_id,
                    space_id,
                    perspective_score.score,
                    perspective_score.updated_at,
                ));
            } else {
                warn!(
                    entity_id_len = perspective_score.entity_id.len(),
                    space_id_len = perspective_score.space_id.len(),
                    "Invalid ID lengths in PerspectiveScore, expected 16 bytes each"
                );
            }
        }

        if events.is_empty() {
            return Ok(None);
        }

        Ok(Some(PendingScoreMessage { events }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(ScoresConsumer::SCORES_TOPIC, "curation.scores");
        assert_eq!(ScoresConsumer::DEFAULT_BATCH_SIZE, 50);
        assert_eq!(ScoresConsumer::DEFAULT_BATCH_TIMEOUT_MS, 1000);
    }
}
