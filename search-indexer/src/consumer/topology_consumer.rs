//! Kafka consumer for topology canonical graph diffs from the topology.canonical topic.
//!
//! Consumes `CanonicalGraphDiff` messages and forwards parsed diffs to the processor.

use hermes_instrumentation::{debug, error, info, info_span, instrument, warn, Instrument};
use hermes_kafka::get_topic_prefix;
use prost::Message;
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    message::Message as KafkaMessage,
    TopicPartitionList,
};
use std::env;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::consumer::messages::StreamMessage;
use crate::errors::IngestError;
use crate::orchestrator::TopologyProcessingBatch;
use crate::topology::state::{ChangeType, ParsedNodeChange};

use hermes_schema::pb::topology::{CanonicalGraphDiff, ChangeType as ProtoChangeType};

/// A parsed diff ready for processing.
#[derive(Debug, Clone)]
pub struct ParsedCanonicalGraphDiff {
    pub root_id: [u8; 16],
    pub changes: Vec<ParsedNodeChange>,
}

/// Pending topology message for batching.
struct PendingTopologyMessage {
    diff: ParsedCanonicalGraphDiff,
}

/// Kafka consumer for canonical graph topology diffs.
pub struct TopologyConsumer {
    consumer: StreamConsumer,
    topic: String,
    batch_size: usize,
    batch_timeout: Duration,
}

impl TopologyConsumer {
    const TOPOLOGY_TOPIC: &'static str = "topology.canonical";
    const DEFAULT_BATCH_SIZE: usize = 10;
    const DEFAULT_BATCH_TIMEOUT_MS: u64 = 1000;

    /// Create a new topology consumer.
    ///
    /// Configuration from environment:
    /// - `TOPOLOGY_KAFKA_TOPIC`: Base topic name (default: "topology.canonical")
    /// - `TOPOLOGY_BATCH_SIZE`: Batch size (default: 10)
    /// - `TOPOLOGY_BATCH_TIMEOUT_MS`: Batch timeout ms (default: 1000)
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IngestError> {
        let prefix = get_topic_prefix();
        let base_topic =
            env::var("TOPOLOGY_KAFKA_TOPIC").unwrap_or_else(|_| Self::TOPOLOGY_TOPIC.to_string());
        let topic = format!("{}{}", prefix, base_topic);

        let batch_size = env::var("TOPOLOGY_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_SIZE);

        let batch_timeout_ms = env::var("TOPOLOGY_BATCH_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_TIMEOUT_MS);

        Self::with_config(brokers, group_id, topic, batch_size, batch_timeout_ms)
    }

    /// Create with custom config.
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
            "Created topology consumer"
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

    /// Subscribe to the topology topic.
    pub fn subscribe(&self) -> Result<(), IngestError> {
        self.consumer
            .subscribe(&[&self.topic])
            .map_err(|e| IngestError::kafka(e.to_string()))?;

        info!(topic = %self.topic, "Subscribed to topology Kafka topic");
        Ok(())
    }

    /// Start consuming messages and send them through the channel.
    #[instrument(skip(self, processor_tx, ack_receiver, shutdown))]
    pub async fn run(
        &self,
        processor_tx: mpsc::Sender<TopologyProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        use futures::StreamExt;

        let mut message_stream = self.consumer.stream();
        let mut batch: Vec<PendingTopologyMessage> = Vec::with_capacity(self.batch_size);
        let mut pending_offsets: Vec<(String, i32, i64)> = Vec::new();
        let mut flush_timer = tokio::time::interval(self.batch_timeout);
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        flush_timer.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("Topology consumer received shutdown signal");
                    break;
                }
                ack_msg = ack_receiver.recv() => {
                    match ack_msg {
                        Some(StreamMessage::Acknowledgment { offsets, success, error }) => {
                            if success {
                                let max_offset = offsets.iter().map(|(_, _, o)| *o).max().unwrap_or(0);
                                if let Err(e) = self.commit_offsets(&offsets).await {
                                    error!(error = %e, offset_count = offsets.len(), max_offset, "Failed to commit topology offsets after ACK");
                                } else {
                                    debug!(
                                        offset_count = offsets.len(),
                                        max_offset,
                                        "ACK: committed topology offsets"
                                    );
                                }
                            } else {
                                let max_offset = offsets.iter().map(|(_, _, o)| *o).max().unwrap_or(0);
                                error!(
                                    offset_count = offsets.len(),
                                    max_offset,
                                    error = error.as_deref().unwrap_or("Unknown error"),
                                    "NACK: shutting down topology consumer to prevent data loss"
                                );
                                return Err(IngestError::LoaderError(
                                    format!("Topology batch processing failed: {}", error.as_deref().unwrap_or("Unknown error"))
                                ));
                            }
                        }
                        Some(StreamMessage::End) | None => {
                            info!("Topology acknowledgment channel closed");
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
                                "Received topology message from Kafka"
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
                                    debug!(
                                        topic = %msg.topic(),
                                        partition = msg.partition(),
                                        offset = msg.offset(),
                                        "Topology message parsed but no changes extracted"
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
                                        "Failed to parse topology message"
                                    );
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!(error = %e, "Kafka error in topology consumer");
                        }
                        None => {
                            info!("Topology Kafka stream ended");
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
                        debug!(count = batch.len(), "Flushing topology batch due to timeout");
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

    /// Flush a batch of pending topology messages to the channel.
    async fn flush_batch(
        &self,
        batch: &[PendingTopologyMessage],
        offsets: &[(String, i32, i64)],
        processor_tx: &mpsc::Sender<TopologyProcessingBatch>,
    ) -> Result<(), IngestError> {
        if batch.is_empty() {
            return Ok(());
        }

        let diffs: Vec<ParsedCanonicalGraphDiff> = batch.iter().map(|p| p.diff.clone()).collect();
        let event_count = diffs.iter().map(|d| d.changes.len()).sum::<usize>();

        if event_count > 0 || !diffs.is_empty() {
            let first_offset = offsets.first().map(|(_, _, o)| *o).unwrap_or(0);
            let last_offset = offsets.last().map(|(_, _, o)| *o).unwrap_or(0);

            async {
                debug!(
                    event_count = event_count,
                    message_count = batch.len(),
                    "Sending batch of topology diffs to processor"
                );
                processor_tx
                    .send(TopologyProcessingBatch {
                        diffs,
                        offsets: offsets.to_vec(),
                        event_count,
                    })
                    .await
                    .map_err(|e| IngestError::ChannelError(e.to_string()))
            }
            .instrument(info_span!(
                "search_indexer.consume_topology_batch",
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

    /// Parse a Kafka message into a topology diff.
    fn parse_message(
        &self,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Result<Option<PendingTopologyMessage>, IngestError> {
        let payload = match msg.payload() {
            Some(p) => p,
            None => {
                warn!("Received topology message with empty payload");
                return Ok(None);
            }
        };

        let diff = CanonicalGraphDiff::decode(payload).map_err(|e| {
            IngestError::parse(format!("Failed to decode CanonicalGraphDiff: {}", e))
        })?;

        if diff.root_id.len() != 16 {
            warn!(
                root_id_len = diff.root_id.len(),
                "Invalid root_id length in CanonicalGraphDiff, expected 16 bytes"
            );
            return Ok(None);
        }

        let root_id: [u8; 16] = diff
            .root_id
            .as_slice()
            .try_into()
            .map_err(|_| IngestError::parse("Failed to convert root_id bytes".to_string()))?;

        let mut changes = Vec::with_capacity(diff.changes.len());

        for node_change in &diff.changes {
            if node_change.space_id.len() != 16 {
                warn!(
                    space_id_len = node_change.space_id.len(),
                    "Invalid space_id length in NodeChange, skipping"
                );
                continue;
            }

            let space_id: [u8; 16] =
                node_change.space_id.as_slice().try_into().map_err(|_| {
                    IngestError::parse("Failed to convert space_id bytes".to_string())
                })?;

            let change_type = match ProtoChangeType::try_from(node_change.change_type) {
                Ok(ProtoChangeType::Added) => ChangeType::Added,
                Ok(ProtoChangeType::Removed) => ChangeType::Removed,
                Ok(ProtoChangeType::Moved) => ChangeType::Moved,
                Ok(ProtoChangeType::Unspecified) | Err(_) => {
                    warn!(
                        change_type = node_change.change_type,
                        "Unknown change type in NodeChange, skipping"
                    );
                    continue;
                }
            };

            let parent_id = node_change.parent_edge.as_ref().and_then(|edge| {
                if edge.parent_id.len() == 16 {
                    let mut arr = [0u8; 16];
                    arr.copy_from_slice(&edge.parent_id);
                    Some(arr)
                } else {
                    None
                }
            });

            changes.push(ParsedNodeChange {
                space_id,
                change_type,
                distance: node_change.distance,
                parent_id,
            });
        }

        if changes.is_empty() {
            return Ok(None);
        }

        debug!(
            root_id = %uuid::Uuid::from_bytes(root_id),
            change_count = changes.len(),
            "Parsed CanonicalGraphDiff"
        );

        Ok(Some(PendingTopologyMessage {
            diff: ParsedCanonicalGraphDiff { root_id, changes },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(TopologyConsumer::TOPOLOGY_TOPIC, "topology.canonical");
        assert_eq!(TopologyConsumer::DEFAULT_BATCH_SIZE, 10);
        assert_eq!(TopologyConsumer::DEFAULT_BATCH_TIMEOUT_MS, 1000);
    }
}
