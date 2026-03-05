//! Integration tests for topology processing.
//!
//! These tests verify that the search indexer correctly processes topology diffs
//! (canonical graph changes) and emits the correct `UpdateInCanonicalGraph`
//! operations. Tests cover varying diff orderings, idempotent replays, and
//! multi-batch scenarios.

use serial_test::serial;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

use search_indexer::consumer::topology_consumer::ParsedCanonicalGraphDiff;
use search_indexer::consumer::StreamMessage;
use search_indexer::errors::IngestError;
use search_indexer::loader::SearchLoader;
use search_indexer::orchestrator::{
    EntitiesConsumerTrait, EntityProcessingBatch, Orchestrator, ScoreProcessingBatch,
    ScoresConsumerTrait, SpaceTopicProcessingBatch, SpaceTopicsConsumerTrait,
    TopologyConsumerTrait, TopologyProcessingBatch,
};
use search_indexer::processor::Processor;
use search_indexer::topology::CanonicalGraphState;
use search_indexer_repository::{
    BatchOperationResult, BatchOperationSummary, DeleteEntityRequest, EntityOperation,
    SearchIndexError, SearchIndexProvider, UnsetEntityPropertiesRequest,
    UpdateEntityRequest,
};
use uuid::Uuid;

// ============================================================================
// Helpers
// ============================================================================

fn make_bytes(n: u8) -> [u8; 16] {
    let mut id = [0u8; 16];
    id[15] = n;
    id
}

fn make_uuid(n: u8) -> Uuid {
    Uuid::from_bytes(make_bytes(n))
}

use search_indexer::topology::state::{ChangeType, ParsedNodeChange};

fn make_topology_batch(
    root_id: [u8; 16],
    changes: Vec<ParsedNodeChange>,
) -> TopologyProcessingBatch {
    let event_count = changes.len();
    TopologyProcessingBatch {
        diffs: vec![ParsedCanonicalGraphDiff { root_id, changes }],
        offsets: vec![("topology.canonical".to_string(), 0, 1i64)],
        event_count,
    }
}

// ============================================================================
// Mock Consumers
// ============================================================================

/// Mock entities consumer - no-op, waits for shutdown.
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

/// Mock scores consumer - no-op, waits for shutdown.
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
        let _ = shutdown.recv().await;
        Ok(())
    }
}

/// Mock space topics consumer - no-op, waits for shutdown.
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

/// Mock topology consumer that sends batches then waits for ACK.
struct MockTopologyConsumer {
    batches: Vec<TopologyProcessingBatch>,
    last_acknowledgment: std::sync::Mutex<Option<bool>>,
}

impl MockTopologyConsumer {
    fn new(batches: Vec<TopologyProcessingBatch>) -> Self {
        Self {
            batches,
            last_acknowledgment: std::sync::Mutex::new(None),
        }
    }

    fn get_last_acknowledgment(&self) -> Option<bool> {
        *self.last_acknowledgment.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl TopologyConsumerTrait for MockTopologyConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        Ok(())
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<TopologyProcessingBatch>,
        mut ack_receiver: mpsc::Receiver<StreamMessage>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        for batch in &self.batches {
            let _ = processor_tx.send(batch.clone()).await;

            // Wait for ACK before sending next batch
            tokio::select! {
                _ = shutdown.recv() => return Ok(()),
                msg = ack_receiver.recv() => {
                    if let Some(StreamMessage::Acknowledgment { success, .. }) = msg {
                        *self.last_acknowledgment.lock().unwrap() = Some(success);
                        if !success {
                            return Err(IngestError::kafka("NACK received"));
                        }
                    }
                }
            }
        }

        // All batches sent and ACKed — return immediately.
        // This signals the orchestrator that the topology consumer is done,
        // triggering graceful shutdown of all other consumers.
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

    #[allow(dead_code)]
    fn get_all_operations(&self) -> Vec<EntityOperation> {
        self.all_operations.lock().unwrap().clone()
    }

    fn get_canonical_graph_updates(&self) -> Vec<(String, bool)> {
        self.all_operations
            .lock()
            .unwrap()
            .iter()
            .filter_map(|op| {
                if let EntityOperation::UpdateInCanonicalGraph(req) = op {
                    Some((req.space_id.clone(), req.in_canonical_graph))
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

            self.all_operations.lock().unwrap().push(op.clone());

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
            wall_ms: 0,
            took_ms: 0,
        })
    }
}

// ============================================================================
// Test Orchestrator Factory
// ============================================================================

// ============================================================================
// Test Setup
// ============================================================================

/// Global mutex to serialize topology tests that rely on TOPOLOGY_STATE_PATH env var.
static TOPOLOGY_PATH_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Sets TOPOLOGY_STATE_PATH to a unique writable temp path.
/// Topology tests are marked #[serial] so only one runs at a time, making this safe.
fn set_topology_state_path() {
    let _guard = TOPOLOGY_PATH_MUTEX.lock().unwrap();
    let dir = std::env::temp_dir()
        .join("topology_integration_tests")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).expect("Failed to create temp dir for topology tests");
    let path = dir.join("state.json");
    // Safety: topology tests run serially via #[serial], so no concurrent env var mutations
    unsafe { std::env::set_var("TOPOLOGY_STATE_PATH", &path) };
}

fn create_topology_test_orchestrator(
    batches: Vec<TopologyProcessingBatch>,
) -> (
    Orchestrator,
    Arc<MockSearchProvider>,
    Arc<MockTopologyConsumer>,
) {
    create_topology_test_orchestrator_with_state(batches, CanonicalGraphState::new())
}

fn create_topology_test_orchestrator_with_state(
    batches: Vec<TopologyProcessingBatch>,
    topology_state: CanonicalGraphState,
) -> (
    Orchestrator,
    Arc<MockSearchProvider>,
    Arc<MockTopologyConsumer>,
) {
    set_topology_state_path();

    let processor = Processor::with_config(
        std::collections::HashMap::new(),
        topology_state,
        0,
    );
    let mock_provider = Arc::new(MockSearchProvider::new());
    let loader = SearchLoader::new(mock_provider.clone());

    let mock_entities_consumer = Arc::new(MockEntitiesConsumer);
    let mock_scores_consumer = Arc::new(MockScoresConsumer);
    let mock_space_topics_consumer = Arc::new(MockSpaceTopicsConsumer);
    let mock_topology_consumer = Arc::new(MockTopologyConsumer::new(batches));

    let orchestrator = Orchestrator::new(
        mock_entities_consumer,
        mock_scores_consumer,
        mock_space_topics_consumer,
        mock_topology_consumer.clone(),
        processor,
        loader,
    );

    (orchestrator, mock_provider, mock_topology_consumer)
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_topology_basic_canonical_graph() {
    // Root + 2 children ADDED → 3x UpdateInCanonicalGraph(true)
    let root = make_bytes(1);
    let child_a = make_bytes(2);
    let child_b = make_bytes(3);

    let batch = make_topology_batch(
        root,
        vec![
            ParsedNodeChange {
                space_id: child_a,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
            ParsedNodeChange {
                space_id: child_b,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
        ],
    );

    let (orchestrator, mock_provider, mock_consumer) =
        create_topology_test_orchestrator(vec![batch]);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    let run_result = result.unwrap();
    assert!(run_result.is_ok(), "Orchestrator should succeed, got error: {:?}", run_result.err());

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_canonical_graph_updates();
    assert_eq!(updates.len(), 3, "Expected 3 updates (root + 2 children)");

    // All should be added (in_canonical_graph=true)
    for (space_id, in_canonical) in &updates {
        assert!(
            *in_canonical,
            "Expected in_canonical_graph=true for space {}",
            space_id
        );
    }

    // Check that all expected space IDs are present
    let space_ids: Vec<&str> = updates.iter().map(|(s, _)| s.as_str()).collect();
    assert!(space_ids.contains(&make_uuid(1).to_string().as_str()));
    assert!(space_ids.contains(&make_uuid(2).to_string().as_str()));
    assert!(space_ids.contains(&make_uuid(3).to_string().as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_topology_remove_child() {
    // Batch 1: root + A + B added
    // Batch 2: B removed
    // Verify UpdateInCanonicalGraph(B, false) in second batch
    let root = make_bytes(1);
    let child_a = make_bytes(2);
    let child_b = make_bytes(3);

    let batch1 = make_topology_batch(
        root,
        vec![
            ParsedNodeChange {
                space_id: child_a,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
            ParsedNodeChange {
                space_id: child_b,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
        ],
    );

    let batch2 = make_topology_batch(
        root,
        vec![ParsedNodeChange {
            space_id: child_b,
            change_type: ChangeType::Removed,
            distance: None,
            parent_id: None,
        }],
    );

    let (orchestrator, mock_provider, mock_consumer) =
        create_topology_test_orchestrator(vec![batch1, batch2]);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_canonical_graph_updates();

    // Batch 1: root(true) + A(true) + B(true) = 3
    // Batch 2: B(false) = 1
    // Total = 4
    assert_eq!(updates.len(), 4, "Expected 4 total updates");

    // The last update should be B removed
    let b_uuid = make_uuid(3).to_string();
    let b_removal = updates
        .iter()
        .filter(|(s, c)| s == &b_uuid && !c)
        .count();
    assert_eq!(b_removal, 1, "Expected exactly 1 removal for B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_topology_move_no_canonicality_change() {
    // Tree: root -> A -> C, root -> B
    // Move C from A to B
    // Verify zero UpdateInCanonicalGraph ops for C in second batch
    let root = make_bytes(1);
    let node_a = make_bytes(2);
    let node_b = make_bytes(3);
    let node_c = make_bytes(4);

    let batch1 = make_topology_batch(
        root,
        vec![
            ParsedNodeChange {
                space_id: node_a,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
            ParsedNodeChange {
                space_id: node_b,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
            ParsedNodeChange {
                space_id: node_c,
                change_type: ChangeType::Added,
                distance: Some(2),
                parent_id: Some(node_a),
            },
        ],
    );

    let batch2 = make_topology_batch(
        root,
        vec![ParsedNodeChange {
            space_id: node_c,
            change_type: ChangeType::Moved,
            distance: Some(2),
            parent_id: Some(node_b),
        }],
    );

    let (orchestrator, mock_provider, mock_consumer) =
        create_topology_test_orchestrator(vec![batch1, batch2]);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_canonical_graph_updates();

    // Batch 1: root + A + B + C = 4 additions
    // Batch 2: MOVED → 0 operations
    // Total = 4
    assert_eq!(updates.len(), 4, "Expected 4 updates (all from batch 1)");

    // All should be additions
    for (_, in_canonical) in &updates {
        assert!(*in_canonical, "All updates should be additions");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_topology_multiple_diffs_varying_order() {
    // Batch 1: root + A
    // Batch 2: B + C added under root
    let root = make_bytes(1);
    let node_a = make_bytes(2);
    let node_b = make_bytes(3);
    let node_c = make_bytes(4);

    let batch1 = make_topology_batch(
        root,
        vec![ParsedNodeChange {
            space_id: node_a,
            change_type: ChangeType::Added,
            distance: Some(1),
            parent_id: Some(root),
        }],
    );

    let batch2 = make_topology_batch(
        root,
        vec![
            ParsedNodeChange {
                space_id: node_b,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
            ParsedNodeChange {
                space_id: node_c,
                change_type: ChangeType::Added,
                distance: Some(1),
                parent_id: Some(root),
            },
        ],
    );

    let (orchestrator, mock_provider, mock_consumer) =
        create_topology_test_orchestrator(vec![batch1, batch2]);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_canonical_graph_updates();

    // Batch 1: root + A = 2
    // Batch 2: B + C = 2
    // Total = 4
    assert_eq!(updates.len(), 4, "Expected 4 updates across 2 batches");

    let space_ids: Vec<String> = updates.iter().map(|(s, _)| s.clone()).collect();
    assert!(space_ids.contains(&make_uuid(1).to_string()));
    assert!(space_ids.contains(&make_uuid(2).to_string()));
    assert!(space_ids.contains(&make_uuid(3).to_string()));
    assert!(space_ids.contains(&make_uuid(4).to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_topology_idempotent_replay() {
    // Same diff sent twice. First produces ops, second produces zero ops.
    let root = make_bytes(1);
    let child = make_bytes(2);

    let changes = vec![ParsedNodeChange {
        space_id: child,
        change_type: ChangeType::Added,
        distance: Some(1),
        parent_id: Some(root),
    }];

    let batch1 = make_topology_batch(root, changes.clone());
    let batch2 = make_topology_batch(root, changes);

    let (orchestrator, mock_provider, mock_consumer) =
        create_topology_test_orchestrator(vec![batch1, batch2]);

    let result = timeout(Duration::from_secs(5), orchestrator.run()).await;
    assert!(result.is_ok(), "Orchestrator should complete");
    assert!(result.unwrap().is_ok(), "Orchestrator should succeed");

    let last_ack = mock_consumer.get_last_acknowledgment();
    assert_eq!(last_ack, Some(true), "Expected ACK");

    let updates = mock_provider.get_canonical_graph_updates();

    // First batch: root + child = 2 additions
    // Second batch: both already canonical = 0 ops
    // Total = 2
    assert_eq!(
        updates.len(),
        2,
        "Expected 2 updates (idempotent replay produces 0 extra)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_topology_entities_get_canonical_flag() {
    // Pre-populate topology state, then send entity upserts.
    // Verify that entity documents produced have in_canonical_graph set correctly.
    //
    // We use Processor::with_config to pre-populate topology state, meaning
    // the topology state is already built before entities arrive.
    // However, the orchestrator test harness wires up consumers to the processor,
    // so we test at the processor level directly for this scenario.
    let root = make_bytes(1);
    let canonical_space = make_bytes(2);
    let non_canonical_space = make_bytes(99);

    // Pre-populate topology state: root + canonical_space
    let topology_state = CanonicalGraphState::from_snapshot(
        Some(root),
        vec![(canonical_space, root, 1)],
    );

    // Verify the state is correct before testing
    assert!(topology_state.is_canonical(&root));
    assert!(topology_state.is_canonical(&canonical_space));
    assert!(!topology_state.is_canonical(&non_canonical_space));

    // Test via processor directly (simpler and avoids concurrent timing issues)
    let processor = Processor::with_config(
        std::collections::HashMap::new(),
        topology_state,
        0,
    );

    use search_indexer::consumer::{EntityEvent, EntityEventType};

    let canonical_entity = EntityEvent {
        event_type: EntityEventType::Upsert,
        entity_id: Uuid::new_v4(),
        space_id: make_uuid(2), // canonical space
        name: Some("Canonical Entity".to_string()),
        description: None,
        avatar: None,
        cover: None,
        image_url: None,
        unset_property_keys: vec![],
        relation_id: None,
        relation_type: None,
        to_entity_id: None,
    };

    let non_canonical_entity = EntityEvent {
        event_type: EntityEventType::Upsert,
        entity_id: Uuid::new_v4(),
        space_id: make_uuid(99), // non-canonical space
        name: Some("Non-Canonical Entity".to_string()),
        description: None,
        avatar: None,
        cover: None,
        image_url: None,
        unset_property_keys: vec![],
        relation_id: None,
        relation_type: None,
        to_entity_id: None,
    };

    let events = vec![canonical_entity, non_canonical_entity];
    let processed = processor.process_batch(events).unwrap();

    // Find the Index events
    use search_indexer::processor::ProcessedEvent;

    let indexed: Vec<_> = processed
        .iter()
        .filter_map(|e| {
            if let ProcessedEvent::Index(doc) = e {
                Some(doc)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(indexed.len(), 2, "Expected 2 indexed documents");

    // Find the document for the canonical space entity
    let canonical_doc = indexed
        .iter()
        .find(|d| d.space_id == make_uuid(2))
        .expect("Should find canonical entity doc");
    assert_eq!(
        canonical_doc.in_canonical_graph,
        Some(true),
        "Entity in canonical space should have in_canonical_graph=true"
    );

    // Find the document for the non-canonical space entity
    let non_canonical_doc = indexed
        .iter()
        .find(|d| d.space_id == make_uuid(99))
        .expect("Should find non-canonical entity doc");
    assert_eq!(
        non_canonical_doc.in_canonical_graph,
        Some(false),
        "Entity in non-canonical space should have in_canonical_graph=false"
    );
}
