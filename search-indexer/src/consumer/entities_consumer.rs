//! Kafka consumer implementation for the search indexer.
//!
//! Consumes entity events from Kafka topics and forwards them to the ingest.

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

use crate::consumer::messages::EntityEvent;
use crate::errors::IngestError;
use crate::orchestrator::EntityProcessingBatch;

use hermes_schema::pb::knowledge::HermesEdit;
use indexer_utils::id::transform_id_bytes;
use sdk::core::ids::{
    AVATAR_PROPERTY_ID, DESCRIPTION_PROPERTY_ID, NAME_PROPERTY_ID, TYPE_RELATION_TYPE_ID,
};
use wire::pb::grc20::{op::Payload, Relation, RelationUpdate};

/// Pending message information for batching.
struct PendingMessage {
    events: Vec<EntityEvent>,
}

/// Kafka consumer for entity events.
pub struct EntitiesConsumer {
    consumer: StreamConsumer,
    topics: Vec<String>,
    batch_size: usize,
    batch_timeout: Duration,
}

impl EntitiesConsumer {
    /// The Kafka topic for knowledge edits (configurable via KAFKA_TOPIC env var).
    const KNOWLEDGE_EDITS_TOPIC: &'static str = "knowledge.edits";

    /// Default batch size for Kafka message batching (configurable via KAFKA_BATCH_SIZE env var).
    const DEFAULT_BATCH_SIZE: usize = 50;

    /// Default batch timeout in milliseconds (configurable via KAFKA_BATCH_TIMEOUT_MS env var).
    const DEFAULT_BATCH_TIMEOUT_MS: u64 = 1000;
    /// Create a new Kafka consumer.
    ///
    /// Configuration is read from environment variables with fallbacks to defaults:
    /// - KAFKA_TOPIC: Topic name (default: "knowledge.edits")
    /// - KAFKA_BATCH_SIZE: Batch size (default: 50)
    /// - KAFKA_BATCH_TIMEOUT_MS: Batch timeout in milliseconds (default: 1000)
    ///
    /// # Arguments
    ///
    /// * `brokers` - Kafka broker addresses (comma-separated)
    /// * `group_id` - Consumer group ID
    ///
    /// # Returns
    ///
    /// * `Ok(EntitiesConsumer)` - A new consumer instance
    /// * `Err(IngestError)` - If consumer creation fails
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IngestError> {
        let topic =
            env::var("KAFKA_TOPIC").unwrap_or_else(|_| Self::KNOWLEDGE_EDITS_TOPIC.to_string());

        let batch_size = env::var("KAFKA_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_SIZE);

        let batch_timeout_ms = env::var("KAFKA_BATCH_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(Self::DEFAULT_BATCH_TIMEOUT_MS);

        Self::with_batch_config(brokers, group_id, topic, batch_size, batch_timeout_ms)
    }

    /// Create a new Kafka consumer with custom batch configuration.
    ///
    /// # Arguments
    ///
    /// * `brokers` - Kafka broker addresses (comma-separated)
    /// * `group_id` - Consumer group ID
    /// * `topic` - Kafka topic to consume from
    /// * `batch_size` - Number of messages to batch before sending
    /// * `batch_timeout_ms` - Maximum time to wait before flushing a partial batch (milliseconds)
    ///
    /// # Returns
    ///
    /// * `Ok(EntitiesConsumer)` - A new consumer instance
    /// * `Err(IngestError)` - If consumer creation fails
    pub fn with_batch_config(
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
            "Created entities consumer"
        );

        let consumer: StreamConsumer = client_config
            .create()
            .map_err(|e| IngestError::kafka(e.to_string()))?;

        Ok(Self {
            consumer,
            topics: vec![topic.clone()],
            batch_size,
            batch_timeout: Duration::from_millis(batch_timeout_ms),
        })
    }

    /// Subscribe to configured topics.
    pub fn subscribe(&self) -> Result<(), IngestError> {
        let topics: Vec<&str> = self.topics.iter().map(|s| s.as_str()).collect();
        self.consumer
            .subscribe(&topics)
            .map_err(|e| IngestError::kafka(e.to_string()))?;

        info!(topics = ?self.topics, "Subscribed to Kafka topics");
        Ok(())
    }

    /// Start consuming messages and send them through the channel.
    ///
    /// Messages are batched before being sent to improve efficiency.
    ///
    /// # Arguments
    ///
    /// * `processor_tx` - Channel to send events to processor
    /// * `ack_receiver` - Channel to receive acknowledgments from loader
    /// * `shutdown` - Shutdown signal receiver
    #[instrument(skip(self, processor_tx, ack_receiver, shutdown))]
    pub async fn run(
        &self,
        processor_tx: mpsc::Sender<EntityProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<crate::consumer::messages::StreamMessage>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        use futures::StreamExt;

        let mut message_stream = self.consumer.stream();
        let mut batch: Vec<PendingMessage> = Vec::with_capacity(self.batch_size);
        let mut pending_offsets: Vec<(String, i32, i64)> = Vec::new();
        let mut flush_timer = tokio::time::interval(self.batch_timeout);
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Skip the first tick immediately
        flush_timer.tick().await;

        // ========================================================================================
        // MAIN CONSUMER LOOP: Coordinate multiple async operations
        //
        // tokio::select! allows waiting on multiple async operations concurrently and executes
        // the first branch that becomes ready.
        //
        // EXECUTION ORDER (by priority):
        // 1. Shutdown signal - Immediate termination, highest priority for graceful shutdown
        // 2. Acknowledgments - Offset commits after successful processing (time-sensitive)
        // 3. Kafka messages - Regular message processing and batching
        // 4. Batch timeout - Periodic flushing of accumulated messages
        //
        // BENEFITS:
        // - Less race condition risk than multi-threaded code: Only one branch executes per iteration
        // - Clear semantics: All concurrent logic visible in one place
        //
        // AT-LEAST-ONCE DELIVERY: Unprocessed batches are discarded on shutdown since they may not fully
        // have been processed and offsets haven't been committed.
        // They'll be re-processed on restart.
        // ========================================================================================
        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("Consumer received shutdown signal");
                    // Don't flush pending messages - they haven't been committed
                    // and will be re-read from the last committed offset on restart
                    // Just close the processor channel by dropping our sender
                    break;
                }
                // Handle acknowledgments from loader
                ack_msg = ack_receiver.recv() => {
                    match ack_msg {
                        Some(crate::consumer::messages::StreamMessage::Acknowledgment { offsets, success, error }) => {
                            if success {
                                if let Err(e) = self.commit_offsets(&offsets).await {
                                    error!(error = %e, "Failed to commit offsets after acknowledgment");
                                } else {
                                    debug!(offset_count = offsets.len(), "Committed offsets after successful processing");
                                }
                            } else {
                                error!(
                                    offset_count = offsets.len(),
                                    error = error.as_deref().unwrap_or("Unknown error"),
                                    "Not committing offsets due to processing failure"
                                );
                            }
                        }
                        Some(crate::consumer::messages::StreamMessage::End) | None => {
                            info!("Acknowledgment channel closed");
                            break;
                        }
                        _ => {
                            // Ignore other message types
                        }
                    }
                }
                message = message_stream.next() => {
                    match message {
                        Some(Ok(msg)) => {
                            info!(
                                topic = %msg.topic(),
                                partition = msg.partition(),
                                offset = msg.offset(),
                                "Received message from Kafka"
                            );
                            match self.parse_message(&msg) {
                                Ok(Some(pending)) => {
                                    batch.push(pending);
                                    pending_offsets.push((msg.topic().to_string(), msg.partition(), msg.offset()));

                                    // Flush if batch is full
                                    if batch.len() >= self.batch_size {
                                        let offsets_to_send = pending_offsets.clone();
                                        self.flush_batch(&batch, &offsets_to_send, &processor_tx).await?;
                                        batch.clear();
                                        pending_offsets.clear();
                                    }
                                }
                                Ok(None) => {
                                    // Message parsed but no events extracted
                                    // Unexpected message, commit offset immediately so,
                                    // We don't re-read this irrelevant message on restart
                                    // We don't hold up batch processing waiting for messages that have no work
                                    debug!(
                                        topic = %msg.topic(),
                                        partition = msg.partition(),
                                        offset = msg.offset(),
                                        "Message parsed but no events extracted"
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
                                        "Failed to parse message"
                                    );
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!(error = %e, "Kafka error");
                            // On Kafka error, we can't send to processor, just log and continue
                            // The error will be handled by retrying or shutting down
                        }
                        None => {
                            info!("Kafka stream ended");
                            // Flush any pending messages
                            if !batch.is_empty() {
                                let offsets_to_send = pending_offsets.clone();
                                self.flush_batch(&batch, &offsets_to_send, &processor_tx).await?;
                            }
                            // Channel will be closed when we drop processor_tx
                            break;
                        }
                    }
                }
                _ = flush_timer.tick() => {
                    // Flush if timeout reached and we have pending messages
                    if !batch.is_empty() {
                        debug!(count = batch.len(), "Flushing batch due to timeout");
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

    /// Flush a batch of pending messages to the channel.
    async fn flush_batch(
        &self,
        batch: &[PendingMessage],
        offsets: &[(String, i32, i64)],
        processor_tx: &mpsc::Sender<EntityProcessingBatch>,
    ) -> Result<(), IngestError> {
        if batch.is_empty() {
            return Ok(());
        }

        // Collect all events from the batch
        let mut all_events = Vec::new();
        for pending in batch {
            all_events.extend(pending.events.clone());
        }

        if !all_events.is_empty() {
            let event_count = all_events.len();
            info!(
                event_count = event_count,
                message_count = batch.len(),
                offset_count = offsets.len(),
                "Sending batch of events to processor"
            );
            processor_tx
                .send(EntityProcessingBatch {
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

    /// Parse a Kafka message into pending message data.
    fn parse_message(
        &self,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Result<Option<PendingMessage>, IngestError> {
        let payload = match msg.payload() {
            Some(p) => p,
            None => {
                debug!("Received message with empty payload");
                return Ok(None);
            }
        };

        let topic = msg.topic();
        let partition = msg.partition();
        let offset = msg.offset();

        debug!(
            topic = %topic,
            partition = partition,
            offset = offset,
            "Processing message"
        );

        // Parse the message based on topic
        let events = if topic == self.topics[0] {
            match self.parse_edit_message(payload, msg) {
                Ok(events) => events,
                Err(e) => {
                    error!(
                        topic = %topic,
                        partition = partition,
                        offset = offset,
                        error = %e,
                        "Failed to parse edit message"
                    );
                    return Err(e);
                }
            }
        } else {
            warn!(topic = %topic, "Unknown topic");
            return Ok(None);
        };

        if events.is_empty() {
            debug!(
                topic = %topic,
                partition = partition,
                offset = offset,
                "Message parsed but no events extracted (likely filtered out)"
            );
            return Ok(None);
        }

        Ok(Some(PendingMessage { events }))
    }

    /// Parse a HermesEdit message into entity events.
    fn parse_edit_message(
        &self,
        payload: &[u8],
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Result<Vec<EntityEvent>, IngestError> {
        let edit = HermesEdit::decode(payload)
            .map_err(|e| IngestError::parse(format!("Failed to decode HermesEdit: {}", e)))?;

        // Log the full message body for debugging
        debug!(
            topic = %msg.topic(),
            partition = msg.partition(),
            offset = msg.offset(),
            edit = ?edit,
            "Received knowledge.edits message"
        );

        // Parse space_id - it's a 16-byte UUID
        let space_id = if edit.space_id.len() == 16 {
            let bytes: [u8; 16] =
                edit.space_id.as_slice().try_into().map_err(|_| {
                    IngestError::parse("Failed to convert space_id bytes".to_string())
                })?;
            Uuid::from_bytes(bytes)
        } else {
            return Err(IngestError::parse(format!(
                "Invalid space_id length: expected 16 bytes, got {}",
                edit.space_id.len()
            )));
        };

        let mut events = Vec::new();
        let mut skipped_entities = 0;

        debug!(
            space_id = %space_id,
            edit_name = %edit.name,
            op_count = edit.ops.len(),
            "Parsing edit message"
        );

        // Process each operation in the edit
        for op in &edit.ops {
            if let Some(payload) = &op.payload {
                match payload {
                    Payload::UpdateEntity(entity) => {
                        if let Some(event) =
                            self.process_update_entity(entity, space_id, &edit, msg)
                        {
                            events.push(event);
                        } else {
                            skipped_entities += 1;
                        }
                    }
                    Payload::UnsetEntityValues(unset) => {
                        // Handle unsetting entity properties (e.g., clearing name/description)
                        if let Ok(id_bytes) = transform_id_bytes(unset.id.clone()) {
                            let entity_id = Uuid::from_bytes(id_bytes);

                            // Convert property IDs to property key names
                            let mut property_keys = Vec::new();
                            for property_bytes in &unset.properties {
                                if let Ok(prop_id_bytes) =
                                    transform_id_bytes(property_bytes.clone())
                                {
                                    let property_id = Uuid::from_bytes(prop_id_bytes).to_string();

                                    // Map known property IDs to their search index field names
                                    if property_id == NAME_PROPERTY_ID {
                                        property_keys.push("name".to_string());
                                    } else if property_id == DESCRIPTION_PROPERTY_ID {
                                        property_keys.push("description".to_string());
                                    } else if property_id == AVATAR_PROPERTY_ID {
                                        property_keys.push("avatar".to_string());
                                    }
                                    // Other properties are not indexed in search, so we ignore them
                                }
                            }

                            if !property_keys.is_empty() {
                                info!(
                                    entity_id = %entity_id,
                                    space_id = %space_id,
                                    property_keys = ?property_keys,
                                    "Unsetting entity properties"
                                );
                                events.push(EntityEvent::unset_properties(
                                    entity_id,
                                    space_id,
                                    property_keys,
                                ));
                            }
                        } else {
                            debug!(
                                entity_id_bytes = ?unset.id,
                                space_id = %space_id,
                                edit_name = %edit.name,
                                "Skipped unset entity values (invalid entity ID)"
                            );
                        }
                    }
                    Payload::CreateRelation(relation) => {
                        if let Some(event) =
                            self.process_create_relation(relation, space_id, &edit, msg)
                        {
                            events.push(event);
                        } else {
                            skipped_entities += 1;
                        }
                    }
                    Payload::UpdateRelation(relation_update) => {
                        if let Some(event) = self.process_update_relation_message(
                            relation_update,
                            space_id,
                            &edit,
                            msg,
                        ) {
                            events.push(event);
                        } else {
                            skipped_entities += 1;
                        }
                    }
                    Payload::DeleteRelation(relation_id_bytes) => {
                        if let Some(event) =
                            self.process_delete_relation(relation_id_bytes, space_id, &edit, msg)
                        {
                            events.push(event);
                        } else {
                            skipped_entities += 1;
                        }
                    }
                    Payload::UnsetRelationFields(_) => {
                        // Relation field unsets don't affect search index for now
                        debug!("Skipped unset relation fields (not relevant for search index)");
                    }
                    Payload::CreateProperty(_) => {
                        // Property creation doesn't affect search index
                        debug!("Skipped create property (not relevant for search index)");
                    }
                }
            }
        }

        if skipped_entities > 0 {
            debug!(
                skipped_count = skipped_entities,
                "Some entities were skipped during processing"
            );
        }

        Ok(events)
    }

    /// Process an UpdateEntity operation.
    fn process_update_entity(
        &self,
        entity: &wire::pb::grc20::Entity,
        space_id: Uuid,
        edit: &HermesEdit,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Option<EntityEvent> {
        let entity_id_bytes = match transform_id_bytes(entity.id.clone()) {
            Ok(bytes) => bytes,
            Err(_) => {
                // Log the full Kafka message when entity ID transformation fails
                let payload = msg.payload().unwrap_or(&[]);
                debug!(
                    topic = %msg.topic(),
                    partition = msg.partition(),
                    offset = msg.offset(),
                    key = ?msg.key(),
                    payload_len = payload.len(),
                    payload_bytes = ?payload,
                    entity_id_bytes = ?entity.id,
                    entity_values_count = entity.values.len(),
                    edit_name = %edit.name,
                    edit_id = ?edit.id,
                    space_id = %space_id,
                    "Skipped entity update (invalid entity ID)"
                );
                return None;
            }
        };
        let entity_id = Uuid::from_bytes(entity_id_bytes);

        // Extract name, description, and avatar from values
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut avatar: Option<String> = None;

        for value in &entity.values {
            let property_id_bytes = match transform_id_bytes(value.property.clone()) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };

            // Convert property ID bytes to UUID string for comparison against known IDs
            let property_id = Uuid::from_bytes(property_id_bytes).to_string();

            if property_id == NAME_PROPERTY_ID {
                name = Some(value.value.clone());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id,
                    name_value = %value.value,
                    "name property detected in property edit"
                );
            } else if property_id == DESCRIPTION_PROPERTY_ID {
                description = Some(value.value.clone());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id,
                    description_value = %value.value,
                    "description property detected in property edit"
                );
            } else if property_id == AVATAR_PROPERTY_ID {
                avatar = Some(value.value.clone());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id,
                    avatar_value = %value.value,
                    "avatar property detected in property edit"
                );
            }
        }

        Some(EntityEvent::upsert(
            entity_id,
            space_id,
            name,
            description,
            avatar,
        ))
    }

    /// Process a CreateRelation operation.
    fn process_create_relation(
        &self,
        relation: &Relation,
        space_id: Uuid,
        edit: &HermesEdit,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Option<EntityEvent> {
        let relation_id_bytes = match transform_id_bytes(relation.id.clone()) {
            Ok(bytes) => bytes,
            Err(_) => {
                debug!(
                    topic = %msg.topic(),
                    partition = msg.partition(),
                    offset = msg.offset(),
                    relation_id_bytes = ?relation.id,
                    edit_name = %edit.name,
                    space_id = %space_id,
                    "Skipped create relation (invalid relation ID)"
                );
                return None;
            }
        };
        let relation_id = Uuid::from_bytes(relation_id_bytes);

        let relation_type_bytes = match transform_id_bytes(relation.r#type.clone()) {
            Ok(bytes) => bytes,
            Err(_) => {
                debug!(
                    topic = %msg.topic(),
                    partition = msg.partition(),
                    offset = msg.offset(),
                    relation_id = %relation_id,
                    relation_type_bytes = ?relation.r#type,
                    edit_name = %edit.name,
                    space_id = %space_id,
                    "Skipped create relation (invalid relation type)"
                );
                return None;
            }
        };
        let relation_type = Uuid::from_bytes(relation_type_bytes);

        // Only process "type" relations (where from_entity has type of to_entity)
        if relation_type.to_string() != TYPE_RELATION_TYPE_ID {
            debug!(
                relation_id = %relation_id,
                relation_type = %relation_type,
                space_id = %space_id,
                "Skipped relation (not a type relation)"
            );
            return None;
        }

        let entity_id_bytes = match transform_id_bytes(relation.from_entity.clone()) {
            Ok(bytes) => bytes,
            Err(_) => {
                debug!(
                    topic = %msg.topic(),
                    partition = msg.partition(),
                    offset = msg.offset(),
                    relation_id = %relation_id,
                    entity_bytes = ?relation.from_entity,
                    edit_name = %edit.name,
                    space_id = %space_id,
                    "Skipped create relation (invalid entity ID)"
                );
                return None;
            }
        };
        let entity_id = Uuid::from_bytes(entity_id_bytes);

        let to_entity_id_bytes = match transform_id_bytes(relation.to_entity.clone()) {
            Ok(bytes) => bytes,
            Err(_) => {
                debug!(
                    topic = %msg.topic(),
                    partition = msg.partition(),
                    offset = msg.offset(),
                    relation_id = %relation_id,
                    to_entity_bytes = ?relation.to_entity,
                    edit_name = %edit.name,
                    space_id = %space_id,
                    "Skipped create relation (invalid to entity ID)"
                );
                return None;
            }
        };
        let to_entity_id = Uuid::from_bytes(to_entity_id_bytes);

        // This is a "type" relation - the from_entity has a type of to_entity
        // We need to update the from_entity's type_relations in the search index
        debug!(
            relation_id = %relation_id,
            relation_type = %relation_type,
            entity_id = %entity_id,
            to_entity_id = %to_entity_id,
            space_id = %space_id,
            "Processing type relation upsert - entity will have type added"
        );

        Some(EntityEvent::create_relation(
            relation_id,
            relation_type,
            entity_id,
            to_entity_id,
            space_id,
        ))
    }

    /// Process an UpdateRelation GRC20 message.
    ///
    /// Note: Relation updates are not supported in the search indexer. This function
    /// handles the protocol message but always returns None (skips the event).
    fn process_update_relation_message(
        &self,
        relation_update: &RelationUpdate,
        space_id: Uuid,
        edit: &HermesEdit,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Option<EntityEvent> {
        let relation_id_bytes = match transform_id_bytes(relation_update.id.clone()) {
            Ok(bytes) => bytes,
            Err(_) => {
                debug!(
                    topic = %msg.topic(),
                    partition = msg.partition(),
                    offset = msg.offset(),
                    relation_id_bytes = ?relation_update.id,
                    edit_name = %edit.name,
                    space_id = %space_id,
                    "Skipped UpdateRelation message (invalid relation ID)"
                );
                return None;
            }
        };
        let relation_id = Uuid::from_bytes(relation_id_bytes);

        // Relation updates are not supported - we only index type relations on create.
        // Skip all UpdateRelation messages.
        debug!(
            relation_id = %relation_id,
            space_id = %space_id,
            "Skipped UpdateRelation message (relation updates not supported)"
        );
        None
    }

    /// Process a DeleteRelation operation.
    ///
    /// For delete relations, we only have the relation_id. The downstream processor
    /// will handle finding and removing the relation from any affected entities.
    fn process_delete_relation(
        &self,
        relation_id_bytes: &[u8],
        space_id: Uuid,
        edit: &HermesEdit,
        msg: &rdkafka::message::BorrowedMessage<'_>,
    ) -> Option<EntityEvent> {
        let relation_id_bytes = match transform_id_bytes(relation_id_bytes.to_vec()) {
            Ok(bytes) => bytes,
            Err(_) => {
                debug!(
                    topic = %msg.topic(),
                    partition = msg.partition(),
                    offset = msg.offset(),
                    relation_id_bytes = ?relation_id_bytes,
                    edit_name = %edit.name,
                    space_id = %space_id,
                    "Skipped delete relation (invalid relation ID)"
                );
                return None;
            }
        };
        let relation_id = Uuid::from_bytes(relation_id_bytes);

        debug!(
            relation_id = %relation_id,
            space_id = %space_id,
            "Processing delete relation"
        );

        Some(EntityEvent::delete_relation(relation_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(EntitiesConsumer::KNOWLEDGE_EDITS_TOPIC, "knowledge.edits");
        assert_eq!(EntitiesConsumer::DEFAULT_BATCH_SIZE, 50);
        assert_eq!(EntitiesConsumer::DEFAULT_BATCH_TIMEOUT_MS, 1000);
    }
}
