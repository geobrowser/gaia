//! Processor module for the search indexer ingest.
//!
//! Transforms entity and score events into search documents.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};

use crate::consumer::StreamMessage;
use crate::consumer::{EntityEvent, EntityEventType, ScoreEvent, ScoreEventType};
use crate::errors::IngestError;
use crate::metrics::SearchIndexerMetrics;
use crate::orchestrator::{EntityProcessingBatch, ProcessedBatch, ScoreProcessingBatch};
use sdk::core::ids::TYPE_RELATION_TYPE_ID;
use search_indexer_shared::EntityDocument;
use uuid::Uuid;

/// Processed result from the entity processor.
#[derive(Debug)]
pub enum ProcessedEvent {
    /// Document to be indexed (create or update).
    Index(EntityDocument),
    /// Document to be deleted.
    Delete {
        entity_id: uuid::Uuid,
        space_id: uuid::Uuid,
    },
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
}

/// Processor that transforms entity and score events into search documents.
///
/// The processor is responsible for:
/// - Converting entity events to EntityDocument structures
/// - Converting score events to score update operations
/// - Filtering out events that shouldn't be indexed
/// - Enriching documents with additional metadata
pub struct Processor {
    // Could hold configuration or caches in the future
}

impl Processor {
    /// Create a new processor.
    pub fn new() -> Self {
        Self {}
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

    /// Run the processor task with both entity and score event processing.
    ///
    /// Receives batches from both consumers, processes them, and sends results to the loader.
    /// Returns a tokio task handle.
    pub fn run(
        self,
        mut entity_rx: mpsc::Receiver<EntityProcessingBatch>,
        mut scores_rx: mpsc::Receiver<ScoreProcessingBatch>,
        loader_tx: mpsc::Sender<ProcessedBatch>,
        entity_ack_tx: mpsc::Sender<StreamMessage>,
        scores_ack_tx: mpsc::Sender<StreamMessage>,
        metrics: Arc<SearchIndexerMetrics>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut entity_closed = false;
            let mut scores_closed = false;

            loop {
                // Exit when both channels are closed
                if entity_closed && scores_closed {
                    break;
                }

                // Use tokio::select to handle both channels
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
                }
            }
            debug!("Processor task shutting down");
        })
    }

    /// Handle a batch of entity events.
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
                    is_scores_batch: false,
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
                    is_scores_batch: true,
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
}
