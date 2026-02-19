//! Integration tests for score update operations.
//!
//! These tests verify that the search indexer correctly handles score updates
//! including zero, negative, and positive score values. This is critical because
//! scores use z-score normalization which produces values in the range (-∞, +∞).

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

use search_indexer::consumer::{ScoreEvent, StreamMessage};
use search_indexer::errors::IngestError;
use search_indexer::loader::SearchLoader;
use search_indexer::orchestrator::{
    EntitiesConsumerTrait, EntityProcessingBatch, Orchestrator, ScoreProcessingBatch,
    ScoresConsumerTrait, SpaceTopicProcessingBatch, SpaceTopicsConsumerTrait,
};
use search_indexer::processor::Processor;
use search_indexer_repository::{
    BatchOperationResult, BatchOperationSummary, DeleteEntityRequest, EntityOperation,
    SearchIndexError, SearchIndexProvider, UnsetEntityPropertiesRequest, UpdateEntityRequest,
};
use uuid::Uuid;

// ============================================================================
// Mock Consumers
// ============================================================================

/// Mock entities consumer that does nothing - we're testing scores
struct MockEntitiesConsumer;

#[async_trait::async_trait]
impl EntitiesConsumerTrait for MockEntitiesConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        Ok(())
    }

    async fn run(
        &self,
        _processor_tx: mpsc::Sender<EntityProcessingBatch>,
        _ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        let _ = shutdown.recv().await;
        Ok(())
    }
}

/// Mock scores consumer for testing
struct MockScoresConsumer {
    events_to_send: Vec<ScoreEvent>,
    last_acknowledgment: std::sync::Mutex<Option<bool>>,
}

impl MockScoresConsumer {
    fn new(events: Vec<ScoreEvent>) -> Self {
        Self {
            events_to_send: events,
            last_acknowledgment: std::sync::Mutex::new(None),
        }
    }

    fn get_last_acknowledgment(&self) -> Option<bool> {
        *self.last_acknowledgment.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl ScoresConsumerTrait for MockScoresConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        Ok(())
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<ScoreProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        let events = self.events_to_send.clone();
        let offsets = vec![("test-scores-topic".to_string(), 0, 1i64)];
        let event_count = events.len();

        if !events.is_empty() {
            let batch = ScoreProcessingBatch {
                events,
                offsets,
                event_count,
            };
            let _ = processor_tx.send(batch).await;
        }

        tokio::select! {
            _ = shutdown.recv() => {}
            msg = ack_receiver.recv() => {
                if let Some(StreamMessage::Acknowledgment { success, .. }) = msg {
                    *self.last_acknowledgment.lock().unwrap() = Some(success);
                }
            }
        }

        Ok(())
    }
}

/// Mock space topics consumer that does nothing - we're testing scores
struct MockSpaceTopicsConsumer;

#[async_trait::async_trait]
impl SpaceTopicsConsumerTrait for MockSpaceTopicsConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        Ok(())
    }

    async fn run(
        &self,
        _processor_tx: mpsc::Sender<SpaceTopicProcessingBatch>,
        _ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        let _ = shutdown.recv().await;
        Ok(())
    }
}

// ============================================================================
// Mock Search Provider
// ============================================================================

struct MockSearchProvider {
    all_operations: std::sync::Mutex<Vec<EntityOperation>>,
}

impl MockSearchProvider {
    fn new() -> Self {
        Self {
            all_operations: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn get_all_operations(&self) -> Vec<EntityOperation> {
        self.all_operations.lock().unwrap().clone()
    }

    fn get_entity_global_score_updates(&self) -> Vec<(String, f64)> {
        self.all_operations
            .lock()
            .unwrap()
            .iter()
            .filter_map(|op| {
                if let EntityOperation::UpdateEntityGlobalScore(req) = op {
                    Some((req.entity_id.clone(), req.score))
                } else {
                    None
                }
            })
            .collect()
    }

    fn get_space_score_updates(&self) -> Vec<(String, f64)> {
        self.all_operations
            .lock()
            .unwrap()
            .iter()
            .filter_map(|op| {
                if let EntityOperation::UpdateSpaceScore(req) = op {
                    Some((req.space_id.clone(), req.score))
                } else {
                    None
                }
            })
            .collect()
    }

    fn get_entity_space_score_updates(&self) -> Vec<(String, String, f64)> {
        self.all_operations
            .lock()
            .unwrap()
            .iter()
            .filter_map(|op| {
                if let EntityOperation::UpdateEntitySpaceScore(req) = op {
                    Some((req.entity_id.clone(), req.space_id.clone(), req.score))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl SearchIndexProvider for MockSearchProvider {
    async fn ensure_index_exists(&self) -> Result<(), SearchIndexError> {
        Ok(())
    }

    async fn update_document(
        &self,
        _request: &UpdateEntityRequest,
    ) -> Result<(), SearchIndexError> {
        Ok(())
    }

    async fn delete_document(
        &self,
        _request: &DeleteEntityRequest,
    ) -> Result<(), SearchIndexError> {
        Ok(())
    }

    async fn unset_document_properties(
        &self,
        _request: &UnsetEntityPropertiesRequest,
    ) -> Result<(), SearchIndexError> {
        Ok(())
    }

    async fn bulk_operations(
        &self,
        operations: &[EntityOperation],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        let mut results = Vec::new();

        for op in operations {
            let entity_id = op.entity_id().to_string();
            let space_id = op.space_id().to_string();

            // Store the operation
            self.all_operations.lock().unwrap().push(op.clone());

            // All operations succeed in this mock
            results.push(BatchOperationResult {
                entity_id,
                space_id,
                operation_type: op.operation_type().to_string(),
                success: true,
                error: None,
            });
        }

        Ok(BatchOperationSummary {
            total: operations.len(),
            succeeded: operations.len(),
            failed: 0,
            results,
        })
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn create_scores_test_orchestrator(
    events: Vec<ScoreEvent>,
) -> (
    Orchestrator,
    Arc<MockSearchProvider>,
    Arc<MockScoresConsumer>,
) {
    let processor = Processor::new();
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_entities_consumer = Arc::new(MockEntitiesConsumer);
    let mock_scores_consumer = Arc::new(MockScoresConsumer::new(events));
    let mock_space_topics_consumer = Arc::new(MockSpaceTopicsConsumer);

    let orchestrator = Orchestrator::new(
        mock_entities_consumer,
        mock_scores_consumer.clone(),
        mock_space_topics_consumer,
        processor,
        loader,
    );

    (orchestrator, mock_provider, mock_scores_consumer)
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_positive_entity_global_score() {
    let entity_id = Uuid::new_v4();
    let score = 1.5;

    let events = vec![ScoreEvent::entity_global_score(entity_id, score, 1000)];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_entity_global_score_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, entity_id.to_string());
    assert_eq!(updates[0].1, score);
}

#[tokio::test]
async fn test_zero_entity_global_score() {
    // Test that zero scores are correctly indexed
    let entity_id = Uuid::new_v4();
    let score = 0.0;

    let events = vec![ScoreEvent::entity_global_score(entity_id, score, 1000)];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK for zero score");

    let updates = mock_provider.get_entity_global_score_updates();
    assert_eq!(updates.len(), 1, "Expected 1 score update");
    assert_eq!(updates[0].0, entity_id.to_string());
    assert_eq!(updates[0].1, 0.0, "Score should be 0.0");
}

#[tokio::test]
async fn test_negative_entity_global_score() {
    // Test that negative scores (from z-score normalization) are correctly indexed
    let entity_id = Uuid::new_v4();
    let score = -1.5;

    let events = vec![ScoreEvent::entity_global_score(entity_id, score, 1000)];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK for negative score");

    let updates = mock_provider.get_entity_global_score_updates();
    assert_eq!(updates.len(), 1, "Expected 1 score update");
    assert_eq!(updates[0].0, entity_id.to_string());
    assert_eq!(updates[0].1, -1.5, "Score should be -1.5");
}

#[tokio::test]
async fn test_zero_space_score() {
    let space_id = Uuid::new_v4();
    let score = 0.0;

    let events = vec![ScoreEvent::space_score(space_id, score, 1000)];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK for zero space score");

    let updates = mock_provider.get_space_score_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, space_id.to_string());
    assert_eq!(updates[0].1, 0.0);
}

#[tokio::test]
async fn test_negative_space_score() {
    let space_id = Uuid::new_v4();
    let score = -2.3;

    let events = vec![ScoreEvent::space_score(space_id, score, 1000)];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for negative space score"
    );

    let updates = mock_provider.get_space_score_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, space_id.to_string());
    assert_eq!(updates[0].1, -2.3);
}

#[tokio::test]
async fn test_zero_entity_space_score() {
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let score = 0.0;

    let events = vec![ScoreEvent::entity_space_score(
        entity_id, space_id, score, 1000,
    )];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for zero entity-space score"
    );

    let updates = mock_provider.get_entity_space_score_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, entity_id.to_string());
    assert_eq!(updates[0].1, space_id.to_string());
    assert_eq!(updates[0].2, 0.0);
}

#[tokio::test]
async fn test_negative_entity_space_score() {
    let entity_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let score = -0.75;

    let events = vec![ScoreEvent::entity_space_score(
        entity_id, space_id, score, 1000,
    )];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(
        last_ack,
        Some(true),
        "Expected ACK for negative entity-space score"
    );

    let updates = mock_provider.get_entity_space_score_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, entity_id.to_string());
    assert_eq!(updates[0].1, space_id.to_string());
    assert_eq!(updates[0].2, -0.75);
}

#[tokio::test]
async fn test_mixed_positive_zero_negative_scores() {
    // Test a batch with a mix of positive, zero, and negative scores
    let entity_id_1 = Uuid::new_v4();
    let entity_id_2 = Uuid::new_v4();
    let entity_id_3 = Uuid::new_v4();
    let space_id_1 = Uuid::new_v4();
    let space_id_2 = Uuid::new_v4();

    let events = vec![
        ScoreEvent::entity_global_score(entity_id_1, 2.5, 1000), // positive
        ScoreEvent::entity_global_score(entity_id_2, 0.0, 1000), // zero
        ScoreEvent::entity_global_score(entity_id_3, -1.8, 1000), // negative
        ScoreEvent::space_score(space_id_1, 0.0, 1000),          // zero
        ScoreEvent::space_score(space_id_2, -0.5, 1000),         // negative
        ScoreEvent::entity_space_score(entity_id_1, space_id_1, 1.2, 1000), // positive
        ScoreEvent::entity_space_score(entity_id_2, space_id_2, 0.0, 1000), // zero
        ScoreEvent::entity_space_score(entity_id_3, space_id_1, -0.9, 1000), // negative
    ];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK for mixed score batch");

    // Verify entity global scores
    let entity_global_updates = mock_provider.get_entity_global_score_updates();
    assert_eq!(entity_global_updates.len(), 3);
    assert!(entity_global_updates.contains(&(entity_id_1.to_string(), 2.5)));
    assert!(entity_global_updates.contains(&(entity_id_2.to_string(), 0.0)));
    assert!(entity_global_updates.contains(&(entity_id_3.to_string(), -1.8)));

    // Verify space scores
    let space_updates = mock_provider.get_space_score_updates();
    assert_eq!(space_updates.len(), 2);
    assert!(space_updates.contains(&(space_id_1.to_string(), 0.0)));
    assert!(space_updates.contains(&(space_id_2.to_string(), -0.5)));

    // Verify entity-space scores
    let entity_space_updates = mock_provider.get_entity_space_score_updates();
    assert_eq!(entity_space_updates.len(), 3);
    assert!(entity_space_updates.contains(&(entity_id_1.to_string(), space_id_1.to_string(), 1.2)));
    assert!(entity_space_updates.contains(&(entity_id_2.to_string(), space_id_2.to_string(), 0.0)));
    assert!(entity_space_updates.contains(&(
        entity_id_3.to_string(),
        space_id_1.to_string(),
        -0.9
    )));
}

#[tokio::test]
async fn test_very_small_negative_scores() {
    // Test edge case: very small negative numbers close to zero
    let entity_id = Uuid::new_v4();
    let score = -0.0001;

    let events = vec![ScoreEvent::entity_global_score(entity_id, score, 1000)];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_entity_global_score_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].1, -0.0001);
}

#[tokio::test]
async fn test_large_negative_scores() {
    // Test edge case: large negative z-scores (e.g., outliers)
    let entity_id = Uuid::new_v4();
    let score = -5.2;

    let events = vec![ScoreEvent::entity_global_score(entity_id, score, 1000)];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_entity_global_score_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].1, -5.2);
}

#[tokio::test]
async fn test_score_updates_preserve_order() {
    // Test that score updates are processed in the correct order
    let entity_id = Uuid::new_v4();

    let events = vec![
        ScoreEvent::entity_global_score(entity_id, 1.0, 1000),
        ScoreEvent::entity_global_score(entity_id, 0.0, 2000),
        ScoreEvent::entity_global_score(entity_id, -1.0, 3000),
    ];

    let (orchestrator, mock_provider, mock_consumer) = create_scores_test_orchestrator(events);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let all_ops = mock_provider.get_all_operations();
    assert_eq!(all_ops.len(), 3, "Expected 3 operations");

    // Verify the operations are in order with correct scores
    for (i, op) in all_ops.iter().enumerate() {
        match op {
            EntityOperation::UpdateEntityGlobalScore(req) => {
                assert_eq!(req.entity_id, entity_id.to_string());
                let expected_score = match i {
                    0 => 1.0,
                    1 => 0.0,
                    2 => -1.0,
                    _ => panic!("Unexpected operation index"),
                };
                assert_eq!(
                    req.score, expected_score,
                    "Expected score {} at index {}",
                    expected_score, i
                );
            }
            _ => panic!("Expected UpdateEntityGlobalScore operation at index {}", i),
        }
    }
}
