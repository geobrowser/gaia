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
use hermes_instrumentation::{debug, error, info, info_span, instrument, warn, Instrument};
use uuid::Uuid;

use crate::consumer::messages::EntityEvent;
use crate::errors::IngestError;
use crate::orchestrator::EntityProcessingBatch;

use hermes_schema::pb::knowledge::HermesEdit;
use sdk::core::ids::{
    AVATAR_PROPERTY_ID, DESCRIPTION_PROPERTY_ID, NAME_PROPERTY_ID, TYPE_RELATION_TYPE_ID,
};
use grc_20::decode_edit;

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
            let first_offset = offsets.first().map(|(_, _, o)| *o).unwrap_or(0);
            let last_offset = offsets.last().map(|(_, _, o)| *o).unwrap_or(0);

            async {
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
                    .map_err(|e| IngestError::ChannelError(e.to_string()))
            }
            .instrument(info_span!(
                "search_indexer.consume_entities_batch",
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

    /// Decode GRC2/GRC2Z payload bytes into a grc_20::Edit
    fn decode_payload<'a>(payload: &'a [u8]) -> Result<grc_20::Edit<'a>, IngestError> {
        if payload.is_empty() {
            return Err(IngestError::parse("Empty payload".to_string()));
        }

        decode_edit(payload)
            .map_err(|e| IngestError::parse(format!("GRC-20 decode error: {:?}", e)))
    }

    /// Convert a grc_20::Id (16 bytes) to a Uuid
    fn id_to_uuid(id: &grc_20::Id) -> Uuid {
        Uuid::from_bytes(*id)
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

        // Decode the GRC-20 payload
        let grc20_edit = Self::decode_payload(&edit.payload)?;

        let mut events = Vec::new();
        let mut skipped_entities = 0;

        debug!(
            space_id = %space_id,
            edit_name = %edit.name,
            op_count = grc20_edit.ops.len(),
            "Parsing edit message"
        );

        // Process each operation in the edit
        for op in &grc20_edit.ops {
            match op {
                grc_20::Op::CreateEntity(entity) => {
                    if let Some(event) = self.process_create_entity(entity, space_id) {
                        events.push(event);
                    } else {
                        skipped_entities += 1;
                    }
                }
                grc_20::Op::UpdateEntity(entity) => {
                    let entity_events = self.process_update_entity(entity, space_id);
                    if entity_events.is_empty() {
                        skipped_entities += 1;
                    } else {
                        events.extend(entity_events);
                    }
                }
                // Note: UnsetEntityFields not available in grc-20 0.1.6
                // Handle unset fields via UpdateEntity with unset_properties
                grc_20::Op::CreateRelation(relation) => {
                    if let Some(event) = self.process_create_relation(relation, space_id) {
                        events.push(event);
                    } else {
                        skipped_entities += 1;
                    }
                }
                grc_20::Op::UpdateRelation(relation_update) => {
                    if let Some(event) =
                        self.process_update_relation_message(relation_update, space_id)
                    {
                        events.push(event);
                    } else {
                        skipped_entities += 1;
                    }
                }
                grc_20::Op::DeleteRelation(del) => {
                    let relation_id = Self::id_to_uuid(&del.id);
                    if let Some(event) = self.process_delete_relation(&relation_id, space_id) {
                        events.push(event);
                    } else {
                        skipped_entities += 1;
                    }
                }
                grc_20::Op::DeleteEntity(del) => {
                    // Handle entity deletion (soft delete)
                    let entity_id = Self::id_to_uuid(&del.id);
                    info!(
                        entity_id = %entity_id,
                        space_id = %space_id,
                        edit_name = %edit.name,
                        "Processing delete entity"
                    );
                    events.push(EntityEvent::delete(entity_id, space_id));
                }
                _ => {
                    // Other operations (RestoreEntity, RestoreRelation, CreateValueRef) not yet implemented
                    debug!("Skipped operation (not yet implemented)");
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

    /// Process a CreateEntity operation.
    ///
    /// CreateEntity initializes a new entity with optional property values.
    /// We extract name, description, and avatar from the values and create an upsert event.
    fn process_create_entity(
        &self,
        entity: &grc_20::CreateEntity,
        space_id: Uuid,
    ) -> Option<EntityEvent> {
        let entity_id = Self::id_to_uuid(&entity.id);

        // Extract name, description, and avatar from values
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut avatar: Option<String> = None;

        for prop_value in &entity.values {
            let property_id = Self::id_to_uuid(&prop_value.property);
            let property_id_str = property_id.to_string();

            // Extract the string value from the grc_20::Value enum
            let value_str = match &prop_value.value {
                grc_20::Value::Text { value, .. } => value.as_ref(),
                _ => continue, // Skip non-string values
            };

            if property_id_str == NAME_PROPERTY_ID {
                name = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    name_value = %value_str,
                    "name property detected in CreateEntity"
                );
            } else if property_id_str == DESCRIPTION_PROPERTY_ID {
                description = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    description_value = %value_str,
                    "description property detected in CreateEntity"
                );
            } else if property_id_str == AVATAR_PROPERTY_ID {
                avatar = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    avatar_value = %value_str,
                    "avatar property detected in CreateEntity"
                );
            }
        }

        info!(
            entity_id = %entity_id,
            space_id = %space_id,
            has_name = name.is_some(),
            has_description = description.is_some(),
            has_avatar = avatar.is_some(),
            "Processing CreateEntity"
        );

        // Always create an upsert event for CreateEntity - even without properties,
        // we want to create the entity document with entity_id and space_id
        Some(EntityEvent::upsert(
            entity_id,
            space_id,
            name,
            description,
            avatar,
        ))
    }

    /// Process an UpdateEntity operation.
    ///
    /// According to GRC-20 v2 spec, UpdateEntity can contain both set_properties and unset_values.
    /// The GRC-20 library validates that a property cannot appear in both set_properties and
    /// unset_values in the same operation (invalid at wire format level).
    ///
    /// This may return multiple events when both set and unset are present (for different properties):
    /// - UnsetProperties event for properties to clear
    /// - Upsert event for properties to set
    fn process_update_entity(
        &self,
        entity: &grc_20::UpdateEntity,
        space_id: Uuid,
    ) -> Vec<EntityEvent> {
        let entity_id = Self::id_to_uuid(&entity.id);
        let mut events = Vec::new();

        // Handle unset_values
        if !entity.unset_values.is_empty() {
            let mut property_keys: Vec<String> = Vec::new();

            for unset_val in &entity.unset_values {
                let property_id = Self::id_to_uuid(&unset_val.property);
                let property_id_str = property_id.to_string();

                // Map known property UUIDs to their field names in OpenSearch
                let field_name = if property_id_str == NAME_PROPERTY_ID {
                    Some("name")
                } else if property_id_str == DESCRIPTION_PROPERTY_ID {
                    Some("description")
                } else if property_id_str == AVATAR_PROPERTY_ID {
                    Some("avatar")
                } else {
                    // Unknown property - skip it
                    debug!(
                        entity_id = %entity_id,
                        space_id = %space_id,
                        property_id = %property_id_str,
                        "Skipping unset for unknown property ID"
                    );
                    None
                };

                if let Some(name) = field_name {
                    property_keys.push(name.to_string());
                }
            }

            if !property_keys.is_empty() {
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_count = property_keys.len(),
                    properties = ?property_keys,
                    "Unset properties operation detected"
                );

                events.push(EntityEvent::unset_properties(
                    entity_id,
                    space_id,
                    property_keys,
                ));
            }
        }

        // Extract name, description, and avatar from set_properties
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut avatar: Option<String> = None;

        for prop_value in &entity.set_properties {
            let property_id = Self::id_to_uuid(&prop_value.property);
            let property_id_str = property_id.to_string();

            // Extract the string value from the grc_20::Value enum
            let value_str = match &prop_value.value {
                grc_20::Value::Text { value, .. } => value.as_ref(),
                _ => continue, // Skip non-string values
            };

            if property_id_str == NAME_PROPERTY_ID {
                name = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    name_value = %value_str,
                    "name property detected in property edit"
                );
            } else if property_id_str == DESCRIPTION_PROPERTY_ID {
                description = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    description_value = %value_str,
                    "description property detected in property edit"
                );
            } else if property_id_str == AVATAR_PROPERTY_ID {
                avatar = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    avatar_value = %value_str,
                    "avatar property detected in property edit"
                );
            }
        }

        // Always create an upsert event if there are set_properties
        // This handles both pure upserts and mixed set/unset operations
        if !entity.set_properties.is_empty() {
            events.push(EntityEvent::upsert(
                entity_id,
                space_id,
                name,
                description,
                avatar,
            ));
        }

        events
    }

    /// Process a CreateRelation operation.
    fn process_create_relation(
        &self,
        relation: &grc_20::CreateRelation,
        space_id: Uuid,
    ) -> Option<EntityEvent> {
        let relation_id = Self::id_to_uuid(&relation.id);
        let relation_type = Self::id_to_uuid(&relation.relation_type);

        // Only process "type" relations (where from has type of to)
        if relation_type.to_string() != TYPE_RELATION_TYPE_ID {
            debug!(
                relation_id = %relation_id,
                relation_type = %relation_type,
                space_id = %space_id,
                "Skipped relation (not a type relation)"
            );
            return None;
        }

        let entity_id = Self::id_to_uuid(&relation.from);
        let to_entity_id = Self::id_to_uuid(&relation.to);

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
        relation_update: &grc_20::UpdateRelation,
        space_id: Uuid,
    ) -> Option<EntityEvent> {
        let relation_id = Self::id_to_uuid(&relation_update.id);

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
        relation_id: &Uuid,
        space_id: Uuid,
    ) -> Option<EntityEvent> {
        debug!(
            relation_id = %relation_id,
            space_id = %space_id,
            "Processing delete relation"
        );

        Some(EntityEvent::delete_relation(*relation_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer::EntityEventType;
    use grc_20::{CreateEntity, PropertyValue, UnsetLanguage, UnsetValue};

    #[test]
    fn test_constants() {
        assert_eq!(EntitiesConsumer::KNOWLEDGE_EDITS_TOPIC, "knowledge.edits");
        assert_eq!(EntitiesConsumer::DEFAULT_BATCH_SIZE, 50);
        assert_eq!(EntitiesConsumer::DEFAULT_BATCH_TIMEOUT_MS, 1000);
    }

    // Helper to create a dummy message reference (unused in process_update_entity)
    #[tokio::test]
    async fn test_process_update_entity_unset_single_property() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create UpdateEntity with unset_values for name property
        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![UnsetValue {
                property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                language: UnsetLanguage::All,
            }],
            context: None,
        };

        let _edit = HermesEdit {
            id: Uuid::new_v4().as_bytes().to_vec(),
            name: "Test Unset".to_string(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: space_id.as_bytes().to_vec(),
            is_canonical: true,
            meta: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        assert_eq!(result.len(), 1);
        let event = &result[0];
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(event.unset_property_keys, vec!["name"]);
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_multiple_properties() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create UpdateEntity with unset_values for name and description properties
        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![
                UnsetValue {
                    property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                    language: UnsetLanguage::All,
                },
                UnsetValue {
                    property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID)
                        .unwrap()
                        .as_bytes(),
                    language: UnsetLanguage::All,
                },
            ],
            context: None,
        };

        let _edit = HermesEdit {
            id: Uuid::new_v4().as_bytes().to_vec(),
            name: "Test Unset Multiple".to_string(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: space_id.as_bytes().to_vec(),
            is_canonical: true,
            meta: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        assert_eq!(result.len(), 1);
        let event = &result[0];
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(event.unset_property_keys.len(), 2);
        assert!(event.unset_property_keys.contains(&"name".to_string()));
        assert!(event
            .unset_property_keys
            .contains(&"description".to_string()));
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_unknown_property() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create UpdateEntity with unset_values for an unknown property
        let unknown_property_id = Uuid::new_v4();
        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![UnsetValue {
                property: *unknown_property_id.as_bytes(),
                language: UnsetLanguage::All,
            }],
            context: None,
        };

        let _edit = HermesEdit {
            id: Uuid::new_v4().as_bytes().to_vec(),
            name: "Test Unset Unknown".to_string(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: space_id.as_bytes().to_vec(),
            is_canonical: true,
            meta: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        // Should return empty vec because no recognized properties to unset
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_with_set_properties() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create UpdateEntity with BOTH set_properties and unset_values
        // This should fall through to upsert logic (not unset)
        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![PropertyValue {
                property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                value: grc_20::Value::Text {
                    value: "New Name".into(),
                    language: None,
                },
            }],
            unset_values: vec![UnsetValue {
                property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID)
                    .unwrap()
                    .as_bytes(),
                language: UnsetLanguage::All,
            }],
            context: None,
        };

        let _edit = HermesEdit {
            id: Uuid::new_v4().as_bytes().to_vec(),
            name: "Test Mixed Operation".to_string(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: space_id.as_bytes().to_vec(),
            is_canonical: true,
            meta: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        // Should return 2 events: UnsetProperties for description, then Upsert for name
        assert_eq!(result.len(), 2);

        // First event: Unset description
        let unset_event = &result[0];
        assert_eq!(unset_event.entity_id, entity_id);
        assert_eq!(unset_event.space_id, space_id);
        assert_eq!(unset_event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(unset_event.unset_property_keys, vec!["description"]);

        // Second event: Upsert with name
        let upsert_event = &result[1];
        assert_eq!(upsert_event.entity_id, entity_id);
        assert_eq!(upsert_event.space_id, space_id);
        assert_eq!(upsert_event.event_type, EntityEventType::Upsert);
        assert_eq!(upsert_event.name, Some("New Name".to_string()));
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_empty() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create UpdateEntity with empty unset_values
        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![],
            context: None,
        };

        let _edit = HermesEdit {
            id: Uuid::new_v4().as_bytes().to_vec(),
            name: "Test Empty Unset".to_string(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: space_id.as_bytes().to_vec(),
            is_canonical: true,
            meta: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        // Should return empty vec (no set_properties, no unset_values to process)
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_avatar() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create UpdateEntity with unset_values for avatar property
        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![UnsetValue {
                property: *Uuid::parse_str(AVATAR_PROPERTY_ID).unwrap().as_bytes(),
                language: UnsetLanguage::All,
            }],
            context: None,
        };

        let _edit = HermesEdit {
            id: Uuid::new_v4().as_bytes().to_vec(),
            name: "Test Unset Avatar".to_string(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: space_id.as_bytes().to_vec(),
            is_canonical: true,
            meta: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        assert_eq!(result.len(), 1);
        let event = &result[0];
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(event.unset_property_keys, vec!["avatar"]);
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_all_three_properties() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create UpdateEntity with unset_values for all three properties
        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![
                UnsetValue {
                    property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                    language: UnsetLanguage::All,
                },
                UnsetValue {
                    property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID)
                        .unwrap()
                        .as_bytes(),
                    language: UnsetLanguage::All,
                },
                UnsetValue {
                    property: *Uuid::parse_str(AVATAR_PROPERTY_ID).unwrap().as_bytes(),
                    language: UnsetLanguage::All,
                },
            ],
            context: None,
        };

        let _edit = HermesEdit {
            id: Uuid::new_v4().as_bytes().to_vec(),
            name: "Test Unset All".to_string(),
            payload: vec![],
            authors: vec![],
            language: None,
            space_id: space_id.as_bytes().to_vec(),
            is_canonical: true,
            meta: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        assert_eq!(result.len(), 1);
        let event = &result[0];
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(event.unset_property_keys.len(), 3);
        assert!(event.unset_property_keys.contains(&"name".to_string()));
        assert!(event
            .unset_property_keys
            .contains(&"description".to_string()));
        assert!(event.unset_property_keys.contains(&"avatar".to_string()));
    }

    // ==================== CreateEntity Tests ====================

    #[tokio::test]
    async fn test_process_create_entity_with_name() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let create_entity = CreateEntity {
            id: *entity_id.as_bytes(),
            values: vec![PropertyValue {
                property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                value: grc_20::Value::Text {
                    value: "Test Entity".into(),
                    language: None,
                },
            }],
            context: None,
        };

        let result = consumer.process_create_entity(&create_entity, space_id);

        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::Upsert);
        assert_eq!(event.name, Some("Test Entity".to_string()));
        assert_eq!(event.description, None);
        assert_eq!(event.avatar, None);
    }

    #[tokio::test]
    async fn test_process_create_entity_with_all_properties() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let create_entity = CreateEntity {
            id: *entity_id.as_bytes(),
            values: vec![
                PropertyValue {
                    property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                    value: grc_20::Value::Text {
                        value: "Test Entity".into(),
                        language: None,
                    },
                },
                PropertyValue {
                    property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID).unwrap().as_bytes(),
                    value: grc_20::Value::Text {
                        value: "A test description".into(),
                        language: None,
                    },
                },
                PropertyValue {
                    property: *Uuid::parse_str(AVATAR_PROPERTY_ID).unwrap().as_bytes(),
                    value: grc_20::Value::Text {
                        value: "https://example.com/avatar.png".into(),
                        language: None,
                    },
                },
            ],
            context: None,
        };

        let result = consumer.process_create_entity(&create_entity, space_id);

        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::Upsert);
        assert_eq!(event.name, Some("Test Entity".to_string()));
        assert_eq!(event.description, Some("A test description".to_string()));
        assert_eq!(event.avatar, Some("https://example.com/avatar.png".to_string()));
    }

    #[tokio::test]
    async fn test_process_create_entity_empty_values() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // CreateEntity with no property values - should still create the entity
        let create_entity = CreateEntity {
            id: *entity_id.as_bytes(),
            values: vec![],
            context: None,
        };

        let result = consumer.process_create_entity(&create_entity, space_id);

        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::Upsert);
        assert_eq!(event.name, None);
        assert_eq!(event.description, None);
        assert_eq!(event.avatar, None);
    }

    #[tokio::test]
    async fn test_process_create_entity_unknown_properties() {
        let consumer = EntitiesConsumer::new("localhost:9092", "test-group").unwrap();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let unknown_property_id = Uuid::new_v4();

        // CreateEntity with unknown property - should create entity but ignore unknown property
        let create_entity = CreateEntity {
            id: *entity_id.as_bytes(),
            values: vec![PropertyValue {
                property: *unknown_property_id.as_bytes(),
                value: grc_20::Value::Text {
                    value: "Unknown Value".into(),
                    language: None,
                },
            }],
            context: None,
        };

        let result = consumer.process_create_entity(&create_entity, space_id);

        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::Upsert);
        // Unknown properties are ignored, so no name/description/avatar
        assert_eq!(event.name, None);
        assert_eq!(event.description, None);
        assert_eq!(event.avatar, None);
    }

}
