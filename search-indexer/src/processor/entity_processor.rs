//! Entity processor implementation.
//!
//! Transforms entity events into EntityDocument structures for indexing.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument};

use crate::consumer::StreamMessage;
use crate::consumer::{EntityEvent, EntityEventType};
use crate::errors::IngestError;
use crate::metrics::SearchIndexerMetrics;
use crate::orchestrator::{ProcessedBatch, ProcessingBatch};
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
}

/// Processor that transforms entity events into search documents.
///
/// The processor is responsible for:
/// - Converting entity events to EntityDocument structures
/// - Filtering out events that shouldn't be indexed
/// - Enriching documents with additional metadata
pub struct EntityProcessor {
    // Could hold configuration or caches in the future
}

impl EntityProcessor {
    /// Create a new entity processor.
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

    /// Run the processor task.
    ///
    /// Receives batches from the consumer, processes them, and sends results to the loader.
    /// Returns a tokio task handle.
    pub fn run(
        self,
        mut processor_rx: mpsc::Receiver<ProcessingBatch>,
        loader_tx: mpsc::Sender<ProcessedBatch>,
        ack_tx: mpsc::Sender<StreamMessage>,
        metrics: Arc<SearchIndexerMetrics>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(batch) = processor_rx.recv().await {
                let ProcessingBatch {
                    events,
                    offsets,
                    event_count,
                } = batch;

                match self.process_batch(events) {
                    Ok(processed_events) => {
                        // Update event processing metrics
                        metrics
                            .total_events_processed
                            .fetch_add(event_count as u64, Ordering::Relaxed);

                        if processed_events.is_empty() {
                            debug!("No documents to index after processing, sending ACK directly");
                            // Send immediate ACK since there's nothing to load
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
                            continue;
                        }

                        // Calculate index count for metrics
                        let index_count = processed_events
                            .iter()
                            .filter(|e| matches!(e, ProcessedEvent::Index(_)))
                            .count();

                        // Send processed batch to loader
                        let processed_batch = ProcessedBatch {
                            events: processed_events,
                            offsets,
                            index_count,
                        };

                        if let Err(send_err) = loader_tx.send(processed_batch).await {
                            error!(error = %send_err, "Failed to send batch to loader - channel closed");
                            break;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to process events, sending NACK to consumer");
                        // On processing error, send NACK to consumer
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
            debug!("Processor task shutting down");
        })
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
            EntityEventType::Delete => Ok(Some(ProcessedEvent::Delete {
                entity_id: event.entity_id,
                space_id: event.space_id,
            })),
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
                    // TODO: Right now we don't have a way to lookup metadata about a relation
                    // during a delete. So we have to try deleting from the search index on
                    // every relation deleted, even though the relations might not be in the index.

                    // If we have relation_type info, check if it's a type relation
                    // If not, we assume it might be a type relation (conservative approach)
                    // let should_process = event
                    //     .relation_type
                    //     .map(|rt| self.is_type_relation(&rt))
                    //     .unwrap_or(true); // If no relation_type, assume it might be a type relation

                    // if should_process {
                    debug!(
                        relation_id = %relation_id,
                        "Processing relation delete"
                    );
                    Ok(Some(ProcessedEvent::RemoveTypeRelationById { relation_id }))
                    // } else {
                    //     debug!(
                    //         relation_type = ?event.relation_type,
                    //         "Skipped non-type relation delete"
                    //     );
                    //     Ok(None)
                    // }
                } else {
                    debug!("Skipped delete relation event with missing relation_id");
                    Ok(None)
                }
            }
        }
    }
}

impl Default for EntityProcessor {
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
        let processor = EntityProcessor::new();

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
        let processor = EntityProcessor::new();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let event = EntityEvent::delete(entity_id, space_id);

        let result = processor.process_event(event).unwrap();
        assert!(matches!(result, Some(ProcessedEvent::Delete { .. })));

        if let Some(ProcessedEvent::Delete {
            entity_id: eid,
            space_id: sid,
        }) = result
        {
            assert_eq!(eid, entity_id);
            assert_eq!(sid, space_id);
        }
    }

    #[test]
    fn test_process_entity_without_name() {
        let processor = EntityProcessor::new();

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
        let processor = EntityProcessor::new();

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
        let processor = EntityProcessor::new();

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
        let processor = EntityProcessor::new();

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
        use sdk::core::ids::TYPE_RELATION_TYPE_ID;

        let processor = EntityProcessor::new();

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
        let processor = EntityProcessor::new();

        // Random relation type should not be a type relation
        let random_relation_type = Uuid::new_v4();
        assert!(!processor.is_type_relation(&random_relation_type));

        // The specific type relation ID should be recognized
        let type_relation_id =
            Uuid::parse_str(TYPE_RELATION_TYPE_ID).expect("TYPE_RELATION_TYPE_ID should be valid");
        assert!(processor.is_type_relation(&type_relation_id));
    }
}
