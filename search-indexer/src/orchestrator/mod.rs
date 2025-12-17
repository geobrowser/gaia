//! Orchestrator module for the search indexer ingest.
//!
//! Coordinates the consumer, processor, and loader components.

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{Duration, interval};
use tracing::{info, instrument};

use crate::consumer::{EntityEvent, KafkaConsumer, StreamMessage};
use crate::errors::IngestError;
use crate::loader::SearchLoader;
use crate::metrics::SearchIndexerMetrics;
use crate::processor::{EntityProcessor, ProcessedEvent};

/// Trait for event consumers used by the orchestrator.
/// This allows for dependency injection and testing with mock consumers.
#[async_trait::async_trait]
pub trait Consumer: Send + Sync {
    /// Subscribe to the configured topics/channels.
    fn subscribe(&self) -> Result<(), IngestError>;

    /// Run the consumer, sending events to processor and receiving acknowledgments from loader.
    async fn run(
        &self,
        processor_tx: mpsc::Sender<ProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError>;
}

/// Configuration for the orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Size of the message channel buffer.
    pub channel_buffer_size: usize,
}

/// Processed batch ready for loading with associated offsets for acknowledgment.
#[derive(Debug)]
pub struct ProcessedBatch {
    pub events: Vec<ProcessedEvent>,
    pub offsets: Vec<(String, i32, i64)>, // Kafka offsets for acknowledgment
    pub index_count: usize,               // Number of index operations for metrics
}

/// Batch of events to be processed with their offsets.
#[derive(Debug, Clone)]
pub struct ProcessingBatch {
    pub events: Vec<EntityEvent>,
    pub offsets: Vec<(String, i32, i64)>, // Kafka offsets for acknowledgment
    pub event_count: usize,               // Number of events for metrics
}

// ProcessingResult is no longer needed - processor sends directly to loader

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            channel_buffer_size: 1000,
        }
    }
}

/// Orchestrator that coordinates the ingest components.
///
/// The orchestrator:
/// - Sets up channels for direct component-to-component communication
/// - Spawns all component tasks (consumer, processor, loader)
/// - Monitors for shutdown signals and component completion
/// - Tracks and logs metrics
///
/// Components communicate directly with each other:
/// - Consumer → Processor (via ProcessingBatch channel)
/// - Processor → Loader (via ProcessedBatch channel)
/// - Loader → Consumer (via Acknowledgment channel)
///
/// Each component has a `run()` method that accepts its required channels
/// and returns a tokio task handle, allowing the orchestrator to simply
/// coordinate startup and shutdown without routing messages.
pub struct Orchestrator {
    consumer: Arc<dyn Consumer>,
    processor: EntityProcessor,
    loader: SearchLoader,
    config: OrchestratorConfig,
    shutdown_tx: broadcast::Sender<()>,
    metrics: Arc<SearchIndexerMetrics>,
}

impl Orchestrator {
    /// Create a new orchestrator with the given components.
    pub fn new(
        consumer: Arc<dyn Consumer>,
        processor: EntityProcessor,
        loader: SearchLoader,
    ) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            consumer,
            processor,
            loader,
            config: OrchestratorConfig::default(),
            shutdown_tx,
            metrics: Arc::new(SearchIndexerMetrics::new()),
        }
    }

    /// Create a new orchestrator with custom configuration.
    pub fn with_config(
        consumer: Arc<dyn Consumer>,
        processor: EntityProcessor,
        loader: SearchLoader,
        config: OrchestratorConfig,
    ) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            consumer,
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
        let consumer = self.consumer;
        let processor = self.processor;
        let loader = self.loader;
        let config = self.config;
        let shutdown_tx = self.shutdown_tx;
        let metrics = self.metrics;

        // Check if loader is ready
        loader.check_ready().await?;

        // Subscribe to Kafka topics
        consumer.subscribe()?;

        // Create channels for direct component-to-component communication:
        // Consumer -> Processor -> Loader -> Consumer (for acks)

        // Channel from consumer to processor
        let (processor_tx, processor_rx) =
            mpsc::channel::<ProcessingBatch>(config.channel_buffer_size);

        // Channel from processor to loader
        let (loader_tx, loader_rx) = mpsc::channel::<ProcessedBatch>(config.channel_buffer_size);

        // Channel from loader back to consumer (for acknowledgments)
        let (ack_tx, ack_receiver) = mpsc::channel::<StreamMessage>(config.channel_buffer_size);

        // Clone senders for components that need them
        let processor_tx_for_consumer = processor_tx.clone();
        let loader_tx_for_processor = loader_tx.clone();
        let ack_tx_for_processor = ack_tx.clone();
        let ack_tx_for_loader = ack_tx.clone();

        // Start processor task - receives from consumer, sends to loader
        let processor_handle = processor.run(
            processor_rx,
            loader_tx_for_processor,
            ack_tx_for_processor,
            Arc::clone(&metrics),
        );

        // Start consumer task - sends to processor, receives acks from loader
        let consumer_clone = Arc::clone(&consumer);
        let shutdown_rx = shutdown_tx.subscribe();
        let consumer_handle = tokio::spawn(async move {
            consumer_clone
                .run(processor_tx_for_consumer, ack_receiver, shutdown_rx)
                .await
        });
        // So that we can await it later on shutdown
        tokio::pin!(consumer_handle);

        // Start loader task - receives from processor, sends acks to consumer
        let loader_handle = loader.run(loader_rx, ack_tx_for_loader, Arc::clone(&metrics));

        // Orchestrator now just monitors for shutdown and metrics
        // Components communicate directly with each other
        info!("Ready to process events from Kafka - components communicating directly");

        // Set up progress logging timer (every 10 seconds)
        let metrics_ref = Arc::clone(&metrics);
        let total_events = Arc::clone(&metrics_ref.total_events_processed);
        let total_docs = Arc::clone(&metrics_ref.total_documents_indexed);
        let mut progress_timer = interval(Duration::from_secs(10));
        progress_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Track previous values for rate calculation
        let mut prev_events: u64 = 0;
        let mut prev_docs: u64 = 0;
        let mut prev_time = std::time::Instant::now();

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Received shutdown signal");
                    let _ = shutdown_tx.send(());
                    break;
                }
                _ = progress_timer.tick() => {
                    let events = total_events.load(Ordering::Relaxed);
                    let docs = total_docs.load(Ordering::Relaxed);

                    // Calculate rates per second
                    let now = std::time::Instant::now();
                    let elapsed_secs = now.duration_since(prev_time).as_secs_f64();

                    let events_per_sec = if elapsed_secs > 0.0 {
                        (events.saturating_sub(prev_events) as f64) / elapsed_secs
                    } else {
                        0.0
                    };

                    let docs_per_sec = if elapsed_secs > 0.0 {
                        (docs.saturating_sub(prev_docs) as f64) / elapsed_secs
                    } else {
                        0.0
                    };

                    info!(
                        events_processed = events,
                        documents_indexed = docs,
                        events_per_sec = format!("{:.2}", events_per_sec),
                        documents_per_sec = format!("{:.2}", docs_per_sec),
                        "Processing progress"
                    );

                    // Update previous values for next calculation
                    prev_events = events;
                    prev_docs = docs;
                    prev_time = now;
                }
            }
        }

        // Close channels to signal components to finish
        // Order matters: close producer channels first, then wait for consumers

        // Close processor channel (consumer stops sending)
        drop(processor_tx);

        // Close loader channel (processor stops sending)
        drop(loader_tx);

        // Wait for processor to finish
        let _ = processor_handle.await;

        // Wait for loader to finish processing all batches and send final acks
        let _ = loader_handle.await;

        // Close ack channel (loader stops sending, signals consumer)
        drop(ack_tx);

        // Wait for consumer to finish (it will exit when ack channel closes or on shutdown signal)
        let _ = consumer_handle.await;

        let final_events = metrics.total_events_processed.load(Ordering::Relaxed);
        let final_docs = metrics.total_documents_indexed.load(Ordering::Relaxed);
        info!(
            total_events_processed = final_events,
            total_documents_indexed = final_docs,
            "Orchestrator shutdown complete"
        );
        Ok(())
    }
}

#[async_trait]
impl Consumer for KafkaConsumer {
    fn subscribe(&self) -> Result<(), IngestError> {
        self.subscribe()
    }

    async fn run(
        &self,
        processor_tx: mpsc::Sender<ProcessingBatch>,
        ack_receiver: mpsc::Receiver<StreamMessage>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IngestError> {
        self.run(processor_tx, ack_receiver, shutdown).await
    }
}
