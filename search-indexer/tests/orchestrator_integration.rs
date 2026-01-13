//! Integration tests for the search indexer orchestrator.
//!
//! These tests use the real Orchestrator but mock dependencies
//! (consumers and SearchIndexProvider) to ensure reliable testing.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

use sdk::core::ids::TYPE_RELATION_TYPE_ID;
use search_indexer::consumer::{EntityEvent, StreamMessage};
use search_indexer::errors::IngestError;
use search_indexer::loader::SearchLoader;
use search_indexer::orchestrator::{
    EntitiesConsumerTrait, EntityProcessingBatch, Orchestrator, OrchestratorConfig,
    ScoreProcessingBatch, ScoresConsumerTrait,
};
use search_indexer::processor::Processor;
use search_indexer_repository::{
    BatchOperationResult, BatchOperationSummary, DeleteEntityRequest, EntityOperation,
    SearchIndexError, SearchIndexProvider, UnsetEntityPropertiesRequest, UpdateEntityRequest,
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
impl EntitiesConsumerTrait for MockConsumer {
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
        processor_tx: mpsc::Sender<EntityProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        // If should_error is true, return an error immediately
        if self.should_error {
            return Err(IngestError::KafkaError("Mock consumer error".to_string()));
        }

        // Convert events to EntityProcessingBatch
        let events = self.events_to_send.clone();
        let offsets = vec![("test-topic".to_string(), 0, 1i64)]; // Mock offset
        let event_count = events.len();

        // Send events to processor
        let batch = EntityProcessingBatch {
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
                        // For tests: we just record whether ACK or NACK was received.
                        // The test can check get_last_acknowledgment() to verify.
                        // We don't return an error on NACK since the mock's job is just
                        // to record what happened, not to simulate real retry behavior.
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

// Mock Scores Consumer for testing - does nothing, just waits for shutdown
struct MockScoresConsumer;

#[async_trait::async_trait]
impl ScoresConsumerTrait for MockScoresConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        Ok(())
    }

    async fn run(
        &self,
        _processor_tx: mpsc::Sender<ScoreProcessingBatch>,
        _ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        // Just wait for shutdown - no score events in these tests
        let _ = shutdown.recv().await;
        Ok(())
    }
}

// Mock Search Provider for testing
struct MockSearchProvider {
    updated_documents: std::sync::Mutex<Vec<UpdateEntityRequest>>,
    deleted_documents: std::sync::Mutex<Vec<DeleteEntityRequest>>,
    unset_properties_calls: std::sync::Mutex<Vec<UnsetEntityPropertiesRequest>>,
    /// Track all operations in order for verifying ordering
    all_operations: std::sync::Mutex<Vec<EntityOperation>>,
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
            all_operations: std::sync::Mutex::new(Vec::new()),
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

    /// Get update requests that have add_type_relation set (type relation upserts).
    fn get_add_type_relation_requests(&self) -> Vec<UpdateEntityRequest> {
        self.updated_documents
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.add_type_relation.is_some())
            .cloned()
            .collect()
    }

    /// Get relation IDs removed via RemoveTypeRelationById operations.
    fn get_removed_relation_ids(&self) -> Vec<String> {
        self.all_operations
            .lock()
            .unwrap()
            .iter()
            .filter_map(|op| {
                if let EntityOperation::RemoveTypeRelationById(r) = op {
                    Some(r.relation_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get all operations in the order they were processed.
    fn get_all_operations_in_order(&self) -> Vec<EntityOperation> {
        self.all_operations.lock().unwrap().clone()
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

    async fn bulk_operations(
        &self,
        operations: &[EntityOperation],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        let mut results = Vec::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for (i, op) in operations.iter().enumerate() {
            let entity_id = op.entity_id().to_string();
            let space_id = op.space_id().to_string();

            // Determine if this operation should fail based on configuration
            let should_fail = match op {
                EntityOperation::Update(_) => self.fail_bulk_updates && i >= operations.len() / 2,
                EntityOperation::Delete(_) => self.fail_bulk_deletes && i >= operations.len() / 2,
                EntityOperation::Unset(_) => self.fail_bulk_unsets && i >= operations.len() / 2,
                EntityOperation::RemoveTypeRelationById(_) => false, // Never fails in mock
                // Score operations never fail in mock
                EntityOperation::UpdateEntityGlobalScore(_)
                | EntityOperation::UpdateSpaceScore(_)
                | EntityOperation::UpdateEntitySpaceScore(_) => false,
            };

            if should_fail {
                failed += 1;
                results.push(BatchOperationResult {
                    entity_id,
                    space_id,
                    success: false,
                    error: Some(SearchIndexError::bulk_operation(
                        "Simulated failure".to_string(),
                    )),
                });
            } else {
                // Track the operation in type-specific vectors
                match op {
                    EntityOperation::Update(req) => {
                        self.updated_documents.lock().unwrap().push(req.clone());
                    }
                    EntityOperation::Delete(req) => {
                        self.deleted_documents.lock().unwrap().push(req.clone());
                    }
                    EntityOperation::Unset(req) => {
                        self.unset_properties_calls
                            .lock()
                            .unwrap()
                            .push(req.clone());
                    }
                    EntityOperation::RemoveTypeRelationById(_) => {
                        // Tracked via all_operations
                    }
                    // Score operations are tracked via all_operations only
                    EntityOperation::UpdateEntityGlobalScore(_)
                    | EntityOperation::UpdateSpaceScore(_)
                    | EntityOperation::UpdateEntitySpaceScore(_) => {
                        // Tracked via all_operations
                    }
                }
                // Also track in all_operations to preserve ordering
                self.all_operations.lock().unwrap().push(op.clone());

                succeeded += 1;
                results.push(BatchOperationResult {
                    entity_id,
                    space_id,
                    success: true,
                    error: None,
                });
            }
        }

        Ok(BatchOperationSummary {
            total: operations.len(),
            succeeded,
            failed,
            results,
        })
    }
}

/// Helper to create a test orchestrator with mocked dependencies
fn create_test_orchestrator(events: Vec<EntityEvent>) -> (Orchestrator, Arc<MockSearchProvider>) {
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events));
    let mock_scores_consumer = Arc::new(MockScoresConsumer);

    let orchestrator = Orchestrator::new(mock_consumer, mock_scores_consumer, processor, loader);

    (orchestrator, mock_provider)
}

/// Helper to create a test orchestrator with mocked dependencies (returns consumer for ACK checking)
fn create_test_orchestrator_with_consumer(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events));
    let mock_scores_consumer = Arc::new(MockScoresConsumer);

    let orchestrator = Orchestrator::new(
        mock_consumer.clone(),
        mock_scores_consumer,
        processor,
        loader,
    );

    (orchestrator, mock_provider, mock_consumer)
}

/// Helper to create a test orchestrator with an error-prone consumer
fn create_error_test_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>) {
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::with_subscribe_error(events));
    let mock_scores_consumer = Arc::new(MockScoresConsumer);

    let orchestrator = Orchestrator::new(mock_consumer, mock_scores_consumer, processor, loader);

    (orchestrator, mock_provider)
}

/// Helper to create a test orchestrator with bulk update failures
fn create_bulk_update_failure_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::with_bulk_update_failures());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events.clone()));
    let mock_scores_consumer = Arc::new(MockScoresConsumer);

    let orchestrator = Orchestrator::new(
        mock_consumer.clone(),
        mock_scores_consumer,
        processor,
        loader,
    );

    (orchestrator, mock_provider, mock_consumer)
}

/// Helper to create a test orchestrator with bulk delete failures
fn create_bulk_delete_failure_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::with_bulk_delete_failures());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events.clone()));
    let mock_scores_consumer = Arc::new(MockScoresConsumer);

    let orchestrator = Orchestrator::new(
        mock_consumer.clone(),
        mock_scores_consumer,
        processor,
        loader,
    );

    (orchestrator, mock_provider, mock_consumer)
}

/// Helper to create a test orchestrator with bulk unset failures
fn create_bulk_unset_failure_orchestrator(
    events: Vec<EntityEvent>,
) -> (Orchestrator, Arc<MockSearchProvider>, Arc<MockConsumer>) {
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::with_bulk_unset_failures());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_consumer = Arc::new(MockConsumer::new(events.clone()));
    let mock_scores_consumer = Arc::new(MockScoresConsumer);

    let orchestrator = Orchestrator::new(
        mock_consumer.clone(),
        mock_scores_consumer,
        processor,
        loader,
    );

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
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());
    let mock_consumer = Arc::new(MockConsumer::new(vec![]));
    let mock_scores_consumer = Arc::new(MockScoresConsumer);

    let config = OrchestratorConfig {
        channel_buffer_size: 2000,
    };

    let _orchestrator = Orchestrator::with_config(
        mock_consumer,
        mock_scores_consumer,
        processor,
        loader,
        config,
    );

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

// ============================================================================
// Type ID Integration Tests (create_relation and delete_relation)
// ============================================================================

#[tokio::test]
async fn test_upsert_type_relation_adds_type_id() {
    // Test that upserting a "type" relation adds the type_id to the entity
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id = Uuid::new_v4(); // The type being assigned
    let relation_id = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![EntityEvent::create_relation(
        relation_id,
        relation_type,
        entity_id,
        type_id,
        space_id,
    )];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for successful operation"
    );

    // Verify the add_type_relation request was created
    let add_type_relation_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(
        add_type_relation_requests.len(),
        1,
        "Expected 1 add_type_relation request"
    );

    let request = &add_type_relation_requests[0];
    assert_eq!(request.entity_id, entity_id.to_string());
    assert_eq!(request.space_id, space_id.to_string());
    assert!(request.add_type_relation.is_some());
    let rel = request.add_type_relation.as_ref().unwrap();
    assert_eq!(rel.entity_to_id, type_id.to_string());
}

#[tokio::test]
async fn test_delete_type_relation_removes_type_id() {
    // Test that deleting a relation removes it from the entity's type_relations
    let relation_id = Uuid::new_v4();

    let events = vec![EntityEvent::delete_relation(relation_id)];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for successful operation"
    );

    // Verify RemoveTypeRelationById operation was processed
    let removed_relation_ids = mock_provider.get_removed_relation_ids();
    assert_eq!(
        removed_relation_ids.len(),
        1,
        "Expected 1 RemoveTypeRelationById operation"
    );
    assert_eq!(removed_relation_ids[0], relation_id.to_string());
}

#[tokio::test]
async fn test_non_type_relation_is_skipped() {
    // Test that relations with a non-type relation_type are skipped
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let to_entity_id = Uuid::new_v4();
    let relation_id = Uuid::new_v4();
    let relation_type = Uuid::new_v4(); // NOT the type relation type

    let events = vec![EntityEvent::create_relation(
        relation_id,
        relation_type,
        entity_id,
        to_entity_id,
        space_id,
    )];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent (even though nothing was indexed)
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Verify NO add_type_relation requests were created
    let add_type_relation_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(
        add_type_relation_requests.len(),
        0,
        "Expected no add_type_relation requests for non-type relation"
    );
}

#[tokio::test]
async fn test_mixed_entity_and_type_relation_events() {
    // Test processing a mix of entity updates and type relation events
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();
    let relation_id = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        // First: upsert the entity
        EntityEvent::upsert(
            entity_id,
            space_id,
            Some("Test Entity".to_string()),
            Some("Description".to_string()),
            None,
        ),
        // Second: add a type via relation
        EntityEvent::create_relation(relation_id, relation_type, entity_id, type_id, space_id),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Should have 2 update operations total:
    // 1. The entity upsert
    // 2. The add_type_relation operation
    assert_eq!(
        mock_provider.get_updated_count(),
        2,
        "Expected 2 update operations"
    );

    // Verify the add_type_relation request
    let add_type_relation_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(add_type_relation_requests.len(), 1);
    assert!(add_type_relation_requests[0].add_type_relation.is_some());
    assert_eq!(
        add_type_relation_requests[0]
            .add_type_relation
            .as_ref()
            .unwrap()
            .entity_to_id,
        type_id.to_string()
    );
}

#[tokio::test]
async fn test_add_then_remove_relation() {
    // Test that adding and then removing a relation works correctly
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();
    let relation_id = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        // First: add the type
        EntityEvent::create_relation(relation_id, relation_type, entity_id, type_id, space_id),
        // Second: remove the type (only relation_id is available for delete)
        EntityEvent::delete_relation(relation_id),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Should have 1 add_type_relation via bulk operations
    let add_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(
        add_requests.len(),
        1,
        "Expected 1 add_type_relation request"
    );

    // Should have 1 RemoveTypeRelationById operation
    let removed_relation_ids = mock_provider.get_removed_relation_ids();
    assert_eq!(
        removed_relation_ids.len(),
        1,
        "Expected 1 RemoveTypeRelationById operation"
    );

    // Verify they're for the same relation
    assert!(add_requests[0].add_type_relation.is_some());
    assert_eq!(
        add_requests[0]
            .add_type_relation
            .as_ref()
            .unwrap()
            .entity_to_id,
        type_id.to_string()
    );
    assert_eq!(
        add_requests[0]
            .add_type_relation
            .as_ref()
            .unwrap()
            .relation_id,
        relation_id.to_string()
    );
    assert_eq!(removed_relation_ids[0], relation_id.to_string());

    // Verify both operations are tracked in order
    let all_ops = mock_provider.get_all_operations_in_order();
    assert_eq!(all_ops.len(), 2, "Expected 2 operations");

    // First operation should be an Update with add_type_relation
    match &all_ops[0] {
        EntityOperation::Update(req) => {
            assert!(
                req.add_type_relation.is_some(),
                "First operation should be add_type_relation"
            );
            assert_eq!(
                req.add_type_relation.as_ref().unwrap().entity_to_id,
                type_id.to_string()
            );
        }
        _ => panic!(
            "First operation should be Update (add_type_relation), got {:?}",
            all_ops[0]
        ),
    }

    // Second operation should be RemoveTypeRelationById
    match &all_ops[1] {
        EntityOperation::RemoveTypeRelationById(req) => {
            assert_eq!(req.relation_id, relation_id.to_string());
        }
        _ => panic!(
            "Second operation should be RemoveTypeRelationById, got {:?}",
            all_ops[1]
        ),
    }
}

#[tokio::test]
async fn test_multiple_types_for_same_entity() {
    // Test adding multiple types to the same entity
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id_1 = Uuid::new_v4();
    let type_id_2 = Uuid::new_v4();
    let type_id_3 = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id,
            type_id_1,
            space_id,
        ),
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id,
            type_id_2,
            space_id,
        ),
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id,
            type_id_3,
            space_id,
        ),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Should have 3 add_type_relation operations
    let add_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(
        add_requests.len(),
        3,
        "Expected 3 add_type_relation requests"
    );

    // Verify all are for the same entity
    for request in &add_requests {
        assert_eq!(request.entity_id, entity_id.to_string());
        assert_eq!(request.space_id, space_id.to_string());
    }

    // Verify all three types are represented
    let added_types: Vec<_> = add_requests
        .iter()
        .filter_map(|r| {
            r.add_type_relation
                .as_ref()
                .map(|rel| rel.entity_to_id.clone())
        })
        .collect();
    assert!(added_types.contains(&type_id_1.to_string()));
    assert!(added_types.contains(&type_id_2.to_string()));
    assert!(added_types.contains(&type_id_3.to_string()));
}

#[tokio::test]
async fn test_create_relations_for_multiple_entities() {
    // Test creating type relations for multiple different entities in a single batch
    let entity_id_1 = Uuid::new_v4();
    let entity_id_2 = Uuid::new_v4();
    let entity_id_3 = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id_1 = Uuid::new_v4();
    let type_id_2 = Uuid::new_v4();
    let type_id_3 = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id_1,
            type_id_1,
            space_id,
        ),
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id_2,
            type_id_2,
            space_id,
        ),
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id_3,
            type_id_3,
            space_id,
        ),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Should have 3 add_type_relation operations
    let add_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(
        add_requests.len(),
        3,
        "Expected 3 add_type_relation requests"
    );

    // Verify each entity got its correct type
    let entity_type_pairs: Vec<_> = add_requests
        .iter()
        .map(|r| {
            (
                r.entity_id.clone(),
                r.add_type_relation
                    .as_ref()
                    .map(|rel| rel.entity_to_id.clone())
                    .unwrap(),
            )
        })
        .collect();

    assert!(entity_type_pairs.contains(&(entity_id_1.to_string(), type_id_1.to_string())));
    assert!(entity_type_pairs.contains(&(entity_id_2.to_string(), type_id_2.to_string())));
    assert!(entity_type_pairs.contains(&(entity_id_3.to_string(), type_id_3.to_string())));
}

#[tokio::test]
async fn test_delete_multiple_relations() {
    // Test deleting multiple relations in a single batch
    let relation_id_1 = Uuid::new_v4();
    let relation_id_2 = Uuid::new_v4();
    let relation_id_3 = Uuid::new_v4();

    let events = vec![
        EntityEvent::delete_relation(relation_id_1),
        EntityEvent::delete_relation(relation_id_2),
        EntityEvent::delete_relation(relation_id_3),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Verify all 3 RemoveTypeRelationById operations were processed
    let removed_relation_ids = mock_provider.get_removed_relation_ids();
    assert_eq!(
        removed_relation_ids.len(),
        3,
        "Expected 3 RemoveTypeRelationById operations"
    );

    // Verify all relation IDs are present
    assert!(removed_relation_ids.contains(&relation_id_1.to_string()));
    assert!(removed_relation_ids.contains(&relation_id_2.to_string()));
    assert!(removed_relation_ids.contains(&relation_id_3.to_string()));
}

#[tokio::test]
async fn test_mixed_create_and_delete_relations_different_entities() {
    // Test mixing create and delete relation operations across different entities
    let entity_id_1 = Uuid::new_v4();
    let entity_id_2 = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id_1 = Uuid::new_v4();
    let type_id_2 = Uuid::new_v4();
    let relation_id_to_delete_1 = Uuid::new_v4();
    let relation_id_to_delete_2 = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        // Create a type relation for entity 1
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id_1,
            type_id_1,
            space_id,
        ),
        // Delete an unrelated relation
        EntityEvent::delete_relation(relation_id_to_delete_1),
        // Create a type relation for entity 2
        EntityEvent::create_relation(
            Uuid::new_v4(),
            relation_type,
            entity_id_2,
            type_id_2,
            space_id,
        ),
        // Delete another unrelated relation
        EntityEvent::delete_relation(relation_id_to_delete_2),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Verify 2 add_type_relation operations
    let add_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(
        add_requests.len(),
        2,
        "Expected 2 add_type_relation requests"
    );

    // Verify 2 RemoveTypeRelationById operations
    let removed_relation_ids = mock_provider.get_removed_relation_ids();
    assert_eq!(
        removed_relation_ids.len(),
        2,
        "Expected 2 RemoveTypeRelationById operations"
    );

    // Verify correct relation IDs were removed
    assert!(removed_relation_ids.contains(&relation_id_to_delete_1.to_string()));
    assert!(removed_relation_ids.contains(&relation_id_to_delete_2.to_string()));

    // Verify correct type relations were added
    let added_entities: Vec<_> = add_requests.iter().map(|r| r.entity_id.clone()).collect();
    assert!(added_entities.contains(&entity_id_1.to_string()));
    assert!(added_entities.contains(&entity_id_2.to_string()));
}

#[tokio::test]
async fn test_interleaved_entity_and_relation_operations() {
    // Test a complex batch with entity upserts, deletes, and relation operations
    let entity_id_1 = Uuid::new_v4();
    let entity_id_2 = Uuid::new_v4();
    let entity_id_3 = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();
    let relation_id_1 = Uuid::new_v4();
    let relation_id_2 = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        // Upsert entity 1
        EntityEvent::upsert(
            entity_id_1,
            space_id,
            Some("Entity One".to_string()),
            Some("Description one".to_string()),
            None,
        ),
        // Create type relation for entity 1
        EntityEvent::create_relation(relation_id_1, relation_type, entity_id_1, type_id, space_id),
        // Upsert entity 2
        EntityEvent::upsert(
            entity_id_2,
            space_id,
            Some("Entity Two".to_string()),
            None,
            None,
        ),
        // Delete entity 3
        EntityEvent::delete(entity_id_3, space_id),
        // Delete a relation
        EntityEvent::delete_relation(relation_id_2),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Verify counts:
    // - 2 entity upserts + 1 add_type_relation = 3 updates
    // - 1 entity delete
    // - 1 relation delete
    assert_eq!(mock_provider.get_updated_count(), 3, "Expected 3 updates");
    assert_eq!(mock_provider.get_deleted_count(), 1, "Expected 1 delete");

    let removed_relation_ids = mock_provider.get_removed_relation_ids();
    assert_eq!(
        removed_relation_ids.len(),
        1,
        "Expected 1 RemoveTypeRelationById operation"
    );
    assert_eq!(removed_relation_ids[0], relation_id_2.to_string());

    // Verify the add_type_relation was for entity 1
    let add_requests = mock_provider.get_add_type_relation_requests();
    assert_eq!(add_requests.len(), 1, "Expected 1 add_type_relation");
    assert_eq!(add_requests[0].entity_id, entity_id_1.to_string());
    assert_eq!(
        add_requests[0]
            .add_type_relation
            .as_ref()
            .unwrap()
            .relation_id,
        relation_id_1.to_string()
    );
}

#[tokio::test]
async fn test_relation_operations_preserve_order() {
    // Test that relation operations are processed in the correct order
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id_1 = Uuid::new_v4();
    let type_id_2 = Uuid::new_v4();
    let relation_id_1 = Uuid::new_v4();
    let relation_id_2 = Uuid::new_v4();
    let relation_id_3 = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        // Create relation 1
        EntityEvent::create_relation(relation_id_1, relation_type, entity_id, type_id_1, space_id),
        // Create relation 2
        EntityEvent::create_relation(relation_id_2, relation_type, entity_id, type_id_2, space_id),
        // Delete relation 1
        EntityEvent::delete_relation(relation_id_1),
        // Delete relation 3 (was never created, but should still process)
        EntityEvent::delete_relation(relation_id_3),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Verify operations are in correct order
    let all_ops = mock_provider.get_all_operations_in_order();
    assert_eq!(all_ops.len(), 4, "Expected 4 operations");

    // Operation 0: Create relation 1 (add type_id_1)
    match &all_ops[0] {
        EntityOperation::Update(req) => {
            assert!(req.add_type_relation.is_some());
            let rel = req.add_type_relation.as_ref().unwrap();
            assert_eq!(rel.relation_id, relation_id_1.to_string());
            assert_eq!(rel.entity_to_id, type_id_1.to_string());
        }
        _ => panic!("Expected Update operation at index 0, got {:?}", all_ops[0]),
    }

    // Operation 1: Create relation 2 (add type_id_2)
    match &all_ops[1] {
        EntityOperation::Update(req) => {
            assert!(req.add_type_relation.is_some());
            let rel = req.add_type_relation.as_ref().unwrap();
            assert_eq!(rel.relation_id, relation_id_2.to_string());
            assert_eq!(rel.entity_to_id, type_id_2.to_string());
        }
        _ => panic!("Expected Update operation at index 1, got {:?}", all_ops[1]),
    }

    // Operation 2: Delete relation 1
    match &all_ops[2] {
        EntityOperation::RemoveTypeRelationById(req) => {
            assert_eq!(req.relation_id, relation_id_1.to_string());
        }
        _ => panic!(
            "Expected RemoveTypeRelationById operation at index 2, got {:?}",
            all_ops[2]
        ),
    }

    // Operation 3: Delete relation 3
    match &all_ops[3] {
        EntityOperation::RemoveTypeRelationById(req) => {
            assert_eq!(req.relation_id, relation_id_3.to_string());
        }
        _ => panic!(
            "Expected RemoveTypeRelationById operation at index 3, got {:?}",
            all_ops[3]
        ),
    }
}

#[tokio::test]
async fn test_create_relation_with_upsert_for_same_entity() {
    // Test that we can upsert an entity and add a type relation in the same batch
    // Both operations should be processed for the same entity
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();
    let relation_id = Uuid::new_v4();
    let relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap();

    let events = vec![
        // Upsert the entity
        EntityEvent::upsert(
            entity_id,
            space_id,
            Some("My Entity".to_string()),
            Some("A description".to_string()),
            Some("https://example.com/avatar.png".to_string()),
        ),
        // Add a type to the same entity
        EntityEvent::create_relation(relation_id, relation_type, entity_id, type_id, space_id),
    ];

    let (orchestrator, mock_provider, mock_consumer) =
        create_test_orchestrator_with_consumer(events);

    // Run the orchestrator
    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    // Verify ACK was sent
    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    // Should have 2 update operations total
    assert_eq!(mock_provider.get_updated_count(), 2, "Expected 2 updates");

    // Verify both operations are for the same entity
    let all_ops = mock_provider.get_all_operations_in_order();
    assert_eq!(all_ops.len(), 2, "Expected 2 operations");

    // First should be the entity upsert
    match &all_ops[0] {
        EntityOperation::Update(req) => {
            assert_eq!(req.entity_id, entity_id.to_string());
            assert_eq!(req.name, Some("My Entity".to_string()));
            assert_eq!(req.description, Some("A description".to_string()));
            assert!(
                req.add_type_relation.is_none(),
                "First update should not have add_type_relation"
            );
        }
        _ => panic!("Expected Update operation at index 0, got {:?}", all_ops[0]),
    }

    // Second should be the type relation add
    match &all_ops[1] {
        EntityOperation::Update(req) => {
            assert_eq!(req.entity_id, entity_id.to_string());
            assert!(req.add_type_relation.is_some());
            let rel = req.add_type_relation.as_ref().unwrap();
            assert_eq!(rel.relation_id, relation_id.to_string());
            assert_eq!(rel.entity_to_id, type_id.to_string());
        }
        _ => panic!("Expected Update operation at index 1, got {:?}", all_ops[1]),
    }
}
