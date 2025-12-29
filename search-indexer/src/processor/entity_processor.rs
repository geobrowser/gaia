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
use search_indexer_shared::EntityDocument;

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
}
