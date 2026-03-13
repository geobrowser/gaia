//! Kafka consumer implementation for the search indexer.
//!
//! Consumes entity events from Kafka topics and forwards them to the ingest.

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
use uuid::Uuid;

use crate::consumer::messages::EntityEvent;
use crate::errors::IngestError;
use crate::orchestrator::EntityProcessingBatch;

use grc_20::decode_edit;
use hermes_schema::pb::knowledge::HermesEdit;
use sdk::core::ids::{
    AVATAR_RELATION_TYPE_ID, COVER_RELATION_TYPE_ID, DESCRIPTION_PROPERTY_ID,
    IMAGE_URL_PROPERTY_ID, NAME_PROPERTY_ID, TYPE_RELATION_TYPE_ID,
};

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
    const DEFAULT_BATCH_SIZE: usize = 10;

    /// Default batch timeout in milliseconds (configurable via KAFKA_BATCH_TIMEOUT_MS env var).
    const DEFAULT_BATCH_TIMEOUT_MS: u64 = 1000;

    /// Create a new Kafka consumer.
    ///
    /// Configuration is read from environment variables with fallbacks to defaults:
    /// - ENVIRONMENT: Environment name for topic prefix ("staging" or "production")
    /// - KAFKA_TOPIC: Base topic name (default: "knowledge.edits")
    /// - KAFKA_BATCH_SIZE: Batch size (default: 10)
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
        let prefix = get_topic_prefix();
        let base_topic =
            env::var("KAFKA_TOPIC").unwrap_or_else(|_| Self::KNOWLEDGE_EDITS_TOPIC.to_string());
        let topic = format!("{}{}", prefix, base_topic);

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

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("Consumer received shutdown signal");
                    break;
                }
                ack_msg = ack_receiver.recv() => {
                    match ack_msg {
                        Some(crate::consumer::messages::StreamMessage::Acknowledgment { offsets, success, error }) => {
                            if success {
                                let max_offset = offsets.iter().map(|(_, _, o)| *o).max().unwrap_or(0);
                                if let Err(e) = self.commit_offsets(&offsets).await {
                                    error!(error = %e, offset_count = offsets.len(), max_offset, "Failed to commit edits offsets after ACK");
                                } else {
                                    debug!(
                                        offset_count = offsets.len(),
                                        max_offset,
                                        "ACK: committed edits offsets"
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
                            debug!(
                                topic = %msg.topic(),
                                partition = msg.partition(),
                                offset = msg.offset(),
                                "Received message from Kafka"
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
                        }
                        None => {
                            info!("Kafka stream ended");
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

        debug!(
            topic = %msg.topic(),
            partition = msg.partition(),
            offset = msg.offset(),
            edit_name = %edit.name,
            "Received knowledge.edits message"
        );

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
                    let entity_id = Self::id_to_uuid(&del.id);
                    debug!(
                        entity_id = %entity_id,
                        space_id = %space_id,
                        edit_name = %edit.name,
                        "Processing delete entity"
                    );
                    events.push(EntityEvent::delete(entity_id, space_id));
                }
                grc_20::Op::RestoreEntity(restore) => {
                    let entity_id = Self::id_to_uuid(&restore.id);
                    debug!(
                        entity_id = %entity_id,
                        space_id = %space_id,
                        edit_name = %edit.name,
                        "Processing restore entity"
                    );
                    events.push(EntityEvent::restore(entity_id, space_id));
                }
                _ => {
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
    /// We extract name, description, and image_url from the values and create an upsert event.
    fn process_create_entity(
        &self,
        entity: &grc_20::CreateEntity,
        space_id: Uuid,
    ) -> Option<EntityEvent> {
        let entity_id = Self::id_to_uuid(&entity.id);

        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut image_url: Option<String> = None;

        for prop_value in &entity.values {
            let property_id = Self::id_to_uuid(&prop_value.property);
            let property_id_str = property_id.to_string();

            let value_str = match &prop_value.value {
                grc_20::Value::Text { value, .. } => value.as_ref(),
                _ => continue,
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
            } else if property_id_str == IMAGE_URL_PROPERTY_ID {
                image_url = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    image_url_value = %value_str,
                    "image_url property detected in CreateEntity"
                );
            }
        }

        debug!(
            entity_id = %entity_id,
            space_id = %space_id,
            has_name = name.is_some(),
            has_description = description.is_some(),
            has_image_url = image_url.is_some(),
            "Processing CreateEntity"
        );

        Some(EntityEvent::upsert(
            entity_id,
            space_id,
            name,
            description,
            None, // avatar is set via relations, not properties
            None, // cover is set via relations, not properties
            image_url,
        ))
    }

    /// Process an UpdateEntity operation.
    ///
    /// This may return multiple events when both set and unset are present:
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

                let field_name = if property_id_str == NAME_PROPERTY_ID {
                    Some("name")
                } else if property_id_str == DESCRIPTION_PROPERTY_ID {
                    Some("description")
                } else if property_id_str == IMAGE_URL_PROPERTY_ID {
                    Some("image_url")
                } else {
                    debug!(
                        entity_id = %entity_id,
                        space_id = %space_id,
                        property_id = %property_id_str,
                        "Skipping unset for unknown property ID"
                    );
                    None
                };

                if let Some(field) = field_name {
                    property_keys.push(field.to_string());
                    if field == "name" {
                        property_keys.push("name_raw".to_string());
                    }
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

        // Extract name, description, and image_url from set_properties
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut image_url: Option<String> = None;

        for prop_value in &entity.set_properties {
            let property_id = Self::id_to_uuid(&prop_value.property);
            let property_id_str = property_id.to_string();

            let value_str = match &prop_value.value {
                grc_20::Value::Text { value, .. } => value.as_ref(),
                _ => continue,
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
            } else if property_id_str == IMAGE_URL_PROPERTY_ID {
                image_url = Some(value_str.to_string());
                debug!(
                    entity_id = %entity_id,
                    space_id = %space_id,
                    property_id = %property_id_str,
                    image_url_value = %value_str,
                    "image_url property detected in property edit"
                );
            }
        }

        if !entity.set_properties.is_empty() {
            events.push(EntityEvent::upsert(
                entity_id,
                space_id,
                name,
                description,
                None, // avatar is set via relations
                None, // cover is set via relations
                image_url,
            ));
        }

        events
    }

    /// Process a CreateRelation operation.
    ///
    /// Handles three indexed relation types:
    /// - TYPE_RELATION_TYPE_ID: type relations (entity has type of to_entity)
    /// - AVATAR_RELATION_TYPE_ID: avatar relations (entity's avatar is the image at to_entity)
    /// - COVER_RELATION_TYPE_ID: cover relations (entity's cover is the image at to_entity)
    fn process_create_relation(
        &self,
        relation: &grc_20::CreateRelation,
        space_id: Uuid,
    ) -> Option<EntityEvent> {
        let relation_id = Self::id_to_uuid(&relation.id);
        let relation_type = Self::id_to_uuid(&relation.relation_type);
        let relation_type_str = relation_type.to_string();

        let is_type = relation_type_str == TYPE_RELATION_TYPE_ID;
        let is_avatar = relation_type_str == AVATAR_RELATION_TYPE_ID;
        let is_cover = relation_type_str == COVER_RELATION_TYPE_ID;

        if !is_type && !is_avatar && !is_cover {
            debug!(
                relation_id = %relation_id,
                relation_type = %relation_type,
                space_id = %space_id,
                "Skipped relation (not an indexed relation type)"
            );
            return None;
        }

        let entity_id = Self::id_to_uuid(&relation.from);
        let to_entity_id = Self::id_to_uuid(&relation.to);

        debug!(
            relation_id = %relation_id,
            relation_type = %relation_type,
            entity_id = %entity_id,
            to_entity_id = %to_entity_id,
            space_id = %space_id,
            "Processing indexed relation"
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
    /// Note: GRC-20 UpdateRelation only updates mutable fields (space pins, version pins,
    /// position), not structural fields (from, to, relation_type). Since we don't index
    /// those mutable fields, we skip all UpdateRelation messages.
    fn process_update_relation_message(
        &self,
        relation_update: &grc_20::UpdateRelation,
        space_id: Uuid,
    ) -> Option<EntityEvent> {
        let relation_id = Self::id_to_uuid(&relation_update.id);

        debug!(
            relation_id = %relation_id,
            space_id = %space_id,
            "Skipped UpdateRelation message (relation updates not supported)"
        );
        None
    }

    /// Process a DeleteRelation operation.
    fn process_delete_relation(&self, relation_id: &Uuid, space_id: Uuid) -> Option<EntityEvent> {
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

    /// Create a consumer for testing without requiring the ENVIRONMENT env var.
    /// Bypasses `get_topic_prefix()` by using `with_batch_config()` directly.
    fn test_consumer() -> EntitiesConsumer {
        EntitiesConsumer::with_batch_config(
            "localhost:9092",
            "test-group",
            "knowledge.edits".to_string(),
            EntitiesConsumer::DEFAULT_BATCH_SIZE,
            EntitiesConsumer::DEFAULT_BATCH_TIMEOUT_MS,
        )
        .expect("Failed to create test EntitiesConsumer")
    }

    #[test]
    fn test_constants() {
        assert_eq!(EntitiesConsumer::KNOWLEDGE_EDITS_TOPIC, "knowledge.edits");
        assert_eq!(EntitiesConsumer::DEFAULT_BATCH_SIZE, 10);
        assert_eq!(EntitiesConsumer::DEFAULT_BATCH_TIMEOUT_MS, 1000);
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_single_property() {
        let consumer = test_consumer();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![UnsetValue {
                property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                language: UnsetLanguage::All,
            }],
            context: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        assert_eq!(result.len(), 1);
        let event = &result[0];
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(event.unset_property_keys, vec!["name", "name_raw"]);
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_multiple_properties() {
        let consumer = test_consumer();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![
                UnsetValue {
                    property: *Uuid::parse_str(NAME_PROPERTY_ID).unwrap().as_bytes(),
                    language: UnsetLanguage::All,
                },
                UnsetValue {
                    property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID).unwrap().as_bytes(),
                    language: UnsetLanguage::All,
                },
            ],
            context: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        assert_eq!(result.len(), 1);
        let event = &result[0];
        assert_eq!(event.entity_id, entity_id);
        assert_eq!(event.space_id, space_id);
        assert_eq!(event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(event.unset_property_keys.len(), 3);
        assert!(event.unset_property_keys.contains(&"name".to_string()));
        assert!(event.unset_property_keys.contains(&"name_raw".to_string()));
        assert!(event
            .unset_property_keys
            .contains(&"description".to_string()));
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_unknown_property() {
        let consumer = test_consumer();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

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

        let result = consumer.process_update_entity(&update_entity, space_id);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_with_set_properties() {
        let consumer = test_consumer();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

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
                property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID).unwrap().as_bytes(),
                language: UnsetLanguage::All,
            }],
            context: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);

        assert_eq!(result.len(), 2);

        let unset_event = &result[0];
        assert_eq!(unset_event.entity_id, entity_id);
        assert_eq!(unset_event.space_id, space_id);
        assert_eq!(unset_event.event_type, EntityEventType::UnsetProperties);
        assert_eq!(unset_event.unset_property_keys, vec!["description"]);

        let upsert_event = &result[1];
        assert_eq!(upsert_event.entity_id, entity_id);
        assert_eq!(upsert_event.space_id, space_id);
        assert_eq!(upsert_event.event_type, EntityEventType::Upsert);
        assert_eq!(upsert_event.name, Some("New Name".to_string()));
    }

    #[tokio::test]
    async fn test_process_update_entity_unset_empty() {
        let consumer = test_consumer();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let update_entity = grc_20::UpdateEntity {
            id: *entity_id.as_bytes(),
            set_properties: vec![],
            unset_values: vec![],
            context: None,
        };

        let result = consumer.process_update_entity(&update_entity, space_id);
        assert!(result.is_empty());
    }

    // ==================== CreateEntity Tests ====================

    #[tokio::test]
    async fn test_process_create_entity_with_name() {
        let consumer = test_consumer();
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
        let consumer = test_consumer();
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
                    property: *Uuid::parse_str(IMAGE_URL_PROPERTY_ID).unwrap().as_bytes(),
                    value: grc_20::Value::Text {
                        value: "https://example.com/image.png".into(),
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
        assert_eq!(
            event.image_url,
            Some("https://example.com/image.png".to_string())
        );
        // Avatar is not set from properties anymore — it comes via relations
        assert_eq!(event.avatar, None);
    }

    #[tokio::test]
    async fn test_process_create_entity_empty_values() {
        let consumer = test_consumer();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

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
        assert_eq!(event.image_url, None);
    }

    #[tokio::test]
    async fn test_process_create_entity_unknown_properties() {
        let consumer = test_consumer();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let unknown_property_id = Uuid::new_v4();

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
        assert_eq!(event.name, None);
        assert_eq!(event.description, None);
        assert_eq!(event.avatar, None);
    }

    // ==================== CreateRelation Tests ====================

    #[tokio::test]
    async fn test_process_create_relation_avatar() {
        let consumer = test_consumer();
        let space_id = Uuid::new_v4();
        let image_entity_id = Uuid::new_v4();

        let relation = grc_20::CreateRelation {
            id: *Uuid::new_v4().as_bytes(),
            relation_type: *Uuid::parse_str(AVATAR_RELATION_TYPE_ID).unwrap().as_bytes(),
            from: *Uuid::new_v4().as_bytes(),
            to: *image_entity_id.as_bytes(),
            from_is_value_ref: false,
            from_space: None,
            from_version: None,
            to_is_value_ref: false,
            to_space: None,
            to_version: None,
            entity: None,
            position: None,
            context: None,
        };

        let result = consumer.process_create_relation(&relation, space_id);

        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.event_type, EntityEventType::CreateRelation);
        assert_eq!(event.to_entity_id, Some(image_entity_id));
    }

    #[tokio::test]
    async fn test_process_create_relation_type() {
        let consumer = test_consumer();
        let space_id = Uuid::new_v4();

        let relation = grc_20::CreateRelation {
            id: *Uuid::new_v4().as_bytes(),
            relation_type: *Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap().as_bytes(),
            from: *Uuid::new_v4().as_bytes(),
            to: *Uuid::new_v4().as_bytes(),
            from_is_value_ref: false,
            from_space: None,
            from_version: None,
            to_is_value_ref: false,
            to_space: None,
            to_version: None,
            entity: None,
            position: None,
            context: None,
        };

        let result = consumer.process_create_relation(&relation, space_id);

        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.event_type, EntityEventType::CreateRelation);
    }

    #[tokio::test]
    async fn test_process_create_relation_non_indexed() {
        let consumer = test_consumer();
        let space_id = Uuid::new_v4();

        let relation = grc_20::CreateRelation {
            id: *Uuid::new_v4().as_bytes(),
            relation_type: *Uuid::new_v4().as_bytes(), // Random non-indexed type
            from: *Uuid::new_v4().as_bytes(),
            to: *Uuid::new_v4().as_bytes(),
            from_is_value_ref: false,
            from_space: None,
            from_version: None,
            to_is_value_ref: false,
            to_space: None,
            to_version: None,
            entity: None,
            position: None,
            context: None,
        };

        let result = consumer.process_create_relation(&relation, space_id);
        assert!(result.is_none());
    }
}
