use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::consumer::KafkaConsumer;
use crate::error::IndexerError;
use crate::models::relations::RelationOp;
use crate::models::values::ValueOp;
use crate::storage::Storage;

/// Data needed to write versioned records for a single edit.
#[derive(Debug)]
pub struct VersionOp {
    /// Edit ID (from HermesEdit.id)
    pub edit_id: Uuid,
    /// Block number for version_key computation
    pub block_number: i64,
    /// Sequence within block for version_key computation
    pub sequence: i64,
    /// Unix timestamp of the edit
    pub created_at: i64,
    /// Value operations to write to value_versions
    pub values: Vec<ValueOp>,
    /// Relation operations to write to relation_versions
    pub relations: Vec<RelationOp>,
    /// Kafka metadata for offset commit after write
    pub kafka_topic: String,
    pub kafka_partition: i32,
    pub kafka_offset: i64,
}

/// Async buffer for versioned writes.
///
/// Decouples versioned writes from the live write path:
/// - Live writes happen immediately (low latency)
/// - Versioned writes queued here and processed asynchronously
/// - Kafka offsets committed only after versioned writes complete
pub struct VersionBuffer {
    tx: mpsc::Sender<VersionOp>,
}

impl VersionBuffer {
    /// Create a new version buffer with a background writer task.
    ///
    /// # Arguments
    /// * `storage` - Database connection for versioned writes
    /// * `consumer` - Kafka consumer for offset commits
    /// * `buffer_size` - Channel capacity (backpressure threshold)
    pub fn new(storage: Arc<Storage>, consumer: Arc<KafkaConsumer>, buffer_size: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer_size);

        // Spawn background writer task
        tokio::spawn(Self::run_writer(rx, storage, consumer));

        Self { tx }
    }

    /// Queue a version operation for async processing.
    ///
    /// This will block if the buffer is full (backpressure).
    pub async fn send(&self, op: VersionOp) -> Result<(), IndexerError> {
        self.tx
            .send(op)
            .await
            .map_err(|_| IndexerError::internal("Version buffer channel closed"))
    }

    /// Try to queue a version operation without blocking.
    ///
    /// Returns Ok(true) if queued, Ok(false) if buffer full.
    #[allow(dead_code)]
    pub fn try_send(&self, op: VersionOp) -> Result<bool, IndexerError> {
        match self.tx.try_send(op) {
            Ok(()) => Ok(true),
            Err(mpsc::error::TrySendError::Full(_)) => Ok(false),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(IndexerError::internal("Version buffer channel closed"))
            }
        }
    }

    /// Background task that processes versioned writes.
    async fn run_writer(
        mut rx: mpsc::Receiver<VersionOp>,
        storage: Arc<Storage>,
        consumer: Arc<KafkaConsumer>,
    ) {
        info!("Version buffer writer started");

        while let Some(op) = rx.recv().await {
            let span = info_span!(
                "version_buffer.write",
                edit_id = %op.edit_id,
                block_number = op.block_number,
                sequence = op.sequence,
                values = op.values.len(),
                relations = op.relations.len(),
            );

            let result = Self::process_op(&storage, &consumer, op)
                .instrument(span)
                .await;

            if let Err(e) = result {
                error!(error = %e, "Failed to process versioned write");
                // Continue processing - don't want one failure to stop the buffer
                // The offset won't be committed, so on restart we'll retry
            }
        }

        warn!("Version buffer writer stopped - channel closed");
    }

    /// Process a single version operation.
    async fn process_op(
        storage: &Storage,
        consumer: &KafkaConsumer,
        op: VersionOp,
    ) -> Result<(), IndexerError> {
        // Start a transaction for versioned writes
        let mut tx = storage.pool.begin().await?;

        // Compute version_key
        let version_key = storage
            .insert_edit_version(
                op.edit_id,
                op.block_number,
                op.sequence,
                op.created_at,
                &mut tx,
            )
            .await?;

        debug!(
            version_key = version_key,
            edit_id = %op.edit_id,
            "Inserted edit version"
        );

        // Write versioned values
        if !op.values.is_empty() {
            storage
                .insert_value_versions(&op.values, version_key, &mut tx)
                .await?;
            debug!(count = op.values.len(), "Inserted value versions");
        }

        // Write versioned relations
        if !op.relations.is_empty() {
            storage
                .insert_relation_versions(&op.relations, version_key, &mut tx)
                .await?;
            debug!(count = op.relations.len(), "Inserted relation versions");
        }

        // Commit the transaction
        tx.commit().await?;

        // Commit Kafka offset only after successful DB write
        consumer.commit_message(&op.kafka_topic, op.kafka_partition, op.kafka_offset)?;

        debug!(
            topic = %op.kafka_topic,
            partition = op.kafka_partition,
            offset = op.kafka_offset,
            "Committed version offset"
        );

        Ok(())
    }
}
