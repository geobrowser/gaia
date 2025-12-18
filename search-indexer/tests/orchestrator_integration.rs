//! Integration tests for the search indexer orchestrator.
//!
//! These tests use the real Orchestrator but mock dependencies
//! (KafkaConsumer and SearchIndexProvider) to ensure reliable testing.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

use search_indexer::consumer::{EntityEvent, StreamMessage};
use search_indexer::errors::IngestError;
use search_indexer::loader::SearchLoader;
use search_indexer::orchestrator::{Consumer, Orchestrator, OrchestratorConfig, ProcessingBatch};
use search_indexer::processor::EntityProcessor;
use search_indexer_repository::{
    BatchOperationResult, BatchOperationSummary, DeleteEntityRequest, SearchIndexError,
    SearchIndexProvider, UnsetEntityPropertiesRequest, UpdateEntityRequest,
};
use uuid::Uuid;

// Mock Consumer for testing
struct MockConsumer {
    events_to_send: Vec<EntityEvent>,
    should_error: bool,
    error_on_subscribe: bool,
    last_acknowledgment: std::sync::Mutex<Option<bool>>, // true for ACK, false for NACK
}

impl MockConsumer {
    fn new(events: Vec<EntityEvent>) -> Self {
        Self {
            events_to_send: events,
            should_error: false,
            error_on_subscribe: false,
            last_acknowledgment: std::sync::Mutex::new(None),
        }
    }

    fn with_subscribe_error(events: Vec<EntityEvent>) -> Self {
        Self {
            events_to_send: events,
            should_error: false,
            error_on_subscribe: true,
            last_acknowledgment: std::sync::Mutex::new(None),
        }
    }

    fn get_last_acknowledgment(&self) -> Option<bool> {
        *self.last_acknowledgment.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl Consumer for MockConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        // Mock subscription - succeeds unless error_on_subscribe is true
        if self.error_on_subscribe {
            Err(IngestError::KafkaError("Mock subscribe error".to_string()))
        } else {
            Ok(())
        }
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<ProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        // If should_error is true, return an error immediately
        if self.should_error {
            return Err(IngestError::KafkaError("Mock consumer error".to_string()));
        }

        // Convert events to ProcessingBatch
        let events = self.events_to_send.clone();
        let offsets = vec![("test-topic".to_string(), 0, 1i64)]; // Mock offset
        let event_count = events.len();

        // Send events to processor
        let batch = ProcessingBatch {
            events,
            offsets,
            event_count,
        };
        let _ = processor_tx.send(batch).await;

        // Wait for shutdown or acknowledgment, then exit
        tokio::select! {
            _ = shutdown.recv() => {
                // Shutdown received
            }
            msg = ack_receiver.recv() => {
                match msg {
                    Some(StreamMessage::Acknowledgment { success, .. }) => {
                        *self.last_acknowledgment.lock().unwrap() = Some(success);
                        if !success {
                            return Err(IngestError::LoaderError("Processing failed".to_string()));
                        }
                    }
                    Some(_) | None => {
                        // Channel closed or unexpected message, exit
                    }
                }
            }
        }

        Ok(())
    }
}

// Mock Search Provider for testing
struct MockSearchProvider {
    updated_documents: std::sync::Mutex<Vec<UpdateEntityRequest>>,
    deleted_documents: std::sync::Mutex<Vec<DeleteEntityRequest>>,
    unset_properties_calls: std::sync::Mutex<Vec<UnsetEntityPropertiesRequest>>,
    // Configuration for simulating failures
    fail_bulk_updates: bool,
    fail_bulk_deletes: bool,
    fail_bulk_unsets: bool,
}

impl MockSearchProvider {
    fn new() -> Self {
        Self {
            updated_documents: std::sync::Mutex::new(Vec::new()),
            deleted_documents: std::sync::Mutex::new(Vec::new()),
            unset_properties_calls: std::sync::Mutex::new(Vec::new()),
            fail_bulk_updates: false,
            fail_bulk_deletes: false,
            fail_bulk_unsets: false,
        }
    }

    fn with_bulk_update_failures() -> Self {
        Self {
            fail_bulk_updates: true,
            ..Self::new()
        }
    }

    fn with_bulk_delete_failures() -> Self {
        Self {
            fail_bulk_deletes: true,
            ..Self::new()
        }
    }

    fn with_bulk_unset_failures() -> Self {
        Self {
            fail_bulk_unsets: true,
            ..Self::new()
        }
    }

    fn get_updated_count(&self) -> usize {
        self.updated_documents.lock().unwrap().len()
    }

    fn get_deleted_count(&self) -> usize {
        self.deleted_documents.lock().unwrap().len()
    }

    fn get_unset_count(&self) -> usize {
        self.unset_properties_calls.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl SearchIndexProvider for MockSearchProvider {
    async fn ensure_index_exists(&self) -> Result<(), SearchIndexError> {
        Ok(())
    }

    async fn update_document(&self, request: &UpdateEntityRequest) -> Result<(), SearchIndexError> {
        self.updated_documents.lock().unwrap().push(request.clone());
        Ok(())
    }

    async fn delete_document(&self, request: &DeleteEntityRequest) -> Result<(), SearchIndexError> {
        self.deleted_documents.lock().unwrap().push(request.clone());
        Ok(())
    }

    async fn bulk_update_documents(
        &self,
        requests: &[UpdateEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        let mut updated = self.updated_documents.lock().unwrap();

        if self.fail_bulk_updates {
            // Simulate partial failures - first half succeeds, second half fails
            let success_count = requests.len() / 2;
            let fail_count = requests.len() - success_count;

            let mut results = Vec::new();
            for (i, request) in requests.iter().enumerate() {
                if i < success_count {
                    updated.push(request.clone());
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: true,
                        error: None,
                    });
                } else {
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: false,
                        error: Some(SearchIndexError::bulk_operation(
                            "Simulated failure".to_string(),
                        )),
                    });
                }
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded: success_count,
                failed: fail_count,
                results,
            })
        } else {
            // All succeed
            for request in requests {
                updated.push(request.clone());
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded: requests.len(),
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
    }

    async fn bulk_delete_documents(
        &self,
        requests: &[DeleteEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        let mut deleted = self.deleted_documents.lock().unwrap();

        if self.fail_bulk_deletes {
            // Simulate partial failures - first half succeeds, second half fails
            let success_count = requests.len() / 2;
            let fail_count = requests.len() - success_count;

            let mut results = Vec::new();
            for (i, request) in requests.iter().enumerate() {
                if i < success_count {
                    deleted.push(request.clone());
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: true,
                        error: None,
                    });
                } else {
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: false,
                        error: Some(SearchIndexError::bulk_operation(
                            "Simulated delete failure".to_string(),
                        )),
                    });
                }
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded: success_count,
                failed: fail_count,
                results,
            })
        } else {
            // All succeed
            for request in requests {
                deleted.push(request.clone());
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded: requests.len(),
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
    }

    async fn unset_document_properties(
        &self,
        request: &UnsetEntityPropertiesRequest,
    ) -> Result<(), SearchIndexError> {
        self.unset_properties_calls
            .lock()
            .unwrap()
            .push(request.clone());
        Ok(())
    }

    async fn bulk_unset_properties(
        &self,
        requests: &[UnsetEntityPropertiesRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        let mut unset_calls = self.unset_properties_calls.lock().unwrap();

        if self.fail_bulk_unsets {
            // Simulate partial failures - first half succeeds, second half fails
            let success_count = requests.len() / 2;
            let fail_count = requests.len() - success_count;

            let mut results = Vec::new();
            for (i, request) in requests.iter().enumerate() {
                if i < success_count {
                    unset_calls.push(request.clone());
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: true,
                        error: None,
                    });
                } else {
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: false,
                        error: Some(SearchIndexError::bulk_operation(
                            "Simulated unset failure".to_string(),
                        )),
                    });
                }
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded: success_count,
                failed: fail_count,
                results,
            })
        } else {
            // All succeed
            for request in requests {
                unset_calls.push(request.clone());
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded: requests.len(),
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
    }
}

/// Helper to create a test orchestrator with mocked dependencies
fn create_test_orchestrator(events: Vec<EntityEvent>) -> (Orchestrator, Arc<MockSearchProvider>) {
    let processor = EntityProcessor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events));

    let orchestrator = Orchestrator::new(mock_consumer, processor, loader);

    (orchestrator, mock_provider)
}

/// Helper to create a test orchestrator with mocked dependencies (returns consumer for ACK checking)
fn create_test_orchestrator_with_consumer(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = EntityProcessor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events));

    let orchestrator = Orchestrator::new(mock_consumer.clone(), processor, loader);

    (orchestrator, mock_provider, mock_consumer)
}

/// Helper to create a test orchestrator with an error-prone consumer
fn create_error_test_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>) {
    let processor = EntityProcessor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::with_subscribe_error(events));

    let orchestrator = Orchestrator::new(mock_consumer, processor, loader);

    (orchestrator, mock_provider)
}

/// Helper to create a test orchestrator with bulk update failures
fn create_bulk_update_failure_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = EntityProcessor::new();
    let mock_provider = Arc::new(MockSearchProvider::with_bulk_update_failures());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events.clone()));

    let orchestrator = Orchestrator::new(mock_consumer.clone(), processor, loader);

    (orchestrator, mock_provider, mock_consumer)
}

/// Helper to create a test orchestrator with bulk delete failures
fn create_bulk_delete_failure_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = EntityProcessor::new();
    let mock_provider = Arc::new(MockSearchProvider::with_bulk_delete_failures());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events.clone()));

    let orchestrator = Orchestrator::new(mock_consumer.clone(), processor, loader);

    (orchestrator, mock_provider, mock_consumer)
}

/// Helper to create a test orchestrator with bulk unset failures
fn create_bulk_unset_failure_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = EntityProcessor::new();
    let mock_provider = Arc::new(MockSearchProvider::with_bulk_unset_failures());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events.clone()));

    let orchestrator = Orchestrator::new(mock_consumer.clone(), processor, loader);

    (orchestrator, mock_provider, mock_consumer)
}

#[tokio::test]
async fn test_orchestrator_full_integration() {
    // Test the complete orchestrator flow with mock consumer and provider
    let events = vec![
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            Some("Description".to_string()),
            None,
        ),
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Another Entity".to_string()),
            None,
            None,
        ),
    ];

    let (orchestrator, mock_provider) = create_test_orchestrator(events);

    // Run the orchestrator with a timeout to avoid hanging
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;

    // The orchestrator should complete successfully
    assert!(result.is_ok());
    let run_result = result.unwrap();
    assert!(run_result.is_ok());

    // Check that documents were indexed
    assert_eq!(mock_provider.get_updated_count(), 2);
}

#[tokio::test]
async fn test_orchestrator_with_delete_events() {
    let events = vec![
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
    ];

    let (orchestrator, mock_provider) = create_test_orchestrator(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    assert_eq!(mock_provider.get_deleted_count(), 2);
}

#[tokio::test]
async fn test_orchestrator_with_unset_properties() {
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();

    let events = vec![EntityEvent::unset_properties(
        entity_id,
        space_id,
        vec!["name".to_string(), "description".to_string()],
    )];

    let (orchestrator, mock_provider) = create_test_orchestrator(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    assert_eq!(mock_provider.get_unset_count(), 1);
}

#[tokio::test]
async fn test_orchestrator_configuration() {
    // Test that orchestrator can be created with custom configuration
    let processor = EntityProcessor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());
    let mock_consumer = Arc::new(MockConsumer::new(vec![]));

    let config = OrchestratorConfig {
        channel_buffer_size: 2000,
    };

    let _orchestrator = Orchestrator::with_config(mock_consumer, processor, loader, config);

    // Verify configuration was applied (we can't easily test this without exposing internals,
    // but at least verify it compiles and creates successfully)
    assert!(true); // If we get here, creation succeeded
}

#[tokio::test]
async fn test_empty_event_batch_processing() {
    let events = vec![]; // Empty batch

    let (orchestrator, mock_provider) = create_test_orchestrator(events);

    // Run the orchestrator with empty events
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    // No documents should be processed
    assert_eq!(mock_provider.get_updated_count(), 0);
}

#[tokio::test]
async fn test_orchestrator_shutdown() {
    // Create orchestrator with some events that will keep it running
    let events = vec![EntityEvent::upsert(
        Uuid::new_v4(),
        Uuid::new_v4(),
        Some("Test Entity".to_string()),
        Some("Description".to_string()),
        None,
    )];

    let (orchestrator, _mock_provider) = create_test_orchestrator(events);

    // Spawn orchestrator in background
    let orchestrator_handle = tokio::spawn(async move { orchestrator.run().await });

    // The orchestrator will complete when the consumer sends End message
    // Wait for orchestrator to complete
    let result = timeout(Duration::from_secs(5), orchestrator_handle).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let orchestrator_result = result.unwrap();
    assert!(
        orchestrator_result.is_ok(),
        "Orchestrator task should succeed"
    );

    let run_result = orchestrator_result.unwrap();
    assert!(
        run_result.is_ok(),
        "Orchestrator should complete successfully"
    );
}

#[tokio::test]
async fn test_orchestrator_error_handling() {
    // Create orchestrator with a consumer that will error
    let events = vec![EntityEvent::upsert(
        Uuid::new_v4(),
        Uuid::new_v4(),
        Some("Test Entity".to_string()),
        Some("Description".to_string()),
        None,
    )];

    let (orchestrator, _mock_provider) = create_error_test_orchestrator(events);

    // Run the orchestrator - it should fail due to consumer error
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let run_result = result.unwrap();
    assert!(
        run_result.is_err(),
        "Orchestrator should return error from consumer"
    );

    // Verify it's the expected error type
    match run_result.unwrap_err() {
        IngestError::KafkaError(msg) => {
            assert_eq!(msg, "Mock subscribe error");
        }
        _ => panic!("Expected KafkaError"),
    }
}

#[tokio::test]
async fn test_orchestrator_bulk_update_failure_nack() {
    // Create events that will trigger bulk updates
    let events = vec![
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 1".to_string()),
            Some("Description 1".to_string()),
            None,
        ),
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 2".to_string()),
            Some("Description 2".to_string()),
            None,
        ),
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 3".to_string()),
            Some("Description 3".to_string()),
            None,
        ),
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 4".to_string()),
            Some("Description 4".to_string()),
            None,
        ),
    ];

    let (orchestrator, _mock_provider, mock_consumer) =
        create_bulk_update_failure_orchestrator(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let run_result = result.unwrap();
    // The orchestrator should fail due to bulk operation failures
    assert!(
        run_result.is_ok(),
        "Orchestrator run should succeed (error is handled via NACK)"
    );

    // Verify that NACK was sent (not ACK)
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(false),
        "Expected NACK due to bulk operation failures"
    );
}

#[tokio::test]
async fn test_orchestrator_bulk_delete_failure_nack() {
    // Create events that will trigger bulk deletes
    let events = vec![
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
    ];

    let (orchestrator, _mock_provider, mock_consumer) =
        create_bulk_delete_failure_orchestrator(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let run_result = result.unwrap();
    // The orchestrator should fail due to bulk operation failures
    assert!(
        run_result.is_ok(),
        "Orchestrator run should succeed (error is handled via NACK)"
    );

    // Verify that NACK was sent (not ACK)
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(false),
        "Expected NACK due to bulk delete failures"
    );
}

#[tokio::test]
async fn test_orchestrator_successful_bulk_operations_ack() {
    // Test that successful bulk operations still send ACK
    let events = vec![
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 1".to_string()),
            Some("Description 1".to_string()),
            None,
        ),
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let run_result = result.unwrap();
    assert!(run_result.is_ok(), "Orchestrator should succeed");

    // Verify that ACK was sent (not NACK)
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for successful operations"
    );

    // Verify documents were processed
    assert_eq!(mock_provider.get_updated_count(), 1);
    assert_eq!(mock_provider.get_deleted_count(), 1);
}

#[tokio::test]
async fn test_bulk_update_success() {
    // Test simple success case: 2 operations, both succeed
    let events = vec![
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 1".to_string()),
            Some("Description 1".to_string()),
            None,
        ),
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 2".to_string()),
            Some("Description 2".to_string()),
            None,
        ),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify that ACK was sent (not NACK)
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for successful bulk update operations"
    );

    // Verify both documents were updated
    assert_eq!(mock_provider.get_updated_count(), 2);
}

#[tokio::test]
async fn test_bulk_update_partial_failure() {
    // Test case with 1 success op and 1 failure op - should end in error/NACK
    let events = vec![
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 1".to_string()),
            Some("Description 1".to_string()),
            None,
        ),
        EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Entity 2".to_string()),
            Some("Description 2".to_string()),
            None,
        ),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_bulk_update_failure_orchestrator(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let run_result = result.unwrap();
    // The orchestrator should succeed (error is handled via NACK)
    assert!(
        run_result.is_ok(),
        "Orchestrator run should succeed (error is handled via NACK)"
    );

    // Verify that NACK was sent (not ACK) - if one fails, whole batch is a failure
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(false),
        "Expected NACK due to bulk update partial failure (1 success, 1 failure)"
    );

    // Verify only 1 document was updated (the successful one)
    assert_eq!(mock_provider.get_updated_count(), 1);
}

#[tokio::test]
async fn test_bulk_delete_success() {
    // Test simple success case: 2 operations, both succeed
    let events = vec![
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify that ACK was sent (not NACK)
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for successful bulk delete operations"
    );

    // Verify both documents were deleted
    assert_eq!(mock_provider.get_deleted_count(), 2);
}

#[tokio::test]
async fn test_bulk_delete_partial_failure() {
    // Test case with 1 success op and 1 failure op - should end in error/NACK
    let events = vec![
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_bulk_delete_failure_orchestrator(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let run_result = result.unwrap();
    // The orchestrator should succeed (error is handled via NACK)
    assert!(
        run_result.is_ok(),
        "Orchestrator run should succeed (error is handled via NACK)"
    );

    // Verify that NACK was sent (not ACK) - if one fails, whole batch is a failure
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(false),
        "Expected NACK due to bulk delete partial failure (1 success, 1 failure)"
    );

    // Verify only 1 document was deleted (the successful one)
    assert_eq!(mock_provider.get_deleted_count(), 1);
}

#[tokio::test]
async fn test_bulk_unset_success() {
    // Test simple success case: 2 operations, both succeed
    let entity_id_1 = Uuid::new_v4();
    let space_id_1 = Uuid::new_v4();
    let entity_id_2 = Uuid::new_v4();
    let space_id_2 = Uuid::new_v4();

    let events = vec![
        EntityEvent::unset_properties(
            entity_id_1,
            space_id_1,
            vec!["name".to_string(), "description".to_string()],
        ),
        EntityEvent::unset_properties(entity_id_2, space_id_2, vec!["avatar".to_string()]),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify that ACK was sent (not NACK)
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for successful bulk unset operations"
    );

    // Verify both unset operations were processed
    assert_eq!(mock_provider.get_unset_count(), 2);
}

#[tokio::test]
async fn test_bulk_unset_partial_failure() {
    // Test case with 1 success op and 1 failure op - should end in error/NACK
    let entity_id_1 = Uuid::new_v4();
    let space_id_1 = Uuid::new_v4();
    let entity_id_2 = Uuid::new_v4();
    let space_id_2 = Uuid::new_v4();

    let events = vec![
        EntityEvent::unset_properties(
            entity_id_1,
            space_id_1,
            vec!["name".to_string(), "description".to_string()],
        ),
        EntityEvent::unset_properties(entity_id_2, space_id_2, vec!["avatar".to_string()]),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_bulk_unset_failure_orchestrator(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");

    let run_result = result.unwrap();
    // The orchestrator should succeed (error is handled via NACK)
    assert!(
        run_result.is_ok(),
        "Orchestrator run should succeed (error is handled via NACK)"
    );

    // Verify that NACK was sent (not ACK) - if one fails, whole batch is a failure
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(false),
        "Expected NACK due to bulk unset partial failure (1 success, 1 failure)"
    );

    // Verify only 1 unset operation was processed (the successful one)
    assert_eq!(mock_provider.get_unset_count(), 1);
}
