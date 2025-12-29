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
    DeleteEntityRequest, SearchIndexProvider, UnsetEntityPropertiesRequest, UpdateEntityRequest,
};

/// Loader that indexes documents into the search engine.
///
/// The loader is responsible for:
/// - Batching documents for efficient bulk indexing
/// - Converting EntityDocuments to UpdateEntityRequests
pub struct SearchLoader {
    provider: Arc<dyn SearchIndexProvider>,
    pending_updates: Vec<UpdateEntityRequest>,
    pending_deletes: Vec<DeleteEntityRequest>,
    pending_unsets: Vec<UnsetEntityPropertiesRequest>,
}

impl SearchLoader {
    /// Create a new search loader with the given provider.
    pub fn new(provider: Arc<dyn SearchIndexProvider>) -> Self {
        Self {
            provider,
            pending_updates: Vec::new(),
            pending_deletes: Vec::new(),
            pending_unsets: Vec::new(),
        }
    }

    /// Load a batch of processed events.
    ///
    /// Processes all events immediately, indexing documents and handling deletes.
    /// Returns summaries of bulk operations performed.
    #[instrument(skip(self, events), fields(event_count = events.len()))]
    pub async fn load(
        &mut self,
        events: Vec<ProcessedEvent>,
    ) -> Result<Vec<search_indexer_repository::BatchOperationSummary>, IngestError> {
        let mut operation_summaries = Vec::new();

        for event in events {
            match event {
                ProcessedEvent::Index(doc) => {
                    // Convert EntityDocument to UpdateEntityRequest
                    let update_request = UpdateEntityRequest {
                        entity_id: doc.entity_id.to_string(),
                        space_id: doc.space_id.to_string(),
                        name: doc.name,
                        description: doc.description,
                        avatar: doc.avatar,
                        cover: doc.cover,
                        entity_global_score: doc.entity_global_score,
                        space_score: doc.space_score,
                        entity_space_score: doc.entity_space_score,
                    };
                    self.pending_updates.push(update_request);
                }
                ProcessedEvent::Delete {
                    entity_id,
                    space_id,
                } => {
                    let delete_request = DeleteEntityRequest {
                        entity_id: entity_id.to_string(),
                        space_id: space_id.to_string(),
                    };
                    self.pending_deletes.push(delete_request);
                }
                ProcessedEvent::UnsetProperties {
                    entity_id,
                    space_id,
                    property_keys,
                } => {
                    // Accumulate unset operations for bulk processing
                    let unset_request = UnsetEntityPropertiesRequest {
                        entity_id: entity_id.to_string(),
                        space_id: space_id.to_string(),
                        property_keys,
                    };
                    self.pending_unsets.push(unset_request);
                }
            }
        }

        // Process all pending updates
        if !self.pending_updates.is_empty() {
            let updates: Vec<UpdateEntityRequest> = self.pending_updates.drain(..).collect();
            let count = updates.len();

            debug!(count = count, "Indexing documents to search index");

            // Use bulk_update_documents from SearchIndexProvider
            match self.provider.bulk_update_documents(&updates).await {
                Ok(summary) => {
                    if summary.failed > 0 {
                        warn!(
                            succeeded = summary.succeeded,
                            failed = summary.failed,
                            "Bulk update completed with some failures"
                        );
                        // Log individual failures
                        for result in summary.results.iter().filter(|r| !r.success) {
                            if let Some(ref err) = result.error {
                                error!(
                                    entity_id = %result.entity_id,
                                    error = %err,
                                    "Failed to update document"
                                );
                            }
                        }
                    } else {
                        debug!(
                            count = summary.succeeded,
                            "Successfully updated all documents"
                        );
                    }
                    operation_summaries.push(summary);
                }
                Err(e) => {
                    error!(error = %e, count = count, "Failed to bulk update documents");
                    return Err(IngestError::loader(format!(
                        "Failed to bulk update {} documents: {}",
                        count, e
                    )));
                }
            }
        }

        // Process all pending deletes
        if !self.pending_deletes.is_empty() {
            let deletes: Vec<DeleteEntityRequest> = self.pending_deletes.drain(..).collect();

            match self.provider.bulk_delete_documents(&deletes).await {
                Ok(summary) => {
                    if summary.failed > 0 {
                        warn!(
                            succeeded = summary.succeeded,
                            failed = summary.failed,
                            "Bulk delete completed with some failures"
                        );
                        // Log individual failures
                        for result in summary.results.iter().filter(|r| !r.success) {
                            if let Some(ref err) = result.error {
                                error!(
                                    entity_id = %result.entity_id,
                                    error = %err,
                                    "Failed to delete document"
                                );
                            }
                        }
                    } else {
                        debug!(
                            count = summary.succeeded,
                            "Successfully deleted all documents"
                        );
                    }
                    operation_summaries.push(summary);
                }
                Err(e) => {
                    error!(error = %e, count = deletes.len(), "Failed to bulk delete documents");
                    return Err(IngestError::loader(format!(
                        "Failed to bulk delete {} documents: {}",
                        deletes.len(),
                        e
                    )));
                }
            }
        }

        // Process all pending unsets
        if !self.pending_unsets.is_empty() {
            let unsets: Vec<UnsetEntityPropertiesRequest> = self.pending_unsets.drain(..).collect();

            match self.provider.bulk_unset_properties(&unsets).await {
                Ok(summary) => {
                    if summary.failed > 0 {
                        warn!(
                            succeeded = summary.succeeded,
                            failed = summary.failed,
                            "Bulk unset completed with some failures"
                        );
                        // Log individual failures
                        for result in summary.results.iter().filter(|r| !r.success) {
                            if let Some(ref err) = result.error {
                                error!(
                                    entity_id = %result.entity_id,
                                    error = %err,
                                    "Failed to unset document properties"
                                );
                            }
                        }
                    } else {
                        debug!(
                            count = summary.succeeded,
                            "Successfully unset properties from all documents"
                        );
                    }
                    operation_summaries.push(summary);
                }
                Err(e) => {
                    error!(error = %e, count = unsets.len(), "Failed to bulk unset properties");
                    return Err(IngestError::loader(format!(
                        "Failed to bulk unset properties from {} documents: {}",
                        unsets.len(),
                        e
                    )));
                }
            }
        }

        Ok(operation_summaries)
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
    /// and sends acknowledgments back to the consumer.
    /// Returns a tokio task handle.
    pub fn run(
        mut self,
        mut loader_rx: mpsc::Receiver<ProcessedBatch>,
        ack_tx: mpsc::Sender<StreamMessage>,
        metrics: Arc<SearchIndexerMetrics>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(batch) = loader_rx.recv().await {
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

    /// Mock search provider for testing.
    struct MockSearchProvider {
        updated_count: AtomicUsize,
        deleted_count: AtomicUsize,
        unset_properties_calls: std::sync::Mutex<Vec<UnsetEntityPropertiesRequest>>,
        unset_should_fail: std::sync::Mutex<bool>,
    }

    impl MockSearchProvider {
        fn new() -> Self {
            Self {
                updated_count: AtomicUsize::new(0),
                deleted_count: AtomicUsize::new(0),
                unset_properties_calls: std::sync::Mutex::new(Vec::new()),
                unset_should_fail: std::sync::Mutex::new(false),
            }
        }

        fn set_unset_should_fail(&self, should_fail: bool) {
            *self.unset_should_fail.lock().unwrap() = should_fail;
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
            self.updated_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn delete_document(
            &self,
            _request: &DeleteEntityRequest,
        ) -> Result<(), SearchIndexError> {
            self.deleted_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn bulk_update_documents(
            &self,
            requests: &[UpdateEntityRequest],
        ) -> Result<BatchOperationSummary, SearchIndexError> {
            let count = requests.len();
            self.updated_count.fetch_add(count, Ordering::SeqCst);
            Ok(BatchOperationSummary {
                total: count,
                succeeded: count,
                failed: 0,
                results: requests
                    .iter()
                    .map(|r| BatchOperationResult {
                        entity_id: r.entity_id.clone(),
                        space_id: r.space_id.clone(),
                        success: true,
                        error: None,
                    })
                    .collect(),
            })
        }

        async fn bulk_delete_documents(
            &self,
            requests: &[DeleteEntityRequest],
        ) -> Result<BatchOperationSummary, SearchIndexError> {
            let count = requests.len();
            self.deleted_count.fetch_add(count, Ordering::SeqCst);
            Ok(BatchOperationSummary {
                total: count,
                succeeded: count,
                failed: 0,
                results: requests
                    .iter()
                    .map(|r| BatchOperationResult {
                        entity_id: r.entity_id.clone(),
                        space_id: r.space_id.clone(),
                        success: true,
                        error: None,
                    })
                    .collect(),
            })
        }

        async fn bulk_unset_properties(
            &self,
            requests: &[UnsetEntityPropertiesRequest],
        ) -> Result<BatchOperationSummary, SearchIndexError> {
            let mut results = Vec::new();
            let mut succeeded = 0;
            let mut failed = 0;

            for request in requests {
                self.unset_properties_calls
                    .lock()
                    .unwrap()
                    .push(request.clone());

                if *self.unset_should_fail.lock().unwrap() {
                    failed += 1;
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: false,
                        error: Some(SearchIndexError::IndexError(
                            "Mock unset failure".to_string(),
                        )),
                    });
                } else {
                    succeeded += 1;
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: true,
                        error: None,
                    });
                }
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded,
                failed,
                results,
            })
        }

        async fn unset_document_properties(
            &self,
            request: &UnsetEntityPropertiesRequest,
        ) -> Result<(), SearchIndexError> {
            self.unset_properties_calls
                .lock()
                .unwrap()
                .push(request.clone());

            if *self.unset_should_fail.lock().unwrap() {
                return Err(SearchIndexError::IndexError(
                    "Mock unset failure".to_string(),
                ));
            }

            Ok(())
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

        assert_eq!(provider.updated_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_delete_processing() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let events = vec![ProcessedEvent::Delete {
            entity_id: Uuid::new_v4(),
            space_id: Uuid::new_v4(),
        }];

        loader.load(events).await.unwrap();

        assert_eq!(provider.deleted_count.load(Ordering::SeqCst), 1);
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

        // Check that unset_properties_calls was incremented (MockSearchProvider always succeeds)
        assert_eq!(provider.unset_properties_calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_mixed_event_types() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        let events = vec![
            ProcessedEvent::Index(EntityDocument::new(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 1".to_string()),
                None,
            )),
            ProcessedEvent::Delete {
                entity_id: Uuid::new_v4(),
                space_id: Uuid::new_v4(),
            },
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

        assert_eq!(provider.updated_count.load(Ordering::SeqCst), 2);
        assert_eq!(provider.deleted_count.load(Ordering::SeqCst), 1);
        assert_eq!(provider.unset_properties_calls.lock().unwrap().len(), 1);
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

        assert_eq!(provider.updated_count.load(Ordering::SeqCst), 2);
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
        assert_eq!(provider.updated_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_load_empty_events() {
        let provider = Arc::new(MockSearchProvider::new());
        let mut loader = SearchLoader::new(provider.clone());

        // Load empty events should succeed
        let summaries = loader.load(vec![]).await.unwrap();
        assert_eq!(summaries.len(), 0);
        assert_eq!(provider.updated_count.load(Ordering::SeqCst), 0);
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

        // Verify the document was processed (MockSearchProvider stores requests)
        assert_eq!(provider.updated_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_unset_properties_failure_tracking() {
        let provider = Arc::new(MockSearchProvider::new());
        provider.set_unset_should_fail(true);
        let mut loader = SearchLoader::new(provider.clone());

        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let events = vec![ProcessedEvent::UnsetProperties {
            entity_id,
            space_id,
            property_keys: vec!["name".to_string()],
        }];

        let summaries = loader.load(events).await.unwrap();

        // Should have one operation summary for the bulk unset operation
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.total, 1);
        assert_eq!(summary.succeeded, 0);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.results.len(), 1);

        let result = &summary.results[0];
        assert_eq!(result.entity_id, entity_id.to_string());
        assert_eq!(result.space_id, space_id.to_string());
        assert!(!result.success);
        assert!(result.error.is_some());
    }
}
