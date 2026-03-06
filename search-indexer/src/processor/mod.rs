//! Processor module for the search indexer ingest.
//!
//! Transforms entity and score events into search documents.

use hermes_instrumentation::{debug, error, info, instrument, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::consumer::StreamMessage;
use crate::consumer::{EntityEvent, EntityEventType, ScoreEvent, ScoreEventType, SpaceTopicEvent};
use crate::errors::IngestError;
use crate::metrics::SearchIndexerMetrics;
use crate::orchestrator::{
    BatchSource, EntityProcessingBatch, ProcessedBatch, ScoreProcessingBatch,
    SpaceTopicProcessingBatch, TopologyProcessingBatch,
};
use crate::topology::persistence;
use crate::topology::CanonicalGraphState;
use sdk::core::ids::{AVATAR_RELATION_TYPE_ID, COVER_RELATION_TYPE_ID, TYPE_RELATION_TYPE_ID};
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
    /// Add a relation to an entity's relations array.
    AddRelation {
        entity_id: uuid::Uuid,
        space_id: uuid::Uuid,
        relation_id: uuid::Uuid,
        relation_type: uuid::Uuid,
        to_entity_id: uuid::Uuid,
    },
    /// Remove a relation from any entity containing it, using only the relation_id.
    /// Used when we don't know which entity contains the relation.
    RemoveRelationById { relation_id: uuid::Uuid },
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
    /// Update in_canonical_graph for all entities in a space.
    /// Emitted when a space's canonical status changes.
    UpdateInCanonicalGraph {
        space_id: uuid::Uuid,
        in_canonical_graph: bool,
    },
}

/// Index into the per-event-type sample counters.
#[derive(Clone, Copy)]
enum SampleCategory {
    EntityUpsert = 0,
    EntityDelete = 1,
    EntityRestore = 2,
    UnsetProperties = 3,
    CreateRelation = 4,
    DeleteRelation = 5,
    EntityGlobalScore = 6,
    SpaceScore = 7,
    EntitySpaceScore = 8,
    UpdateSpaceTopicEntityId = 9,
    TopologyChange = 10,
}

const SAMPLE_CATEGORY_COUNT: usize = 11;

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
    /// Canonical graph state for determining in_canonical_graph status.
    topology_state: CanonicalGraphState,
    sample_counters: [AtomicU64; SAMPLE_CATEGORY_COUNT],
    sample_interval: u64,
}

impl Processor {
    /// Create a new processor with an empty space topic cache.
    pub fn new() -> Self {
        Self {
            space_topic_cache: HashMap::new(),
            topology_state: CanonicalGraphState::new(),
            sample_counters: std::array::from_fn(|_| AtomicU64::new(0)),
            sample_interval: 0,
        }
    }

    /// Create a new processor with a pre-warmed space topic cache and topology state.
    pub fn with_config(
        cache: HashMap<Uuid, Uuid>,
        topology_state: CanonicalGraphState,
        sample_interval: u64,
    ) -> Self {
        info!(
            cache_size = cache.len(),
            topology_nodes = topology_state.len(),
            sample_interval,
            "Processor created with space topic cache and topology state"
        );
        Self {
            space_topic_cache: cache,
            topology_state,
            sample_counters: std::array::from_fn(|_| AtomicU64::new(0)),
            sample_interval,
        }
    }

    /// Create a new processor with a pre-warmed space topic cache.
    pub fn with_space_topic_cache(cache: HashMap<Uuid, Uuid>, sample_interval: u64) -> Self {
        info!(
            cache_size = cache.len(),
            sample_interval, "Processor created with space topic cache"
        );
        Self {
            space_topic_cache: cache,
            topology_state: CanonicalGraphState::new(),
            sample_counters: std::array::from_fn(|_| AtomicU64::new(0)),
            sample_interval,
        }
    }

    /// Check if a relation type is one we index (type, avatar, or cover).
    fn is_indexed_relation(&self, relation_type: &Uuid) -> bool {
        let rt = relation_type.to_string();
        rt == TYPE_RELATION_TYPE_ID || rt == AVATAR_RELATION_TYPE_ID || rt == COVER_RELATION_TYPE_ID
    }

    /// Returns true for approximately 1-in-N events of the given category
    /// (where N = sample_interval). Each event type has its own counter so
    /// that high-volume event types don't starve low-volume ones.
    /// When sample_interval is 0, sampling is disabled.
    fn should_sample(&self, category: SampleCategory) -> bool {
        if self.sample_interval == 0 {
            return false;
        }
        let count = self.sample_counters[category as usize].fetch_add(1, Ordering::Relaxed);
        count.is_multiple_of(self.sample_interval)
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

            if self.should_sample(SampleCategory::UpdateSpaceTopicEntityId) {
                info!(
                    space_id = %event.space_id,
                    topic_entity_id = %event.topic_entity_id,
                    "[sample] UpdateSpaceTopicEntityId"
                );
            }

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
            //    AFTER this event. The None fields (name, description, etc.) are
            //    ignored by build_update_doc in the loader, so only entity_id,
            //    space_id, and space_topic_entity_id are written to the index.
            let mut doc = EntityDocument::new(event.topic_entity_id, event.space_id, None, None);
            doc.space_topic_entity_id = Some(event.topic_entity_id.to_string());
            processed.push(ProcessedEvent::Index(doc));
        }

        debug!(
            processed_count = processed.len(),
            "Processed space topic event batch"
        );
        Ok(processed)
    }

    /// Process a topology batch: apply changes to in-memory graph, return update operations.
    #[instrument(skip(self, batch), fields(diff_count = batch.diffs.len()))]
    pub fn process_topology_batch(&self, batch: &TopologyProcessingBatch) -> Vec<ProcessedEvent> {
        let mut ops = Vec::new();
        for diff in &batch.diffs {
            let root_uuid = Uuid::from_bytes(diff.root_id);
            let changes = self
                .topology_state
                .apply_changes(diff.root_id, &diff.changes);
            for change in changes {
                let is_root = change.space_id == root_uuid;
                if is_root {
                    info!(
                        space_id = %change.space_id,
                        in_canonical_graph = change.in_canonical_graph,
                        is_root = true,
                        "TopologyChange for root space"
                    );
                } else if self.should_sample(SampleCategory::TopologyChange) {
                    info!(
                        space_id = %change.space_id,
                        in_canonical_graph = change.in_canonical_graph,
                        "[sample] TopologyChange"
                    );
                }
                ops.push(ProcessedEvent::UpdateInCanonicalGraph {
                    space_id: change.space_id,
                    in_canonical_graph: change.in_canonical_graph,
                });
            }
        }
        debug!(operations = ops.len(), "Processed topology batch");
        ops
    }

    /// Get a reference to the topology state (for persistence).
    pub fn topology_state(&self) -> &CanonicalGraphState {
        &self.topology_state
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
                if self.should_sample(SampleCategory::EntityGlobalScore) {
                    info!(
                        entity_id = %entity_id,
                        score = event.score,
                        "[sample] EntityGlobalScore"
                    );
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
                if self.should_sample(SampleCategory::SpaceScore) {
                    info!(
                        space_id = %space_id,
                        score = event.score,
                        "[sample] SpaceScore"
                    );
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
                if self.should_sample(SampleCategory::EntitySpaceScore) {
                    info!(
                        entity_id = %entity_id,
                        space_id = %space_id,
                        score = event.score,
                        "[sample] EntitySpaceScore"
                    );
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
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        mut self,
        mut entity_rx: mpsc::Receiver<EntityProcessingBatch>,
        mut scores_rx: mpsc::Receiver<ScoreProcessingBatch>,
        mut space_topics_rx: mpsc::Receiver<SpaceTopicProcessingBatch>,
        mut topology_rx: mpsc::Receiver<TopologyProcessingBatch>,
        loader_tx: mpsc::Sender<ProcessedBatch>,
        entity_ack_tx: mpsc::Sender<StreamMessage>,
        scores_ack_tx: mpsc::Sender<StreamMessage>,
        space_topics_ack_tx: mpsc::Sender<StreamMessage>,
        topology_ack_tx: mpsc::Sender<StreamMessage>,
        metrics: Arc<SearchIndexerMetrics>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut entity_closed = false;
            let mut scores_closed = false;
            let mut space_topics_closed = false;
            let mut topology_closed = false;

            loop {
                // Exit when all channels are closed
                if entity_closed && scores_closed && space_topics_closed && topology_closed {
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
                    // Handle topology events
                    topology_batch = topology_rx.recv(), if !topology_closed => {
                        match topology_batch {
                            Some(batch) => {
                                self.handle_topology_batch(
                                    batch,
                                    &loader_tx,
                                    &topology_ack_tx,
                                    &metrics,
                                ).await;
                            }
                            None => {
                                warn!("Topology processor channel closed");
                                topology_closed = true;
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

    /// Handle a batch of topology events.
    ///
    /// Flow:
    /// 1. Apply changes to in-memory graph → get operations
    /// 2. Save graph state to JSON (write-then-rename) — if save fails, NACK
    /// 3. Send ops to loader → loader executes → ACK → consumer commits offsets
    #[instrument(skip(self, batch, loader_tx, ack_tx, metrics), fields(event_count = batch.event_count))]
    async fn handle_topology_batch(
        &self,
        batch: TopologyProcessingBatch,
        loader_tx: &mpsc::Sender<ProcessedBatch>,
        ack_tx: &mpsc::Sender<StreamMessage>,
        metrics: &Arc<SearchIndexerMetrics>,
    ) {
        let TopologyProcessingBatch {
            diffs: _,
            offsets: _offsets,
            event_count,
        } = &batch;
        let event_count = *event_count;

        // 1. Apply changes to in-memory graph
        let processed_events = self.process_topology_batch(&batch);

        metrics
            .total_events_processed
            .fetch_add(event_count as u64, Ordering::Relaxed);

        // 2. Persist state to disk before committing Kafka offsets
        let topology_state = self.topology_state.clone();
        let state_path = persistence::state_path();
        let save_result =
            tokio::task::spawn_blocking(move || persistence::save(&topology_state, &state_path))
                .await;

        match save_result {
            Ok(Ok(())) => {} // Save succeeded
            Ok(Err(e)) => {
                error!(error = %e, "Failed to save topology state, NACKing batch");
                if let Err(send_err) = ack_tx
                    .send(StreamMessage::Acknowledgment {
                        offsets: batch.offsets,
                        success: false,
                        error: Some(format!("Topology state save failed: {}", e)),
                    })
                    .await
                {
                    error!(error = %send_err, "Failed to send topology NACK - channel closed");
                }
                return;
            }
            Err(join_err) => {
                error!(error = %join_err, "Topology state save task panicked, NACKing batch");
                if let Err(send_err) = ack_tx
                    .send(StreamMessage::Acknowledgment {
                        offsets: batch.offsets,
                        success: false,
                        error: Some(format!("Topology state save panicked: {}", join_err)),
                    })
                    .await
                {
                    error!(error = %send_err, "Failed to send topology NACK - channel closed");
                }
                return;
            }
        }

        // 3. Send to loader
        if processed_events.is_empty() {
            debug!("No topology updates to index, sending ACK directly");
            if let Err(send_err) = ack_tx
                .send(StreamMessage::Acknowledgment {
                    offsets: batch.offsets,
                    success: true,
                    error: None,
                })
                .await
            {
                error!(error = %send_err, "Failed to send topology acknowledgment - channel closed");
            }
            return;
        }

        let processed_batch = ProcessedBatch {
            events: processed_events,
            offsets: batch.offsets,
            index_count: 0,
            source: BatchSource::Topology,
        };

        if let Err(send_err) = loader_tx.send(processed_batch).await {
            error!(error = %send_err, "Failed to send topology batch to loader - channel closed");
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
                doc.image_url = event.image_url;

                // Look up space_topic_entity_id from cache
                if let Some(topic_entity_id) = self.space_topic_cache.get(&event.space_id) {
                    doc.space_topic_entity_id = Some(topic_entity_id.to_string());
                }

                // Set in_canonical_graph from topology state
                let space_bytes = event.space_id.into_bytes();
                doc.in_canonical_graph = Some(self.topology_state.is_canonical(&space_bytes));

                if self.should_sample(SampleCategory::EntityUpsert) {
                    info!(
                        entity_id = %doc.entity_id,
                        space_id = %doc.space_id,
                        name = ?doc.name,
                        description = ?doc.description,
                        has_avatar = doc.avatar.is_some(),
                        has_cover = doc.cover.is_some(),
                        has_image_url = doc.image_url.is_some(),
                        "[sample] EntityUpsert"
                    );
                }

                Ok(Some(ProcessedEvent::Index(doc)))
            }
            EntityEventType::Delete => {
                // Soft delete: create a document with deleted=true
                // The upsert will preserve existing fields and only update the deleted flag
                let mut doc = EntityDocument::new(
                    event.entity_id,
                    event.space_id,
                    None, // Name not needed for delete
                    None, // Description not needed for delete
                );
                doc.deleted = Some(true);

                if self.should_sample(SampleCategory::EntityDelete) {
                    info!(
                        entity_id = %doc.entity_id,
                        space_id = %doc.space_id,
                        "[sample] EntityDelete"
                    );
                }

                Ok(Some(ProcessedEvent::Index(doc)))
            }
            EntityEventType::Restore => {
                // Restore: create a document with deleted=false to un-delete
                // The upsert will preserve existing fields and only update the deleted flag
                let mut doc = EntityDocument::new(
                    event.entity_id,
                    event.space_id,
                    None, // Name not needed for restore
                    None, // Description not needed for restore
                );
                doc.deleted = Some(false);

                if self.should_sample(SampleCategory::EntityRestore) {
                    info!(
                        entity_id = %doc.entity_id,
                        space_id = %doc.space_id,
                        "[sample] EntityRestore"
                    );
                }

                Ok(Some(ProcessedEvent::Index(doc)))
            }
            EntityEventType::UnsetProperties => {
                if event.unset_property_keys.is_empty() {
                    // No properties to unset, skip
                    return Ok(None);
                }
                if self.should_sample(SampleCategory::UnsetProperties) {
                    info!(
                        entity_id = %event.entity_id,
                        space_id = %event.space_id,
                        property_keys = ?event.unset_property_keys,
                        "[sample] UnsetProperties"
                    );
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
                    if self.is_indexed_relation(&relation_type) {
                        debug!(
                            entity_id = %event.entity_id,
                            relation_id = %relation_id,
                            relation_type = %relation_type,
                            to_entity_id = %to_entity_id,
                            space_id = %event.space_id,
                            "Processing indexed relation - adding to entity's relations"
                        );

                        if self.should_sample(SampleCategory::CreateRelation) {
                            info!(
                                entity_id = %event.entity_id,
                                space_id = %event.space_id,
                                relation_id = %relation_id,
                                relation_type = %relation_type,
                                to_entity_id = %to_entity_id,
                                "[sample] CreateRelation"
                            );
                        }

                        Ok(Some(ProcessedEvent::AddRelation {
                            entity_id: event.entity_id,
                            space_id: event.space_id,
                            relation_id,
                            relation_type,
                            to_entity_id,
                        }))
                    } else {
                        debug!(
                            relation_type = %relation_type,
                            "Skipped non-indexed relation"
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
                    if self.should_sample(SampleCategory::DeleteRelation) {
                        info!(
                            relation_id = %relation_id,
                            "[sample] DeleteRelation"
                        );
                    }

                    Ok(Some(ProcessedEvent::RemoveRelationById { relation_id }))
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
    use sdk::core::ids::{AVATAR_RELATION_TYPE_ID, COVER_RELATION_TYPE_ID, TYPE_RELATION_TYPE_ID};
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
            None,
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
    fn test_process_upsert_with_image_url() {
        let processor = Processor::new();

        let event = EntityEvent::upsert(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Image Entity".to_string()),
            None,
            None,
            None,
            Some("https://example.com/img.png".to_string()),
        );

        let result = processor.process_event(event).unwrap();
        if let Some(ProcessedEvent::Index(doc)) = result {
            assert_eq!(
                doc.image_url,
                Some("https://example.com/img.png".to_string())
            );
        } else {
            panic!("Expected ProcessedEvent::Index");
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
                None,
                None,
            ),
            EntityEvent::upsert(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("Entity 2".to_string()),
                Some("Desc".to_string()),
                None,
                None,
                None,
            ),
            EntityEvent::delete(Uuid::new_v4(), Uuid::new_v4()),
        ];

        let results = processor.process_batch(events).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_process_create_relation_non_indexed() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();
        let relation_type = Uuid::new_v4(); // Non-indexed relation
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
        // Non-indexed relations should be skipped
        assert!(result.is_none());
    }

    #[test]
    fn test_process_create_relation_type() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();
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

        let result = processor.process_event(event).unwrap();
        assert!(result.is_some());
        assert!(matches!(result, Some(ProcessedEvent::AddRelation { .. })));

        if let Some(ProcessedEvent::AddRelation {
            entity_id: eid,
            space_id: sid,
            relation_id: rid,
            relation_type: rt,
            to_entity_id: teid,
        }) = result
        {
            assert_eq!(eid, entity_id);
            assert_eq!(sid, space_id);
            assert_eq!(rid, relation_id);
            assert_eq!(rt, relation_type);
            assert_eq!(teid, to_entity_id);
        }
    }

    #[test]
    fn test_process_create_avatar_relation() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();
        let relation_type = Uuid::parse_str(AVATAR_RELATION_TYPE_ID)
            .expect("AVATAR_RELATION_TYPE_ID should be valid");
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
        assert!(result.is_some());
        assert!(matches!(result, Some(ProcessedEvent::AddRelation { .. })));
    }

    #[test]
    fn test_process_create_cover_relation() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();
        let relation_type = Uuid::parse_str(COVER_RELATION_TYPE_ID)
            .expect("COVER_RELATION_TYPE_ID should be valid");
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
        assert!(matches!(result, Some(ProcessedEvent::AddRelation { .. })));
    }

    #[test]
    fn test_process_delete_relation() {
        let processor = Processor::new();

        let relation_id = Uuid::new_v4();

        let event = EntityEvent::delete_relation(relation_id);

        let result = processor.process_event(event).unwrap();
        assert!(result.is_some());
        assert!(matches!(
            result,
            Some(ProcessedEvent::RemoveRelationById { .. })
        ));

        if let Some(ProcessedEvent::RemoveRelationById { relation_id: rid }) = result {
            assert_eq!(rid, relation_id);
        }
    }

    #[test]
    fn test_is_indexed_relation() {
        let processor = Processor::new();

        // Random relation type should not be indexed
        let random_relation_type = Uuid::new_v4();
        assert!(!processor.is_indexed_relation(&random_relation_type));

        // TYPE relation should be indexed
        let type_relation_id =
            Uuid::parse_str(TYPE_RELATION_TYPE_ID).expect("TYPE_RELATION_TYPE_ID should be valid");
        assert!(processor.is_indexed_relation(&type_relation_id));

        // AVATAR relation should be indexed
        let avatar_relation_id = Uuid::parse_str(AVATAR_RELATION_TYPE_ID)
            .expect("AVATAR_RELATION_TYPE_ID should be valid");
        assert!(processor.is_indexed_relation(&avatar_relation_id));

        // COVER relation should be indexed
        let cover_relation_id = Uuid::parse_str(COVER_RELATION_TYPE_ID)
            .expect("COVER_RELATION_TYPE_ID should be valid");
        assert!(processor.is_indexed_relation(&cover_relation_id));
    }

    #[test]
    fn test_process_upsert_with_space_topic_cache() {
        // Pre-warm cache with a space→topic mapping
        let space_id = Uuid::new_v4();
        let topic_entity_id = Uuid::new_v4();
        let mut cache = HashMap::new();
        cache.insert(space_id, topic_entity_id);

        let processor = Processor::with_space_topic_cache(cache, 0);

        let event = EntityEvent::upsert(
            Uuid::new_v4(),
            space_id,
            Some("Test Entity".to_string()),
            None,
            None,
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
            assert!(
                doc.description.is_none(),
                "Stub document should have no description"
            );
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
