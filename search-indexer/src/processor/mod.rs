//! Processor module for the search indexer ingest.
//!
//! Transforms entity and score events into search documents.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use hermes_instrumentation::{debug, error, info, instrument, warn};

use crate::consumer::StreamMessage;
use crate::consumer::{EntityEvent, EntityEventType, ScoreEvent, ScoreEventType, SpaceTopicEvent};
use crate::errors::IngestError;
use crate::metrics::SearchIndexerMetrics;
use crate::orchestrator::{BatchSource, EntityProcessingBatch, ProcessedBatch, ScoreProcessingBatch, SpaceTopicProcessingBatch};
use sdk::core::ids::TYPE_RELATION_TYPE_ID;
use search_indexer_shared::EntityDocument;
use uuid::Uuid;

/// Processed result from the entity processor.
#[derive(Debug)]
pub enum ProcessedEvent {
    /// Document to be indexed (create or update).
    /// For soft deletes, the document will have `deleted=Some(true)`.
    Index(EntityDocument),
    /// Properties to be unset from a document.
    UnsetProperties {
        entity_id: uuid::Uuid,
        space_id: uuid::Uuid,
        property_keys: Vec<String>,
    },
    /// Add a type relation to an entity's type_relations array.
    AddTypeRelation {
        entity_id: uuid::Uuid,
        space_id: uuid::Uuid,
        relation_id: uuid::Uuid,
        entity_to_id: uuid::Uuid,
    },
    /// Remove a type relation from any entity containing it, using only the relation_id.
    /// Used when we don't know which entity contains the relation.
    RemoveTypeRelationById { relation_id: uuid::Uuid },
    /// Update an entity's global score across all spaces.
    /// This will update entity_global_score for all documents with this entity_id.
    UpdateEntityGlobalScore { entity_id: uuid::Uuid, score: f64 },
    /// Update a space's score.
    /// This will update space_score for all documents in this space.
    UpdateSpaceScore { space_id: uuid::Uuid, score: f64 },
    /// Update an entity's score within a specific space.
    /// This is the most targeted update - affects exactly one document.
    UpdateEntitySpaceScore {
        entity_id: uuid::Uuid,
        space_id: uuid::Uuid,
        score: f64,
    },
    /// Update space_topic_entity_id for all entities in a space.
    /// This records which entity represents the space (its topic entity).
    UpdateSpaceTopicEntityId {
        space_id: uuid::Uuid,
        topic_entity_id: uuid::Uuid,
    },
}

/// Processor that transforms entity and score events into search documents.
///
/// The processor is responsible for:
/// - Converting entity events to EntityDocument structures
/// - Converting score events to score update operations
/// - Filtering out events that shouldn't be indexed
/// - Enriching documents with additional metadata (e.g., space_topic_entity_id from cache)
pub struct Processor {
    /// In-memory cache of space_id → topic_entity_id.
    /// Used to set space_topic_entity_id on new entity documents during upserts.
    space_topic_cache: HashMap<Uuid, Uuid>,
}

impl Processor {
    /// Create a new processor with an empty space topic cache.
    pub fn new() -> Self {
        Self {
            space_topic_cache: HashMap::new(),
        }
    }

    /// Create a new processor with a pre-warmed space topic cache.
    pub fn with_space_topic_cache(cache: HashMap<Uuid, Uuid>) -> Self {
        info!(cache_size = cache.len(), "Processor created with space topic cache");
        Self {
            space_topic_cache: cache,
        }
    }

    /// Check if a relation type represents an entity type relationship.
    ///
    /// Returns true if the relation type ID matches the "type" relation type ID,
    /// which indicates that the from_entity has a type of to_entity.
    fn is_type_relation(&self, relation_type: &Uuid) -> bool {
        relation_type.to_string() == TYPE_RELATION_TYPE_ID
    }

    /// Process a batch of entity events.
    ///
    /// # Arguments
    ///
    /// * `events` - The events to process
    ///
    /// # Returns
    ///
    /// A vector of processed events ready for loading.
    #[instrument(skip(self, events), fields(event_count = events.len()))]
    pub fn process_batch(
        &self,
        events: Vec<EntityEvent>,
    ) -> Result<Vec<ProcessedEvent>, IngestError> {
        let mut processed = Vec::with_capacity(events.len());

        for event in events {
            if let Some(result) = self.process_event(event)? {
                processed.push(result);
            }
        }

        debug!(processed_count = processed.len(), "Processed event batch");
        Ok(processed)
    }

    /// Process a batch of score events.
    ///
    /// # Arguments
    ///
    /// * `events` - The score events to process
    ///
    /// # Returns
    ///
    /// A vector of processed events ready for loading.
    #[instrument(skip(self, events), fields(event_count = events.len()))]
    pub fn process_score_batch(
        &self,
        events: Vec<ScoreEvent>,
    ) -> Result<Vec<ProcessedEvent>, IngestError> {
        let mut processed = Vec::with_capacity(events.len());

        for event in events {
            processed.push(self.process_score_event(event)?);
        }

        debug!(
            processed_count = processed.len(),
            "Processed score event batch"
        );
        Ok(processed)
    }

    /// Process a batch of space topic events.
    ///
    /// For each event this:
    /// 1. Updates the in-memory cache so subsequent entity upserts get `space_topic_entity_id` set.
    /// 2. Emits an `UpdateSpaceTopicEntityId` to backfill existing docs via `update_by_query`.
    /// 3. Upserts a stub document for the topic entity itself. This ensures the mapping
    ///    survives indexer restarts (the cache warm-up query will find this document) even
    ///    if no other entities in the space have been indexed yet. When the full entity data
    ///    arrives via `knowledge.edits`, the upsert merges into this stub.
    #[instrument(skip(self, events), fields(event_count = events.len()))]
    pub fn process_space_topic_batch(
        &mut self,
        events: Vec<SpaceTopicEvent>,
    ) -> Result<Vec<ProcessedEvent>, IngestError> {
        let mut processed = Vec::with_capacity(events.len() * 2);

        for event in events {
            // Update the cache before creating the processed events
            self.space_topic_cache
                .insert(event.space_id, event.topic_entity_id);

            // Backfill all existing documents in this space
            processed.push(ProcessedEvent::UpdateSpaceTopicEntityId {
                space_id: event.space_id,
                topic_entity_id: event.topic_entity_id,
            });

            // Upsert a stub document for the topic entity itself. This is
            // necessary for two reasons:
            //
            // 1. Restart resilience: if the indexer restarts before any entities
            //    in this space are indexed, the in-memory cache is lost. The
            //    cache warm-up query (`get_space_topic_mappings`) reads
            //    `space_topic_entity_id` from the index — without this stub
            //    document, there would be nothing to warm from.
            //
            // 2. Pipeline emit ordering: within a single block, hermes-pipeline
            //    emits `space.topics` BEFORE `knowledge.edits`, so the topic
            //    entity's full data (name, description, avatar) could arrive
            //    AFTER this event. The stub is created now and the full entity
            //    data merges into it via upsert when `knowledge.edits` is
            //    processed.
            let mut doc = EntityDocument::new(
                event.topic_entity_id,
                event.space_id,
                None,
                None,
            );
            doc.space_topic_entity_id = Some(event.topic_entity_id.to_string());
            processed.push(ProcessedEvent::Index(doc));
        }

        debug!(
            processed_count = processed.len(),
            "Processed space topic event batch"
        );
        Ok(processed)
    }

    /// Process a single score event.
    fn process_score_event(&self, event: ScoreEvent) -> Result<ProcessedEvent, IngestError> {
        match event.event_type {
            ScoreEventType::EntityGlobalScore => {
                let entity_id = event.entity_id.ok_or_else(|| {
                    error!("EntityGlobalScore event missing entity_id");
                    IngestError::parse("EntityGlobalScore event missing entity_id".to_string())
                })?;
                if !event.score.is_finite() {
                    error!(
                        entity_id = %entity_id,
                        score = event.score,
                        "EntityGlobalScore event contains invalid score (NaN or infinite)"
                    );
                    return Err(IngestError::parse(format!(
                        "invalid score: NaN or infinite (value: {})",
                        event.score
                    )));
                }
                Ok(ProcessedEvent::UpdateEntityGlobalScore {
                    entity_id,
                    score: event.score,
                })
            }
            ScoreEventType::SpaceScore => {
                let space_id = event.space_id.ok_or_else(|| {
                    error!("SpaceScore event missing space_id");
                    IngestError::parse("SpaceScore event missing space_id".to_string())
                })?;
                if !event.score.is_finite() {
                    error!(
                        space_id = %space_id,
                        score = event.score,
                        "SpaceScore event contains invalid score (NaN or infinite)"
                    );
                    return Err(IngestError::parse(format!(
                        "invalid score: NaN or infinite (value: {})",
                        event.score
                    )));
                }
                Ok(ProcessedEvent::UpdateSpaceScore {
                    space_id,
                    score: event.score,
                })
            }
            ScoreEventType::EntitySpaceScore => {
                let entity_id = event.entity_id.ok_or_else(|| {
                    error!("EntitySpaceScore event missing entity_id");
                    IngestError::parse("EntitySpaceScore event missing entity_id".to_string())
                })?;
                let space_id = event.space_id.ok_or_else(|| {
                    error!("EntitySpaceScore event missing space_id");
                    IngestError::parse("EntitySpaceScore event missing space_id".to_string())
                })?;
                if !event.score.is_finite() {
                    error!(
                        entity_id = %entity_id,
                        space_id = %space_id,
                        score = event.score,
                        "EntitySpaceScore event contains invalid score (NaN or infinite)"
                    );
                    return Err(IngestError::parse(format!(
                        "invalid score: NaN or infinite (value: {})",
                        event.score
                    )));
                }
                Ok(ProcessedEvent::UpdateEntitySpaceScore {
                    entity_id,
                    space_id,
                    score: event.score,
                })
            }
        }
    }

    /// Run the processor task with entity, score, and space topic event processing.
    ///
    /// Receives batches from all consumers, processes them, and sends results to the loader.
    /// Returns a tokio task handle.
    pub fn run(
        mut self,
        mut entity_rx: mpsc::Receiver<EntityProcessingBatch>,
        mut scores_rx: mpsc::Receiver<ScoreProcessingBatch>,
        mut space_topics_rx: mpsc::Receiver<SpaceTopicProcessingBatch>,
        loader_tx: mpsc::Sender<ProcessedBatch>,
        entity_ack_tx: mpsc::Sender<StreamMessage>,
        scores_ack_tx: mpsc::Sender<StreamMessage>,
        space_topics_ack_tx: mpsc::Sender<StreamMessage>,
        metrics: Arc<SearchIndexerMetrics>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut entity_closed = false;
            let mut scores_closed = false;
            let mut space_topics_closed = false;

            loop {
                // Exit when all channels are closed
                if entity_closed && scores_closed && space_topics_closed {
                    break;
                }

                // Use tokio::select to handle all channels
                tokio::select! {
                    // Handle entity events
                    entity_batch = entity_rx.recv(), if !entity_closed => {
                        match entity_batch {
                            Some(batch) => {
                                self.handle_entity_batch(
                                    batch,
                                    &loader_tx,
                                    &entity_ack_tx,
                                    &metrics,
                                ).await;
                            }
                            None => {
                                warn!("Entity processor channel closed");
                                entity_closed = true;
                            }
                        }
                    }
                    // Handle score events
                    score_batch = scores_rx.recv(), if !scores_closed => {
                        match score_batch {
                            Some(batch) => {
                                self.handle_score_batch(
                                    batch,
                                    &loader_tx,
                                    &scores_ack_tx,
                                    &metrics,
                                ).await;
                            }
                            None => {
                                warn!("Scores processor channel closed");
                                scores_closed = true;
                            }
                        }
                    }
                    // Handle space topic events
                    space_topic_batch = space_topics_rx.recv(), if !space_topics_closed => {
                        match space_topic_batch {
                            Some(batch) => {
                                self.handle_space_topic_batch(
                                    batch,
                                    &loader_tx,
                                    &space_topics_ack_tx,
                                    &metrics,
                                ).await;
                            }
                            None => {
                                warn!("Space topics processor channel closed");
                                space_topics_closed = true;
                            }
                        }
                    }
                }
            }
            debug!("Processor task shutting down");
        })
    }

    /// Handle a batch of entity events.
    #[instrument(skip(self, batch, loader_tx, ack_tx, metrics), fields(event_count = batch.event_count))]
    async fn handle_entity_batch(
        &self,
        batch: EntityProcessingBatch,
        loader_tx: &mpsc::Sender<ProcessedBatch>,
        ack_tx: &mpsc::Sender<StreamMessage>,
        metrics: &Arc<SearchIndexerMetrics>,
    ) {
        let EntityProcessingBatch {
            events,
            offsets,
            event_count,
        } = batch;

        match self.process_batch(events) {
            Ok(processed_events) => {
                metrics
                    .total_events_processed
                    .fetch_add(event_count as u64, Ordering::Relaxed);

                if processed_events.is_empty() {
                    debug!("No documents to index after processing, sending ACK directly");
                    if let Err(send_err) = ack_tx
                        .send(StreamMessage::Acknowledgment {
                            offsets,
                            success: true,
                            error: None,
                        })
                        .await
                    {
                        error!(error = %send_err, "Failed to send acknowledgment - channel closed");
                    }
                    return;
                }

                let index_count = processed_events
                    .iter()
                    .filter(|e| matches!(e, ProcessedEvent::Index(_)))
                    .count();

                let processed_batch = ProcessedBatch {
                    events: processed_events,
                    offsets,
                    index_count,
                    source: BatchSource::Entity,
                };

                if let Err(send_err) = loader_tx.send(processed_batch).await {
                    error!(error = %send_err, "Failed to send batch to loader - channel closed");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to process events, sending NACK to consumer");
                if let Err(send_err) = ack_tx
                    .send(StreamMessage::Acknowledgment {
                        offsets,
                        success: false,
                        error: Some(e.to_string()),
                    })
                    .await
                {
                    error!(error = %send_err, "Failed to send failure acknowledgment - channel closed");
                }
            }
        }
    }

    /// Handle a batch of score events.
    #[instrument(skip(self, batch, loader_tx, ack_tx, metrics), fields(event_count = batch.event_count))]
    async fn handle_score_batch(
        &self,
        batch: ScoreProcessingBatch,
        loader_tx: &mpsc::Sender<ProcessedBatch>,
        ack_tx: &mpsc::Sender<StreamMessage>,
        metrics: &Arc<SearchIndexerMetrics>,
    ) {
        let ScoreProcessingBatch {
            events,
            offsets,
            event_count,
        } = batch;

        match self.process_score_batch(events) {
            Ok(processed_events) => {
                metrics
                    .total_events_processed
                    .fetch_add(event_count as u64, Ordering::Relaxed);

                if processed_events.is_empty() {
                    debug!("No score updates to index, sending ACK directly");
                    if let Err(send_err) = ack_tx
                        .send(StreamMessage::Acknowledgment {
                            offsets,
                            success: true,
                            error: None,
                        })
                        .await
                    {
                        error!(error = %send_err, "Failed to send scores acknowledgment - channel closed");
                    }
                    return;
                }

                // Score updates count as operations, not index operations
                let processed_batch = ProcessedBatch {
                    events: processed_events,
                    offsets,
                    index_count: 0, // Score updates don't count as new documents
                    source: BatchSource::Score,
                };

                if let Err(send_err) = loader_tx.send(processed_batch).await {
                    error!(error = %send_err, "Failed to send score batch to loader - channel closed");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to process score events, sending NACK to consumer");
                if let Err(send_err) = ack_tx
                    .send(StreamMessage::Acknowledgment {
                        offsets,
                        success: false,
                        error: Some(e.to_string()),
                    })
                    .await
                {
                    error!(error = %send_err, "Failed to send scores failure acknowledgment - channel closed");
                }
            }
        }
    }

    /// Handle a batch of space topic events.
    #[instrument(skip(self, batch, loader_tx, ack_tx, metrics), fields(event_count = batch.event_count))]
    async fn handle_space_topic_batch(
        &mut self,
        batch: SpaceTopicProcessingBatch,
        loader_tx: &mpsc::Sender<ProcessedBatch>,
        ack_tx: &mpsc::Sender<StreamMessage>,
        metrics: &Arc<SearchIndexerMetrics>,
    ) {
        let SpaceTopicProcessingBatch {
            events,
            offsets,
            event_count,
        } = batch;

        match self.process_space_topic_batch(events) {
            Ok(processed_events) => {
                metrics
                    .total_events_processed
                    .fetch_add(event_count as u64, Ordering::Relaxed);

                if processed_events.is_empty() {
                    debug!("No space topic updates to index, sending ACK directly");
                    if let Err(send_err) = ack_tx
                        .send(StreamMessage::Acknowledgment {
                            offsets,
                            success: true,
                            error: None,
                        })
                        .await
                    {
                        error!(error = %send_err, "Failed to send space topics acknowledgment - channel closed");
                    }
                    return;
                }

                let processed_batch = ProcessedBatch {
                    events: processed_events,
                    offsets,
                    index_count: 0,
                    source: BatchSource::SpaceTopic,
                };

                if let Err(send_err) = loader_tx.send(processed_batch).await {
                    error!(error = %send_err, "Failed to send space topic batch to loader - channel closed");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to process space topic events, sending NACK to consumer");
                if let Err(send_err) = ack_tx
                    .send(StreamMessage::Acknowledgment {
                        offsets,
                        success: false,
                        error: Some(e.to_string()),
                    })
                    .await
                {
                    error!(error = %send_err, "Failed to send space topics failure acknowledgment - channel closed");
                }
            }
        }
    }

    /// Process a single entity event.
    fn process_event(&self, event: EntityEvent) -> Result<Option<ProcessedEvent>, IngestError> {
        match event.event_type {
            EntityEventType::Upsert => {
                // Names are now optional - index entities even without names
                let mut doc = EntityDocument::new(
                    event.entity_id,
                    event.space_id,
                    event.name,
                    event.description,
                );

                // Set optional fields
                doc.avatar = event.avatar;
                doc.cover = event.cover;

                // Look up space_topic_entity_id from cache
                if let Some(topic_entity_id) = self.space_topic_cache.get(&event.space_id) {
                    doc.space_topic_entity_id = Some(topic_entity_id.to_string());
                }

                Ok(Some(ProcessedEvent::Index(doc)))
            }
            EntityEventType::Delete => {
                // Soft delete: create a document with deleted=true
                // The upsert will preserve existing fields and only update the deleted flag
                let mut doc = EntityDocument::new(
                    event.entity_id,
                    event.space_id,
                    None,  // Name not needed for delete
                    None,  // Description not needed for delete
                );
                doc.deleted = Some(true);

                Ok(Some(ProcessedEvent::Index(doc)))
            }
            EntityEventType::Restore => {
                // Restore: create a document with deleted=false to un-delete
                // The upsert will preserve existing fields and only update the deleted flag
                let mut doc = EntityDocument::new(
                    event.entity_id,
                    event.space_id,
                    None,  // Name not needed for restore
                    None,  // Description not needed for restore
                );
                doc.deleted = Some(false);

                Ok(Some(ProcessedEvent::Index(doc)))
            }
            EntityEventType::UnsetProperties => {
                if event.unset_property_keys.is_empty() {
                    // No properties to unset, skip
                    return Ok(None);
                }
                Ok(Some(ProcessedEvent::UnsetProperties {
                    entity_id: event.entity_id,
                    space_id: event.space_id,
                    property_keys: event.unset_property_keys,
                }))
            }
            EntityEventType::CreateRelation => {
                if let (Some(relation_id), Some(relation_type), Some(to_entity_id)) =
                    (event.relation_id, event.relation_type, event.to_entity_id)
                {
                    if self.is_type_relation(&relation_type) {
                        // This is a type relation - we need to add it to type_relations
                        debug!(
                            entity_id = %event.entity_id,
                            relation_id = %relation_id,
                            to_entity_id = %to_entity_id,
                            space_id = %event.space_id,
                            "Processing type relation upsert - adding to entity's type_relations"
                        );

                        Ok(Some(ProcessedEvent::AddTypeRelation {
                            entity_id: event.entity_id,
                            space_id: event.space_id,
                            relation_id,
                            entity_to_id: to_entity_id,
                        }))
                    } else {
                        debug!(
                            relation_type = %relation_type,
                            "Skipped non-type relation upsert"
                        );
                        Ok(None)
                    }
                } else {
                    debug!("Skipped create relation event with missing fields");
                    Ok(None)
                }
            }
            EntityEventType::DeleteRelation => {
                // For delete relations, we may not know which entity contains the relation.
                // We only need the relation_id to perform the removal.
                if let Some(relation_id) = event.relation_id {
                    debug!(
                        relation_id = %relation_id,
                        "Processing relation delete"
                    );
                    Ok(Some(ProcessedEvent::RemoveTypeRelationById { relation_id }))
                } else {
                    debug!("Skipped delete relation event with missing relation_id");
                    Ok(None)
                }
            }
        }
    }
}

impl Default for Processor {
    fn default() -> Self {
        Self::new()
    }
}

impl Processor {
    /// Get the current size of the space topic cache (for testing/metrics).
    #[cfg(test)]
    pub fn space_topic_cache_len(&self) -> usize {
        self.space_topic_cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdk::core::ids::TYPE_RELATION_TYPE_ID;
    use uuid::Uuid;

    #[test]
    fn test_process_upsert_event() {
        let processor = Processor::new();

        let event = EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            Some("Description".to_string()),
            None,
        );

        let result = processor.process_event(event).unwrap();
        assert!(matches!(result, Some(ProcessedEvent::Index(_))));

        if let Some(ProcessedEvent::Index(doc)) = result {
            assert_eq!(doc.name, Some("Test Entity".to_string()));
            assert_eq!(doc.description, Some("Description".to_string()));
        }
    }

    #[test]
    fn test_process_delete_event() {
        let processor = Processor::new();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let event = EntityEvent::delete(entity_id, space_id);

        let result = processor.process_event(event).unwrap();
        assert!(matches!(result, Some(ProcessedEvent::Index(_))));

        if let Some(ProcessedEvent::Index(doc)) = result {
            assert_eq!(doc.entity_id, entity_id);
            assert_eq!(doc.space_id, space_id);
            assert_eq!(doc.deleted, Some(true));
            assert_eq!(doc.name, None);
            assert_eq!(doc.description, None);
        }
    }

    #[test]
    fn test_process_restore_event() {
        let processor = Processor::new();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let event = EntityEvent::restore(entity_id, space_id);

        let result = processor.process_event(event).unwrap();
        assert!(matches!(result, Some(ProcessedEvent::Index(_))));

        if let Some(ProcessedEvent::Index(doc)) = result {
            assert_eq!(doc.entity_id, entity_id);
            assert_eq!(doc.space_id, space_id);
            assert_eq!(doc.deleted, Some(false));
            assert_eq!(doc.name, None);
            assert_eq!(doc.description, None);
        }
    }

    #[test]
    fn test_process_entity_without_name() {
        let processor = Processor::new();

        let event = EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            None, // No name
            None,
            None,
        );

        let result = processor.process_event(event).unwrap();
        assert!(matches!(result, Some(ProcessedEvent::Index(_))));

        if let Some(ProcessedEvent::Index(doc)) = result {
            assert!(doc.name.is_none());
        }
    }

    #[test]
    fn test_process_batch() {
        let processor = Processor::new();

        let events = vec![
            EntityEvent::upsert(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 1".to_string()),
                None,
                None,
            ),
            EntityEvent::upsert(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 2".to_string()),
                Some("Desc".to_string()),
                None,
            ),
            EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        ];

        let results = processor.process_batch(events).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_process_create_relation_non_type() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();
        let relation_type = Uuid::new_v4(); // Non-type relation
        let entity_id = Uuid::new_v4();
        let to_entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let event = EntityEvent::create_relation(
            relation_id,
            relation_type,
            entity_id,
            to_entity_id,
            space_id,
        );

        let result = processor.process_event(event).unwrap();
        // Non-type relations should be skipped
        assert!(result.is_none());
    }

    #[test]
    fn test_process_create_relation_type() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();
        // Use the actual type relation ID
        let relation_type =
            Uuid::parse_str(TYPE_RELATION_TYPE_ID).expect("TYPE_RELATION_TYPE_ID should be valid");
        let entity_id = Uuid::new_v4();
        let to_entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let event = EntityEvent::create_relation(
            relation_id,
            relation_type,
            entity_id,
            to_entity_id,
            space_id,
        );

        // Since this is a type relation, it should return an AddTypeRelation event
        let result = processor.process_event(event).unwrap();
        assert!(result.is_some());
        assert!(matches!(
            result,
            Some(ProcessedEvent::AddTypeRelation { .. })
        ));

        if let Some(ProcessedEvent::AddTypeRelation {
            entity_id: eid,
            space_id: sid,
            relation_id: rid,
            entity_to_id: etid,
        }) = result
        {
            assert_eq!(eid, entity_id);
            assert_eq!(sid, space_id);
            assert_eq!(rid, relation_id);
            assert_eq!(etid, to_entity_id);
        }
    }

    #[test]
    fn test_process_delete_type_relation() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();

        // delete_relation should produce RemoveTypeRelationById
        let event = EntityEvent::delete_relation(relation_id);

        let result = processor.process_event(event).unwrap();
        assert!(result.is_some());
        assert!(matches!(
            result,
            Some(ProcessedEvent::RemoveTypeRelationById { .. })
        ));

        if let Some(ProcessedEvent::RemoveTypeRelationById { relation_id: rid }) = result {
            assert_eq!(rid, relation_id);
        }
    }

    #[test]
    fn test_is_type_relation() {
        let processor = Processor::new();

        // Random relation type should not be a type relation
        let random_relation_type = Uuid::new_v4();
        assert!(!processor.is_type_relation(&random_relation_type));

        // The specific type relation ID should be recognized
        let type_relation_id =
            Uuid::parse_str(TYPE_RELATION_TYPE_ID).expect("TYPE_RELATION_TYPE_ID should be valid");
        assert!(processor.is_type_relation(&type_relation_id));
    }

    #[test]
    fn test_process_upsert_with_space_topic_cache() {
        // Pre-warm cache with a space→topic mapping
        let space_id = Uuid::new_v4();
        let topic_entity_id = Uuid::new_v4();
        let mut cache = HashMap::new();
        cache.insert(space_id, topic_entity_id);

        let processor = Processor::with_space_topic_cache(cache);

        let event = EntityEvent::upsert(
            Uuid::new_v4(),
            space_id,
            Some("Test Entity".to_string()),
            None,
            None,
        );

        let result = processor.process_event(event).unwrap();
        assert!(matches!(result, Some(ProcessedEvent::Index(_))));

        if let Some(ProcessedEvent::Index(doc)) = result {
            assert_eq!(
                doc.space_topic_entity_id,
                Some(topic_entity_id.to_string()),
                "Upsert for a cached space should have space_topic_entity_id set"
            );
        }
    }

    #[test]
    fn test_process_upsert_without_space_topic_cache() {
        // Empty cache — no space_topic_entity_id should be set
        let processor = Processor::new();

        let event = EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            None,
            None,
        );

        let result = processor.process_event(event).unwrap();
        assert!(matches!(result, Some(ProcessedEvent::Index(_))));

        if let Some(ProcessedEvent::Index(doc)) = result {
            assert_eq!(
                doc.space_topic_entity_id, None,
                "Upsert for an uncached space should have space_topic_entity_id None"
            );
        }
    }

    #[test]
    fn test_space_topic_batch_updates_cache_and_creates_stub() {
        let mut processor = Processor::new();
        assert_eq!(processor.space_topic_cache_len(), 0);

        let space_id = Uuid::new_v4();
        let topic_entity_id = Uuid::new_v4();

        // Process a space topic event
        let events = vec![SpaceTopicEvent {
            space_id,
            topic_entity_id,
        }];
        let processed = processor.process_space_topic_batch(events).unwrap();

        // Should emit 2 events: UpdateSpaceTopicEntityId + Index stub
        assert_eq!(processed.len(), 2);
        assert!(matches!(
            processed[0],
            ProcessedEvent::UpdateSpaceTopicEntityId { .. }
        ));

        // Verify the stub document
        if let ProcessedEvent::Index(ref doc) = processed[1] {
            assert_eq!(doc.entity_id, topic_entity_id);
            assert_eq!(doc.space_id, space_id);
            assert_eq!(
                doc.space_topic_entity_id,
                Some(topic_entity_id.to_string()),
                "Stub document should have space_topic_entity_id set to itself"
            );
            assert!(doc.name.is_none(), "Stub document should have no name");
            assert!(doc.description.is_none(), "Stub document should have no description");
        } else {
            panic!("Expected ProcessedEvent::Index for stub document");
        }

        // Cache should now have one entry
        assert_eq!(processor.space_topic_cache_len(), 1);

        // Now upsert an entity for that space — it should pick up the topic
        let event = EntityEvent::upsert(
            Uuid::new_v4(),
            space_id,
            Some("New Entity".to_string()),
            None,
            None,
        );

        let result = processor.process_event(event).unwrap();
        if let Some(ProcessedEvent::Index(doc)) = result {
            assert_eq!(
                doc.space_topic_entity_id,
                Some(topic_entity_id.to_string()),
                "After processing a SpaceTopicEvent, subsequent upserts should have the topic set"
            );
        } else {
            panic!("Expected ProcessedEvent::Index");
        }
    }
}
