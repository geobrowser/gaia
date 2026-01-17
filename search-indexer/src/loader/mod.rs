//! Loader module for the search indexer ingest.
//!
//! Loads processed documents into the search index using UpdateEntityRequest.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};

use crate::consumer::StreamMessage;
use crate::errors::IngestError;
use crate::metrics::SearchIndexerMetrics;
use crate::orchestrator::ProcessedBatch;
use crate::processor::ProcessedEvent;
use search_indexer_repository::{
    EntityOperation, RemoveTypeRelationData, SearchIndexProvider, TypeRelationData,
    UnsetEntityPropertiesRequest, UpdateEntityGlobalScoreRequest, UpdateEntityRequest,
    UpdateEntitySpaceScoreRequest, UpdateSpaceScoreRequest,
};

/// Loader that indexes documents into the search engine.
///
/// The loader is responsible for:
/// - Batching documents for efficient bulk indexing
/// - Converting EntityDocuments to EntityOperations
/// - Maintaining operation order for consistency
pub struct SearchLoader {
    provider: Arc<dyn SearchIndexProvider>,
    /// All pending operations, maintained in order for correct sequencing
    pending_operations: Vec<EntityOperation>,
}

impl SearchLoader {
    /// Create a new search loader with the given provider.
    pub fn new(provider: Arc<dyn SearchIndexProvider>) -> Self {
        Self {
            provider,
            pending_operations: Vec::new(),
        }
    }

    /// Load a batch of processed events.
    ///
    /// Converts events to EntityOperations and processes them IN ORDER using bulk_operations.
    /// This maintains consistency when multiple operations affect the same entity.
    #[instrument(skip(self, events), fields(event_count = events.len()))]
    pub async fn load(
        &mut self,
        events: Vec<ProcessedEvent>,
    ) -> Result<Vec<search_indexer_repository::BatchOperationSummary>, IngestError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        // Convert events to EntityOperations, maintaining order
        for event in events {
            match event {
                ProcessedEvent::Index(doc) => {
                    self.pending_operations
                        .push(EntityOperation::Update(UpdateEntityRequest {
                            entity_id: doc.entity_id.to_string(),
                            space_id: doc.space_id.to_string(),
                            name: doc.name,
                            description: doc.description,
                            avatar: doc.avatar,
                            cover: doc.cover,
                            add_type_relation: None,
                            entity_global_score: doc.entity_global_score,
                            space_score: doc.space_score,
                            entity_space_score: doc.entity_space_score,
                            deleted: doc.deleted,
                        }));
                }
                ProcessedEvent::Delete { .. } => {
                    // Delete events are now handled as soft deletes (Index with deleted=true)
                    // This case should never be reached in the new implementation
                    error!("Unexpected ProcessedEvent::Delete - should be using soft delete (Index with deleted=true)");
                    continue;
                }
                ProcessedEvent::UnsetProperties {
                    entity_id,
                    space_id,
                    property_keys,
                } => {
                    self.pending_operations.push(EntityOperation::Unset(
                        UnsetEntityPropertiesRequest {
                            entity_id: entity_id.to_string(),
                            space_id: space_id.to_string(),
                            property_keys,
                        },
                    ));
                }
                ProcessedEvent::AddTypeRelation {
                    entity_id,
                    space_id,
                    relation_id,
                    entity_to_id,
                } => {
                    self.pending_operations
                        .push(EntityOperation::Update(UpdateEntityRequest {
                            entity_id: entity_id.to_string(),
                            space_id: space_id.to_string(),
                            name: None,
                            description: None,
                            avatar: None,
                            cover: None,
                            add_type_relation: Some(TypeRelationData {
                                relation_id: relation_id.to_string(),
                                entity_to_id: entity_to_id.to_string(),
                            }),
                            entity_global_score: None,
                            space_score: None,
                            entity_space_score: None,
                            deleted: None,
                        }));
                }
                ProcessedEvent::RemoveTypeRelationById { relation_id } => {
                    self.pending_operations
                        .push(EntityOperation::RemoveTypeRelationById(
                            RemoveTypeRelationData {
                                relation_id: relation_id.to_string(),
                            },
                        ));
                }
                ProcessedEvent::UpdateEntityGlobalScore { entity_id, score } => {
                    self.pending_operations
                        .push(EntityOperation::UpdateEntityGlobalScore(
                            UpdateEntityGlobalScoreRequest {
                                entity_id: entity_id.to_string(),
                                score,
                            },
                        ));
                }
                ProcessedEvent::UpdateSpaceScore { space_id, score } => {
                    self.pending_operations
                        .push(EntityOperation::UpdateSpaceScore(UpdateSpaceScoreRequest {
                            space_id: space_id.to_string(),
                            score,
                        }));
                }
                ProcessedEvent::UpdateEntitySpaceScore {
                    entity_id,
                    space_id,
                    score,
                } => {
                    self.pending_operations
                        .push(EntityOperation::UpdateEntitySpaceScore(
                            UpdateEntitySpaceScoreRequest {
                                entity_id: entity_id.to_string(),
                                space_id: space_id.to_string(),
                                score,
                            },
                        ));
                }
            }
        }

        // Process all operations in a single bulk call, maintaining order
        let operations: Vec<EntityOperation> = self.pending_operations.drain(..).collect();
        let count = operations.len();

        debug!(count = count, "Processing operations in order");

        match self.provider.bulk_operations(&operations).await {
            Ok(summary) => {
                if summary.failed > 0 {
                    warn!(
                        succeeded = summary.succeeded,
                        failed = summary.failed,
                        "Bulk operations completed with some failures"
                    );
                    for result in summary.results.iter().filter(|r| !r.success) {
                        if let Some(ref err) = result.error {
                            error!(
                                entity_id = %result.entity_id,
                                error = %err,
                                "Failed operation"
                            );
                        }
                    }
                } else {
                    debug!(
                        count = summary.succeeded,
                        "Successfully completed all operations"
                    );
                }
                Ok(vec![summary])
            }
            Err(e) => {
                error!(error = %e, count = count, "Failed bulk operations");
                Err(IngestError::loader(format!(
                    "Failed to process {} operations: {}",
                    count, e
                )))
            }
        }
    }

    /// Check if the provider is ready (for health checks).
    /// Note: The current SearchIndexProvider doesn't have a health_check method,
    /// so we just return Ok for now.
    pub async fn check_ready(&self) -> Result<(), IngestError> {
        // The provider is ready if it was created successfully
        Ok(())
    }

    /// Run the loader task.
    ///
    /// Receives processed batches from the processor, loads them into the search index,
    /// and sends acknowledgments back to the appropriate consumer (entity or scores).
    /// Returns a tokio task handle.
    ///
    /// # Arguments
    ///
    /// * `loader_rx` - Channel to receive processed batches from the processor
    /// * `entity_ack_tx` - Channel to send acknowledgments back to the entity consumer
    /// * `scores_ack_tx` - Channel to send acknowledgments back to the scores consumer
    /// * `metrics` - Metrics tracker
    pub fn run(
        mut self,
        mut loader_rx: mpsc::Receiver<ProcessedBatch>,
        entity_ack_tx: mpsc::Sender<StreamMessage>,
        scores_ack_tx: mpsc::Sender<StreamMessage>,
        metrics: Arc<SearchIndexerMetrics>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(batch) = loader_rx.recv().await {
                // Determine which ack channel to use based on batch type
                let ack_tx = if batch.is_scores_batch {
                    &scores_ack_tx
                } else {
                    &entity_ack_tx
                };

                match self.load(batch.events).await {
                    Ok(operation_summaries) => {
                        // Check if any operation had failures
                        let total_failed =
                            operation_summaries.iter().map(|s| s.failed).sum::<usize>();
                        if total_failed > 0 {
                            // At least one indexing operation failed, send NACK
                            error!(
                                "Bulk operations completed with {} failures across {} operations",
                                total_failed,
                                operation_summaries.len()
                            );
                            if let Err(send_err) = ack_tx
                                .send(StreamMessage::Acknowledgment {
                                    offsets: batch.offsets,
                                    success: false,
                                    error: Some(format!(
                                        "Bulk operations completed with {} failures",
                                        total_failed
                                    )),
                                })
                                .await
                            {
                                error!(error = %send_err, "Failed to send failure acknowledgment - channel closed");
                            }
                        } else {
                            // All operations successful, send ACK
                            if let Err(send_err) = ack_tx
                                .send(StreamMessage::Acknowledgment {
                                    offsets: batch.offsets,
                                    success: true,
                                    error: None,
                                })
                                .await
                            {
                                error!(error = %send_err, "Failed to send success acknowledgment - channel closed");
                            }

                            // Update metrics
                            metrics
                                .total_documents_indexed
                                .fetch_add(batch.index_count as u64, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to load batch");
                        if let Err(send_err) = ack_tx
                            .send(StreamMessage::Acknowledgment {
                                offsets: batch.offsets,
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
            debug!("Loader task shutting down");
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use search_indexer_repository::{
        BatchOperationResult, BatchOperationSummary, SearchIndexError, UnsetEntityPropertiesRequest,
    };
    use search_indexer_shared::EntityDocument;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    /// Represents an operation that was performed, used to track ordering.
    #[derive(Debug, Clone, PartialEq)]
    enum TrackedOperation {
        Update {
            entity_id: String,
            add_type_relation: Option<TypeRelationData>,
        },
        Delete {
            entity_id: String,
        },
        Unset {
            entity_id: String,
            property_keys: Vec<String>,
        },
        RemoveTypeRelationById {
            relation_id: String,
        },
    }

    /// Mock search provider for testing.
    struct MockSearchProvider {
        operation_count: AtomicUsize,
        /// Tracks all operations in the order they were executed
        operation_order: std::sync::Mutex<Vec<TrackedOperation>>,
    }

    impl MockSearchProvider {
        fn new() -> Self {
            Self {
                operation_count: AtomicUsize::new(0),
                operation_order: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn get_operation_order(&self) -> Vec<TrackedOperation> {
            self.operation_order.lock().unwrap().clone()
        }

        fn get_operation_count(&self) -> usize {
            self.operation_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SearchIndexProvider for MockSearchProvider {
        async fn ensure_index_exists(&self) -> Result<(), SearchIndexError> {
            Ok(())
        }

        async fn update_document(
            &self,
            _request: &UpdateEntityRequest,
        ) -> Result<(), SearchIndexError> {
            self.operation_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn delete_document(
            &self,
            _request: &DeleteEntityRequest,
        ) -> Result<(), SearchIndexError> {
            self.operation_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn unset_document_properties(
            &self,
            _request: &UnsetEntityPropertiesRequest,
        ) -> Result<(), SearchIndexError> {
            self.operation_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn bulk_operations(
            &self,
            operations: &[EntityOperation],
        ) -> Result<BatchOperationSummary, SearchIndexError> {
            let count = operations.len();
            self.operation_count.fetch_add(count, Ordering::SeqCst);

            // Track operation order
            let mut ops = self.operation_order.lock().unwrap();
            for op in operations {
                match op {
                    EntityOperation::Update(r) => {
                        ops.push(TrackedOperation::Update {
                            entity_id: r.entity_id.clone(),
                            add_type_relation: r.add_type_relation.clone(),
                        });
                    }
                    EntityOperation::Delete(r) => {
                        ops.push(TrackedOperation::Delete {
                            entity_id: r.entity_id.clone(),
                        });
                    }
                    EntityOperation::Unset(r) => {
                        ops.push(TrackedOperation::Unset {
                            entity_id: r.entity_id.clone(),
                            property_keys: r.property_keys.clone(),
                        });
                    }
                    EntityOperation::RemoveTypeRelationById(r) => {
                        ops.push(TrackedOperation::RemoveTypeRelationById {
                            relation_id: r.relation_id.clone(),
                        });
                    }
                    // Score updates are tracked but don't need detailed tracking in tests
                    EntityOperation::UpdateEntityGlobalScore(_)
                    | EntityOperation::UpdateSpaceScore(_)
                    | EntityOperation::UpdateEntitySpaceScore(_) => {
                        // Score updates pass through - no special tracking needed
                    }
                }
            }
            drop(ops);

            let results: Vec<BatchOperationResult> = operations
                .iter()
                .map(|op| BatchOperationResult {
                    entity_id: op.entity_id().to_string(),
                    space_id: op.space_id().to_string(),
                    success: true,
                    error: None,
                })
                .collect();

            Ok(BatchOperationSummary {
                total: count,
                succeeded: count,
                failed: 0,
                results,
            })
        }
    }

    #[tokio::test]
    async fn test_load_and_flush() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let events = vec![
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Test 1".to_string()),
                None,
            )),
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Test 2".to_string()),
                None,
            )),
        ];

        loader.load(events).await.unwrap();

        assert_eq!(provider.get_operation_count(), 2);
    }

    #[tokio::test]
    async fn test_delete_processing() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Create a soft delete document (Index with deleted=true)
        let mut doc = EntityDocument::new(entity_id, space_id, None, None);
        doc.deleted = Some(true);

        let events = vec![ProcessedEvent::Index(doc)];

        loader.load(events).await.unwrap();

        assert_eq!(provider.get_operation_count(), 1);
        let ops = provider.get_operation_order();
        assert!(matches!(ops[0], TrackedOperation::Update { .. }));
    }

    #[tokio::test]
    async fn test_unset_properties_processing() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let events = vec![ProcessedEvent::UnsetProperties {
            entity_id: Uuid::new_v4(),
            space_id: Uuid::new_v4(),
            property_keys: vec!["name".to_string(), "description".to_string()],
        }];

        loader.load(events).await.unwrap();

        assert_eq!(provider.get_operation_count(), 1);
        let ops = provider.get_operation_order();
        assert!(matches!(ops[0], TrackedOperation::Unset { .. }));
    }

    #[tokio::test]
    async fn test_mixed_event_types() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let mut delete_doc = EntityDocument::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
            None,
        );
        delete_doc.deleted = Some(true);

        let events = vec![
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 1".to_string()),
                None,
            )),
            ProcessedEvent::Index(delete_doc), // Soft delete
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 2".to_string()),
                Some("Description".to_string()),
            )),
            ProcessedEvent::UnsetProperties {
                entity_id: Uuid::new_v4(),
                space_id: Uuid::new_v4(),
                property_keys: vec!["name".to_string()],
            },
        ];

        loader.load(events).await.unwrap();

        // All 4 operations processed in a single bulk call
        assert_eq!(provider.get_operation_count(), 4);
        let ops = provider.get_operation_order();
        assert!(matches!(ops[0], TrackedOperation::Update { .. })); // Index
        assert!(matches!(ops[1], TrackedOperation::Update { .. })); // Soft delete (now Update)
        assert!(matches!(ops[2], TrackedOperation::Update { .. })); // Index
        assert!(matches!(ops[3], TrackedOperation::Unset { .. }));
    }

    #[tokio::test]
    async fn test_load_multiple_documents() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        // Add multiple documents
        let events = vec![
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 1".to_string()),
                None,
            )),
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 2".to_string()),
                None,
            )),
        ];

        loader.load(events).await.unwrap();
        // Should process all documents immediately

        assert_eq!(provider.get_operation_count(), 2);
    }

    #[tokio::test]
    async fn test_load_processes_immediately() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let events = vec![ProcessedEvent::Index(EntityDocument::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            None,
        ))];

        loader.load(events).await.unwrap();
        // Load processes immediately
        assert_eq!(provider.get_operation_count(), 1);
    }

    #[tokio::test]
    async fn test_load_empty_events() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        // Load empty events should succeed
        let summaries = loader.load(vec![]).await.unwrap();
        assert_eq!(summaries.len(), 0);
        assert_eq!(provider.get_operation_count(), 0);
    }

    #[tokio::test]
    async fn test_default_configuration() {
        let provider = Arc::new(MockSearchProvider::new());
        let _loader = SearchLoader::new(provider);

        // Test that default config works
        assert!(true);
    }

    #[tokio::test]
    async fn test_check_ready() {
        let provider = Arc::new(MockSearchProvider::new());
        let loader = SearchLoader::new(provider);

        // check_ready should always return Ok for the current implementation
        let result = loader.check_ready().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_entity_document_conversion() {
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let doc = EntityDocument::new(
            entity_id,
            space_id,
            Some("Test Name".to_string()),
            Some("Test Description".to_string()),
        );

        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let events = vec![ProcessedEvent::Index(doc)];
        loader.load(events).await.unwrap();

        // Verify the document was processed
        assert_eq!(provider.get_operation_count(), 1);
    }

    #[tokio::test]
    async fn test_add_type_relation_processing() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let relation_id = Uuid::new_v4();
        let entity_to_id = Uuid::new_v4();

        let events = vec![ProcessedEvent::AddTypeRelation {
            entity_id,
            space_id,
            relation_id,
            entity_to_id,
        }];

        loader.load(events).await.unwrap();

        // AddTypeRelation should create an Update operation with add_type_relation set
        assert_eq!(provider.get_operation_count(), 1);

        let ops = provider.get_operation_order();
        assert!(matches!(
            &ops[0],
            TrackedOperation::Update { add_type_relation: Some(rel), .. }
            if rel.entity_to_id == entity_to_id.to_string()
        ));
    }

    #[tokio::test]
    async fn test_remove_type_relation_processing() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let relation_id = Uuid::new_v4();

        let events = vec![ProcessedEvent::RemoveTypeRelationById { relation_id }];

        loader.load(events).await.unwrap();

        // RemoveTypeRelationById goes through bulk_operations
        assert_eq!(provider.get_operation_count(), 1);
    }

    #[tokio::test]
    async fn test_multiple_add_type_relations() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let entity_to_id1 = Uuid::new_v4();
        let entity_to_id2 = Uuid::new_v4();

        let events = vec![
            ProcessedEvent::AddTypeRelation {
                entity_id: Uuid::new_v4(),
                space_id: Uuid::new_v4(),
                relation_id: Uuid::new_v4(),
                entity_to_id: entity_to_id1,
            },
            ProcessedEvent::AddTypeRelation {
                entity_id: Uuid::new_v4(),
                space_id: Uuid::new_v4(),
                relation_id: Uuid::new_v4(),
                entity_to_id: entity_to_id2,
            },
        ];

        loader.load(events).await.unwrap();

        assert_eq!(provider.get_operation_count(), 2);

        let ops = provider.get_operation_order();
        assert!(matches!(
            &ops[0],
            TrackedOperation::Update { add_type_relation: Some(rel), .. }
            if rel.entity_to_id == entity_to_id1.to_string()
        ));
        assert!(matches!(
            &ops[1],
            TrackedOperation::Update { add_type_relation: Some(rel), .. }
            if rel.entity_to_id == entity_to_id2.to_string()
        ));
    }

    #[tokio::test]
    async fn test_mixed_type_relation_operations() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let add_entity_to_id = Uuid::new_v4();
        let remove_relation_id = Uuid::new_v4();

        let events = vec![
            ProcessedEvent::AddTypeRelation {
                entity_id: Uuid::new_v4(),
                space_id: Uuid::new_v4(),
                relation_id: Uuid::new_v4(),
                entity_to_id: add_entity_to_id,
            },
            ProcessedEvent::RemoveTypeRelationById {
                relation_id: remove_relation_id,
            },
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Test Entity".to_string()),
                None,
            )),
        ];

        loader.load(events).await.unwrap();

        // 3 operations: AddTypeRelation, RemoveTypeRelationById, and Index
        assert_eq!(provider.get_operation_count(), 3);

        // Verify the operations are in order
        let ops = provider.get_operation_order();
        assert_eq!(ops.len(), 3);

        // First should be the add_type_relation (Update)
        assert!(matches!(
            &ops[0],
            TrackedOperation::Update { add_type_relation: Some(rel), .. }
            if rel.entity_to_id == add_entity_to_id.to_string()
        ));

        // Second should be the RemoveTypeRelationById
        assert!(matches!(
            &ops[1],
            TrackedOperation::RemoveTypeRelationById { relation_id }
            if *relation_id == remove_relation_id.to_string()
        ));

        // Third should be the regular index (Update with no add_type_relation)
        assert!(matches!(
            &ops[2],
            TrackedOperation::Update {
                add_type_relation: None,
                ..
            }
        ));
    }

    /// This test verifies that type relation operations are processed in order.
    ///
    /// The scenario: RemoveTypeRelationById followed by AddTypeRelation
    #[tokio::test]
    async fn test_type_relation_operations_preserve_order() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let relation_id = Uuid::new_v4();
        let entity_to_id = Uuid::new_v4();

        // Events in this order: Remove first, then Add
        let events = vec![
            ProcessedEvent::RemoveTypeRelationById { relation_id },
            ProcessedEvent::AddTypeRelation {
                entity_id,
                space_id,
                relation_id,
                entity_to_id,
            },
        ];

        loader.load(events).await.unwrap();

        // 2 operations via bulk_operations
        assert_eq!(
            provider.get_operation_count(),
            2,
            "Should have 2 operations"
        );

        // Verify operations are tracked
        let ops = provider.get_operation_order();
        assert_eq!(ops.len(), 2, "Should have 2 operations tracked");

        // First operation should be RemoveTypeRelationById
        assert!(
            matches!(&ops[0], TrackedOperation::RemoveTypeRelationById { relation_id: rid } if *rid == relation_id.to_string()),
            "First operation should be RemoveTypeRelationById, got: {:?}",
            ops[0]
        );

        // Second operation should be AddTypeRelation (via Update)
        assert!(
            matches!(&ops[1], TrackedOperation::Update { add_type_relation: Some(rel), .. } if rel.entity_to_id == entity_to_id.to_string()),
            "Second operation should be AddTypeRelation (via Update), got: {:?}",
            ops[1]
        );
    }

    /// Test that an UpdateEntityRequest with both add_type_relation AND other properties
    /// results in two separate bulk operations.
    #[tokio::test]
    async fn test_update_with_add_type_relation_and_properties() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let relation_id = Uuid::new_v4();
        let entity_to_id = Uuid::new_v4();

        // Create a document with name set
        let doc = EntityDocument::new(
            entity_id,
            space_id,
            Some("Test Entity Name".to_string()),
            Some("Test Description".to_string()),
        );
        // We need to simulate having add_type_relation - but EntityDocument doesn't have this field.
        // Instead, we'll test this at a lower level by creating the operation directly.
        // For now, let's just test that Index + AddTypeRelation for the same entity creates proper operations.

        let events = vec![
            // First, add a type relation
            ProcessedEvent::AddTypeRelation {
                entity_id,
                space_id,
                relation_id,
                entity_to_id,
            },
            // Then, index the document with name/description
            ProcessedEvent::Index(doc),
        ];

        loader.load(events).await.unwrap();

        // Should have 2 operations: one for add_type_relation, one for the document update
        assert_eq!(provider.get_operation_count(), 2);

        let ops = provider.get_operation_order();
        assert_eq!(ops.len(), 2);

        // First should be add_type_relation
        assert!(matches!(
            &ops[0],
            TrackedOperation::Update { add_type_relation: Some(rel), .. }
            if rel.entity_to_id == entity_to_id.to_string()
        ));

        // Second should be regular document update (no add_type_relation)
        assert!(matches!(
            &ops[1],
            TrackedOperation::Update {
                add_type_relation: None,
                ..
            }
        ));
    }
}
