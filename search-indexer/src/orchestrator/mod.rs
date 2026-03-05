//! Orchestrator module for the search indexer ingest.
//!
//! Coordinates the consumer, processor, and loader components.

use async_trait::async_trait;
use hermes_instrumentation::{error, info, instrument};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, Duration};

#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};

use crate::consumer::{
    EntitiesConsumer, EntityEvent, ParsedCanonicalGraphDiff, ScoresConsumer, SpaceTopicsConsumer,
    StreamMessage, TopologyConsumer,
};
use crate::errors::IngestError;
use crate::loader::SearchLoader;
use crate::metrics::SearchIndexerMetrics;
use crate::processor::{ProcessedEvent, Processor};

/// Read the resident set size (RSS) of the current process in MB.
/// Only available on Linux (reads `/proc/self/status`); returns `None` on other platforms.
fn read_rss_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    let kb_str = rest.trim().trim_end_matches(" kB").trim();
                    if let Ok(kb) = kb_str.parse::<u64>() {
                        return Some(kb / 1024);
                    }
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Trait for entity event consumers used by the orchestrator.
/// This allows for dependency injection and testing with mock consumers.
#[async_trait::async_trait]
pub trait EntitiesConsumerTrait: Send + Sync {
    /// Subscribe to the configured topics/channels.
    fn subscribe(&self) -> Result<(), IngestError>;

    /// Run the consumer, sending events to processor and receiving acknowledgments from loader.
    async fn run(
        &self,
        processor_tx: mpsc::Sender<EntityProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError>;
}

/// Trait for score event consumers used by the orchestrator.
/// This allows for dependency injection and testing with mock consumers.
#[async_trait::async_trait]
pub trait ScoresConsumerTrait: Send + Sync {
    /// Subscribe to the configured topics/channels.
    fn subscribe(&self) -> Result<(), IngestError>;

    /// Run the consumer, sending events to processor and receiving acknowledgments from loader.
    async fn run(
        &self,
        processor_tx: mpsc::Sender<ScoreProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError>;
}

/// Trait for space topic event consumers used by the orchestrator.
/// This allows for dependency injection and testing with mock consumers.
#[async_trait::async_trait]
pub trait SpaceTopicsConsumerTrait: Send + Sync {
    /// Subscribe to the configured topics/channels.
    fn subscribe(&self) -> Result<(), IngestError>;

    /// Run the consumer, sending events to processor and receiving acknowledgments from loader.
    async fn run(
        &self,
        processor_tx: mpsc::Sender<SpaceTopicProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError>;
}

/// Trait for topology event consumers used by the orchestrator.
/// This allows for dependency injection and testing with mock consumers.
#[async_trait::async_trait]
pub trait TopologyConsumerTrait: Send + Sync {
    /// Subscribe to the configured topics/channels.
    fn subscribe(&self) -> Result<(), IngestError>;

    /// Run the consumer, sending events to processor and receiving acknowledgments from loader.
    async fn run(
        &self,
        processor_tx: mpsc::Sender<TopologyProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError>;
}

/// Configuration for the orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Size of the message channel buffers.
    pub channel_buffer_size: usize,
}

/// Identifies which consumer pipeline a processed batch originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchSource {
    Entity,
    Score,
    SpaceTopic,
    Topology,
}

/// Processed batch ready for loading with associated offsets for acknowledgment.
#[derive(Debug)]
pub struct ProcessedBatch {
    pub events: Vec<ProcessedEvent>,
    pub offsets: Vec<(String, i32, i64)>, // Kafka offsets for acknowledgment
    pub index_count: usize,               // Number of index operations for metrics
    pub source: BatchSource,
}

/// Batch of entity events to be processed with their offsets.
#[derive(Debug, Clone)]
pub struct EntityProcessingBatch {
    pub events: Vec<EntityEvent>,
    pub offsets: Vec<(String, i32, i64)>, // Kafka offsets for acknowledgment
    pub event_count: usize,               // Number of events for metrics
}

/// Batch of score events to be processed with their offsets.
#[derive(Debug, Clone)]
pub struct ScoreProcessingBatch {
    pub events: Vec<crate::consumer::ScoreEvent>,
    pub offsets: Vec<(String, i32, i64)>, // Kafka offsets for acknowledgment
    pub event_count: usize,               // Number of events for metrics
}

/// Batch of space topic events to be processed with their offsets.
#[derive(Debug, Clone)]
pub struct SpaceTopicProcessingBatch {
    pub events: Vec<crate::consumer::SpaceTopicEvent>,
    pub offsets: Vec<(String, i32, i64)>, // Kafka offsets for acknowledgment
    pub event_count: usize,               // Number of events for metrics
}

/// Batch of topology diffs to be processed with their offsets.
#[derive(Debug, Clone)]
pub struct TopologyProcessingBatch {
    pub diffs: Vec<ParsedCanonicalGraphDiff>,
    pub offsets: Vec<(String, i32, i64)>, // Kafka offsets for acknowledgment
    pub event_count: usize,               // Number of node changes for metrics
}

// ProcessingResult is no longer needed - processor sends directly to loader

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            channel_buffer_size: 10,
        }
    }
}

impl OrchestratorConfig {
    /// Build config from environment variables.
    ///
    /// - `CHANNEL_BUFFER_SIZE`: Max batches in flight per channel (default: 5).
    ///   Use a smaller value when memory is limited to avoid OOM on large backlogs.
    pub fn from_env() -> Self {
        let channel_buffer_size = std::env::var("CHANNEL_BUFFER_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(5);
        Self {
            channel_buffer_size,
        }
    }
}

/// Orchestrator that coordinates the ingest components.
///
/// The orchestrator:
/// - Sets up channels for direct component-to-component communication
/// - Spawns all component tasks (consumers, processor, loader)
/// - Monitors for shutdown signals and component completion
/// - Tracks and logs metrics
///
/// Components communicate directly with each other:
/// - EntitiesConsumer → Processor (via EntityProcessingBatch channel)
/// - ScoresConsumer → Processor (via ScoreProcessingBatch channel)
/// - Processor → Loader (via ProcessedBatch channel)
/// - Loader → Consumers (via Acknowledgment channels)
///
/// Each component has a `run()` method that accepts its required channels
/// and returns a tokio task handle, allowing the orchestrator to simply
/// coordinate startup and shutdown without routing messages.
pub struct Orchestrator {
    entities_consumer: Arc<dyn EntitiesConsumerTrait>,
    scores_consumer: Arc<dyn ScoresConsumerTrait>,
    space_topics_consumer: Arc<dyn SpaceTopicsConsumerTrait>,
    topology_consumer: Arc<dyn TopologyConsumerTrait>,
    processor: Processor,
    loader: SearchLoader,
    config: OrchestratorConfig,
    shutdown_tx: broadcast::Sender<()>,
    metrics: Arc<SearchIndexerMetrics>,
}

impl Orchestrator {
    /// Create a new orchestrator with the given components.
    pub fn new(
        entities_consumer: Arc<dyn EntitiesConsumerTrait>,
        scores_consumer: Arc<dyn ScoresConsumerTrait>,
        space_topics_consumer: Arc<dyn SpaceTopicsConsumerTrait>,
        topology_consumer: Arc<dyn TopologyConsumerTrait>,
        processor: Processor,
        loader: SearchLoader,
    ) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            entities_consumer,
            scores_consumer,
            space_topics_consumer,
            topology_consumer,
            processor,
            loader,
            config: OrchestratorConfig::default(),
            shutdown_tx,
            metrics: Arc::new(SearchIndexerMetrics::new()),
        }
    }

    /// Create a new orchestrator with custom configuration.
    pub fn with_config(
        entities_consumer: Arc<dyn EntitiesConsumerTrait>,
        scores_consumer: Arc<dyn ScoresConsumerTrait>,
        space_topics_consumer: Arc<dyn SpaceTopicsConsumerTrait>,
        topology_consumer: Arc<dyn TopologyConsumerTrait>,
        processor: Processor,
        loader: SearchLoader,
        config: OrchestratorConfig,
    ) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            entities_consumer,
            scores_consumer,
            space_topics_consumer,
            topology_consumer,
            processor,
            loader,
            config,
            shutdown_tx,
            metrics: Arc::new(SearchIndexerMetrics::new()),
        }
    }

    /// Run the orchestrator.
    ///
    /// This method starts all ingest components and coordinates message flow.
    /// It blocks until a shutdown signal is received or an error occurs.
    #[instrument(skip(self))]
    pub async fn run(self) -> Result<(), IngestError> {
        info!("Starting search indexer orchestrator");

        // Take ownership of components to avoid partial moves
        let entities_consumer = self.entities_consumer;
        let scores_consumer = self.scores_consumer;
        let space_topics_consumer = self.space_topics_consumer;
        let topology_consumer = self.topology_consumer;
        let processor = self.processor;
        let loader = self.loader;
        let config = self.config;
        let shutdown_tx = self.shutdown_tx;
        let metrics = self.metrics;

        // Check if loader is ready
        loader.check_ready().await?;

        // Subscribe to Kafka topics
        entities_consumer.subscribe()?;
        info!("Entities consumer subscribed");

        scores_consumer.subscribe()?;
        info!("Scores consumer subscribed");

        space_topics_consumer.subscribe()?;
        info!("Space topics consumer subscribed");

        topology_consumer.subscribe()?;
        info!("Topology consumer subscribed");

        // Create channels for direct component-to-component communication:
        // Consumer -> Processor -> Loader -> Consumer (for acks)

        // Channel from entities consumer to processor
        let (entities_processor_tx, entities_processor_rx) =
            mpsc::channel::<EntityProcessingBatch>(config.channel_buffer_size);

        // Channel from processor to loader (shared by all event types)
        let (loader_tx, loader_rx) = mpsc::channel::<ProcessedBatch>(config.channel_buffer_size);

        // Channel from loader back to entities consumer (for acknowledgments)
        let (entities_ack_tx, entities_ack_rx) =
            mpsc::channel::<StreamMessage>(config.channel_buffer_size);

        // Channels from scores consumer to processor
        let (scores_processor_tx, scores_processor_rx) =
            mpsc::channel::<ScoreProcessingBatch>(config.channel_buffer_size);

        // Channel from loader back to scores consumer (for acknowledgments)
        let (scores_ack_tx, scores_ack_rx) =
            mpsc::channel::<StreamMessage>(config.channel_buffer_size);

        // Channels from space topics consumer to processor
        let (space_topics_processor_tx, space_topics_processor_rx) =
            mpsc::channel::<SpaceTopicProcessingBatch>(config.channel_buffer_size);

        // Channel from loader back to space topics consumer (for acknowledgments)
        let (space_topics_ack_tx, space_topics_ack_rx) =
            mpsc::channel::<StreamMessage>(config.channel_buffer_size);

        // Channels from topology consumer to processor
        let (topology_processor_tx, topology_processor_rx) =
            mpsc::channel::<TopologyProcessingBatch>(config.channel_buffer_size);

        // Channel from loader back to topology consumer (for acknowledgments)
        let (topology_ack_tx, topology_ack_rx) =
            mpsc::channel::<StreamMessage>(config.channel_buffer_size);

        // Clone senders for components that need them
        let entities_processor_tx_for_consumer = entities_processor_tx.clone();
        let loader_tx_for_processor = loader_tx.clone();
        let entities_ack_tx_for_processor = entities_ack_tx.clone();
        let entities_ack_tx_for_loader = entities_ack_tx.clone();
        let scores_ack_tx_for_processor = scores_ack_tx.clone();
        let scores_ack_tx_for_loader = scores_ack_tx.clone();
        let space_topics_ack_tx_for_processor = space_topics_ack_tx.clone();
        let space_topics_ack_tx_for_loader = space_topics_ack_tx.clone();
        let topology_ack_tx_for_processor = topology_ack_tx.clone();
        let topology_ack_tx_for_loader = topology_ack_tx.clone();

        // Start processor task - receives from all consumers, sends to loader
        let processor_handle = processor.run(
            entities_processor_rx,
            scores_processor_rx,
            space_topics_processor_rx,
            topology_processor_rx,
            loader_tx_for_processor,
            entities_ack_tx_for_processor,
            scores_ack_tx_for_processor,
            space_topics_ack_tx_for_processor,
            topology_ack_tx_for_processor,
            Arc::clone(&metrics),
        );

        // Start entities consumer task - sends to processor, receives acks from loader
        let entities_consumer_clone = Arc::clone(&entities_consumer);
        let shutdown_rx = shutdown_tx.subscribe();
        let entities_consumer_handle = tokio::spawn(async move {
            entities_consumer_clone
                .run(
                    entities_processor_tx_for_consumer,
                    entities_ack_rx,
                    shutdown_rx,
                )
                .await
        });
        // So that we can await it later on shutdown
        tokio::pin!(entities_consumer_handle);

        // Start scores consumer task
        let scores_consumer_clone = Arc::clone(&scores_consumer);
        let shutdown_rx = shutdown_tx.subscribe();
        let scores_consumer_handle = tokio::spawn(async move {
            scores_consumer_clone
                .run(scores_processor_tx, scores_ack_rx, shutdown_rx)
                .await
        });
        // So that we can await it later on shutdown
        tokio::pin!(scores_consumer_handle);

        // Start space topics consumer task
        let space_topics_consumer_clone = Arc::clone(&space_topics_consumer);
        let shutdown_rx = shutdown_tx.subscribe();
        let space_topics_consumer_handle = tokio::spawn(async move {
            space_topics_consumer_clone
                .run(space_topics_processor_tx, space_topics_ack_rx, shutdown_rx)
                .await
        });
        // So that we can await it later on shutdown
        tokio::pin!(space_topics_consumer_handle);

        // Start topology consumer task
        let topology_consumer_clone = Arc::clone(&topology_consumer);
        let shutdown_rx = shutdown_tx.subscribe();
        let topology_consumer_handle = tokio::spawn(async move {
            topology_consumer_clone
                .run(topology_processor_tx, topology_ack_rx, shutdown_rx)
                .await
        });
        tokio::pin!(topology_consumer_handle);

        // Start loader task - receives from processor, sends acks to appropriate consumer
        let loader_handle = loader.run(
            loader_rx,
            entities_ack_tx_for_loader,
            scores_ack_tx_for_loader,
            space_topics_ack_tx_for_loader,
            topology_ack_tx_for_loader,
            Arc::clone(&metrics),
        );

        // Orchestrator now just monitors for shutdown and metrics
        // Components communicate directly with each other
        info!("Ready to process events from Kafka (entities + scores + space topics + topology) - components communicating directly");

        // Set up progress logging timer (every 10 seconds)
        let metrics_ref = Arc::clone(&metrics);
        let total_events = Arc::clone(&metrics_ref.total_events_processed);
        let total_docs = Arc::clone(&metrics_ref.total_documents_indexed);
        let mut progress_timer = interval(Duration::from_secs(10));
        progress_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Track previous values for rate calculation
        let mut prev_events: u64 = 0;
        let mut prev_docs: u64 = 0;
        let mut prev_bulk_calls: u64 = 0;
        let mut prev_bulk_wall: u64 = 0;
        let mut prev_ops: u64 = 0;
        let mut prev_failed: u64 = 0;
        let mut prev_time = std::time::Instant::now();

        // Track consumer results for shutdown
        let mut entities_consumer_result: Option<
            Result<Result<(), IngestError>, tokio::task::JoinError>,
        > = None;
        let mut scores_consumer_result: Option<
            Result<Result<(), IngestError>, tokio::task::JoinError>,
        > = None;
        let mut space_topics_consumer_result: Option<
            Result<Result<(), IngestError>, tokio::task::JoinError>,
        > = None;
        let mut topology_consumer_result: Option<
            Result<Result<(), IngestError>, tokio::task::JoinError>,
        > = None;

        // Set up signal handlers for graceful shutdown
        // SIGTERM is what Kubernetes sends during pod termination
        #[cfg(unix)]
        let shutdown_signal = async {
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => "SIGINT",
                _ = sigterm.recv() => "SIGTERM",
            }
        };

        #[cfg(not(unix))]
        let shutdown_signal = async {
            tokio::signal::ctrl_c().await.ok();
            "SIGINT"
        };

        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                signal_name = &mut shutdown_signal => {
                    info!("Received {}, initiating graceful shutdown", signal_name);
                    let _ = shutdown_tx.send(());
                    break;
                }
                result = &mut entities_consumer_handle => {
                    // Entities consumer task completed (either finished or errored)
                    error!("Entities consumer completed unexpectedly, initiating shutdown");
                    entities_consumer_result = Some(result);
                    let _ = shutdown_tx.send(());
                    break;
                }
                result = &mut scores_consumer_handle => {
                    // Scores consumer task completed (either finished or errored)
                    error!("Scores consumer completed unexpectedly, initiating shutdown");
                    scores_consumer_result = Some(result);
                    let _ = shutdown_tx.send(());
                    break;
                }
                result = &mut space_topics_consumer_handle => {
                    // Space topics consumer task completed (either finished or errored)
                    error!("Space topics consumer completed unexpectedly, initiating shutdown");
                    space_topics_consumer_result = Some(result);
                    let _ = shutdown_tx.send(());
                    break;
                }
                result = &mut topology_consumer_handle => {
                    // Topology consumer task completed (either finished or errored)
                    error!("Topology consumer completed unexpectedly, initiating shutdown");
                    topology_consumer_result = Some(result);
                    let _ = shutdown_tx.send(());
                    break;
                }
                _ = progress_timer.tick() => {
                    let events = total_events.load(Ordering::Relaxed);
                    let docs = total_docs.load(Ordering::Relaxed);
                    let bulk_calls = metrics_ref.total_bulk_calls.load(Ordering::Relaxed);
                    let bulk_wall = metrics_ref.total_bulk_wall_ms.load(Ordering::Relaxed);
                    let ops = metrics_ref.total_operations.load(Ordering::Relaxed);
                    let failed = metrics_ref.total_failed_operations.load(Ordering::Relaxed);

                    // Cumulative operation type counts
                    let updates = metrics_ref.total_updates.load(Ordering::Relaxed);
                    let deletes = metrics_ref.total_deletes.load(Ordering::Relaxed);
                    let unsets = metrics_ref.total_unsets.load(Ordering::Relaxed);
                    let remove_rels = metrics_ref.total_remove_relations.load(Ordering::Relaxed);
                    let scores = metrics_ref.total_score_updates.load(Ordering::Relaxed);
                    let topics = metrics_ref.total_space_topic_updates.load(Ordering::Relaxed);

                    // Calculate deltas and rates per second
                    let now = std::time::Instant::now();
                    let elapsed_secs = now.duration_since(prev_time).as_secs_f64();

                    let (events_per_sec, docs_per_sec, ops_per_sec, bulk_calls_per_sec, avg_bulk_wall_ms) = if elapsed_secs > 0.0 {
                        let delta_events = events.saturating_sub(prev_events);
                        let delta_docs = docs.saturating_sub(prev_docs);
                        let delta_ops = ops.saturating_sub(prev_ops);
                        let delta_bulk_calls = bulk_calls.saturating_sub(prev_bulk_calls);
                        let delta_bulk_wall = bulk_wall.saturating_sub(prev_bulk_wall);

                        let avg = if delta_bulk_calls > 0 {
                            delta_bulk_wall as f64 / delta_bulk_calls as f64
                        } else {
                            0.0
                        };

                        (
                            delta_events as f64 / elapsed_secs,
                            delta_docs as f64 / elapsed_secs,
                            delta_ops as f64 / elapsed_secs,
                            delta_bulk_calls as f64 / elapsed_secs,
                            avg,
                        )
                    } else {
                        (0.0, 0.0, 0.0, 0.0, 0.0)
                    };

                    let delta_failed = failed.saturating_sub(prev_failed);

                    let rss_mb = read_rss_mb().map(|v| v.to_string()).unwrap_or_else(|| "n/a".to_string());

                    info!(
                        events_processed = events,
                        documents_indexed = docs,
                        events_per_sec = format!("{:.1}", events_per_sec),
                        docs_per_sec = format!("{:.1}", docs_per_sec),
                        ops_per_sec = format!("{:.1}", ops_per_sec),
                        bulk_calls_per_sec = format!("{:.1}", bulk_calls_per_sec),
                        avg_bulk_ms = format!("{:.1}", avg_bulk_wall_ms),
                        failed_ops = delta_failed,
                        updates = updates,
                        deletes = deletes,
                        unsets = unsets,
                        remove_relations = remove_rels,
                        score_updates = scores,
                        topic_updates = topics,
                        rss_mb = %rss_mb,
                        "indexer.stats"
                    );

                    // Update previous values for next calculation
                    prev_events = events;
                    prev_docs = docs;
                    prev_bulk_calls = bulk_calls;
                    prev_bulk_wall = bulk_wall;
                    prev_ops = ops;
                    prev_failed = failed;
                    prev_time = now;
                }
            }
        }

        // Close channels to signal components to finish
        // Order matters: close producer channels first, then wait for consumers

        // Close processor channel (consumer stops sending)
        drop(entities_processor_tx);

        // Close loader channel (processor stops sending)
        drop(loader_tx);

        // Wait for processor to finish
        let _ = processor_handle.await;

        // Wait for loader to finish processing all batches and send final acks
        let _ = loader_handle.await;

        // Close ack channels (loader stops sending, signals consumers)
        drop(entities_ack_tx);
        drop(scores_ack_tx);
        drop(space_topics_ack_tx);
        drop(topology_ack_tx);

        // Wait for topology consumer to finish (if we don't already have its result)
        let topology_consumer_final_result = match topology_consumer_result {
            Some(result) => result,
            None => topology_consumer_handle.await,
        };
        info!("Topology consumer shutdown complete");

        // Wait for space topics consumer to finish (if we don't already have its result)
        let space_topics_consumer_final_result = match space_topics_consumer_result {
            Some(result) => result,
            None => space_topics_consumer_handle.await,
        };
        info!("Space topics consumer shutdown complete");

        // Wait for scores consumer to finish (if we don't already have its result)
        let scores_consumer_final_result = match scores_consumer_result {
            Some(result) => result,
            None => scores_consumer_handle.await,
        };
        info!("Scores consumer shutdown complete");

        // Wait for entities consumer to finish (if we don't already have its result)
        let entities_consumer_final_result = match entities_consumer_result {
            Some(result) => result,
            None => entities_consumer_handle.await,
        };
        info!("Entities consumer shutdown complete");

        let final_events = metrics.total_events_processed.load(Ordering::Relaxed);
        let final_docs = metrics.total_documents_indexed.load(Ordering::Relaxed);
        info!(
            total_events_processed = final_events,
            total_documents_indexed = final_docs,
            "Orchestrator shutdown complete"
        );

        // Propagate consumer errors if any (entities takes priority)
        match entities_consumer_final_result {
            Ok(Ok(())) => match scores_consumer_final_result {
                Ok(Ok(())) => match space_topics_consumer_final_result {
                    Ok(Ok(())) => match topology_consumer_final_result {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(e)) => Err(e),
                        Err(join_error) => Err(IngestError::OrchestratorError(format!(
                            "Topology consumer task panicked: {}",
                            join_error
                        ))),
                    },
                    Ok(Err(e)) => Err(e),
                    Err(join_error) => Err(IngestError::OrchestratorError(format!(
                        "Space topics consumer task panicked: {}",
                        join_error
                    ))),
                },
                Ok(Err(e)) => Err(e),
                Err(join_error) => Err(IngestError::OrchestratorError(format!(
                    "Scores consumer task panicked: {}",
                    join_error
                ))),
            },
            Ok(Err(e)) => Err(e),
            Err(join_error) => Err(IngestError::OrchestratorError(format!(
                "Entities consumer task panicked: {}",
                join_error
            ))),
        }
    }
}

#[async_trait]
impl EntitiesConsumerTrait for EntitiesConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        self.subscribe()
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<EntityProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        self.run(processor_tx, ack_receiver, shutdown).await
    }
}

#[async_trait]
impl ScoresConsumerTrait for ScoresConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        self.subscribe()
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<ScoreProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        self.run(processor_tx, ack_receiver, shutdown).await
    }
}

#[async_trait]
impl SpaceTopicsConsumerTrait for SpaceTopicsConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        self.subscribe()
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<SpaceTopicProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        self.run(processor_tx, ack_receiver, shutdown).await
    }
}

#[async_trait]
impl TopologyConsumerTrait for TopologyConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        self.subscribe()
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<TopologyProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        self.run(processor_tx, ack_receiver, shutdown).await
    }
}
