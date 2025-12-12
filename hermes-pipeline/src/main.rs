//! Hermes Pipeline
//!
//! Consumes space-related events from hermes-substream via hermes-relay and
//! transforms them into Hermes protobuf messages for publication to Kafka.
//!
//! ## Event Types Handled
//!
//! - `SPACE_REGISTERED` - new space registrations -> `space.creations` topic
//! - `SUBSPACE_ADDED` - trust extensions -> `space.trust.extensions` topic
//! - `SUBSPACE_REMOVED` - trust revocations -> `space.trust.extensions` topic
//! - `EDITS_PUBLISHED` - edit publications -> `knowledge.edits` topic
//!
//! ## Architecture
//!
//! The pipeline processes blocks in three parallel stages:
//! 1. **Transform**: All pipelines run concurrently to transform actions into events
//! 2. **Join**: Wait for all transformations to complete
//! 3. **Emit**: Send events to Kafka in order (spaces, trust, edits)
//!
//! ## Configuration
//!
//! Environment variables:
//! - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
//! - `KAFKA_USERNAME` - SASL username for managed Kafka (optional)
//! - `KAFKA_PASSWORD` - SASL password for managed Kafka (optional)
//! - `KAFKA_SSL_CA_PEM` - Custom CA cert for SSL (optional)

mod cache;
mod emit;
mod pipelines;

use std::env;
use std::fmt;
use std::sync::Arc;

use prost::Message;

use hermes_kafka::create_producer;
use hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData;
use hermes_relay::stream::utils;
use hermes_relay::{Actions, HermesModule, Sink, StreamSource};

use cache::MockIpfsCache;
use emit::{topics, Emitter};
use pipelines::edits::RetryConfig;
use pipelines::trust::get_extension_type;
use pipelines::BlockMetadata;

/// Error type for the pipeline that implements std::error::Error
#[derive(Debug)]
pub struct PipelineError(anyhow::Error);

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<anyhow::Error> for PipelineError {
    fn from(err: anyhow::Error) -> Self {
        PipelineError(err)
    }
}

impl From<prost::DecodeError> for PipelineError {
    fn from(err: prost::DecodeError) -> Self {
        PipelineError(anyhow::Error::from(err))
    }
}

/// Pipeline transformer that processes all space-related events.
///
/// Subscribes to `HermesModule::Actions` and processes:
/// - `SPACE_REGISTERED` -> spaces pipeline
/// - `SUBSPACE_ADDED/REMOVED` -> trust pipeline
/// - `EDITS_PUBLISHED` -> edits pipeline (with IPFS cache lookup)
///
/// All pipelines run in parallel, then events are emitted to Kafka in order.
pub struct Pipeline {
    emitter: Emitter,
    cache: Arc<MockIpfsCache>,
    retry_config: RetryConfig,
}

impl Pipeline {
    pub fn new(emitter: Emitter) -> Self {
        Self {
            emitter,
            cache: Arc::new(MockIpfsCache::new()),
            retry_config: RetryConfig::default(),
        }
    }

    /// Create a pipeline with custom retry configuration.
    #[allow(dead_code)]
    pub fn with_retry_config(emitter: Emitter, retry_config: RetryConfig) -> Self {
        Self {
            emitter,
            cache: Arc::new(MockIpfsCache::new()),
            retry_config,
        }
    }
}

impl Sink for Pipeline {
    type Error = PipelineError;

    async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
        let output = utils::output(data);
        let relay_meta = utils::block_metadata(data);
        let meta: BlockMetadata = relay_meta.clone().into();

        // Decode the Actions message from the block output
        let actions_msg = Actions::decode(output.value.as_slice())?;
        let actions = &actions_msg.actions;

        // =========================================================================
        // Phase 1: Transform all pipelines in parallel
        // =========================================================================

        // Clone what we need for the spawned tasks
        let actions_clone = actions.clone();
        let meta_clone = meta.clone();

        // Spawn spaces transform (sync, but we wrap in spawn_blocking for uniformity)
        let spaces_handle = tokio::task::spawn_blocking(move || {
            pipelines::spaces::transform(&actions_clone, &meta_clone)
        });

        let actions_clone = actions.clone();
        let meta_clone = meta.clone();

        // Spawn trust transform (sync)
        let trust_handle = tokio::task::spawn_blocking(move || {
            pipelines::trust::transform(&actions_clone, &meta_clone)
        });

        // Spawn edits transform (async - has cache lookups)
        let cache = Arc::clone(&self.cache);
        let retry_config = self.retry_config.clone();
        let actions_clone = actions.clone();
        let meta_clone = meta.clone();

        let edits_handle = tokio::spawn(async move {
            pipelines::edits::transform(&actions_clone, &meta_clone, &cache, &retry_config).await
        });

        // =========================================================================
        // Phase 2: Wait for all transforms to complete
        // =========================================================================

        let (spaces_result, trust_result, edits_result) =
            tokio::try_join!(spaces_handle, trust_handle, edits_handle)
                .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?;

        let spaces = spaces_result?;
        let trust = trust_result?;
        let edits = edits_result?;

        // =========================================================================
        // Phase 3: Emit events to Kafka in order
        // =========================================================================
        // Ordering matters here:
        // 1. Spaces must be emitted first since trust and edit events reference spaces
        // 2. Trust events come next as they define the space topology
        // 3. Edits come last as they may reference entities across trusted spaces

        // Emit spaces
        for event in &spaces.events {
            self.emitter.emit(event)?;
            println!(
                "Block {}: Space registered: {}",
                meta.block_number,
                hex::encode(&event.space_id)
            );
        }

        // Emit trust events
        for trust_event in &trust.events {
            self.emitter.emit(&trust_event.event)?;
            let action_type = if trust_event.is_removal {
                "removed"
            } else {
                "added"
            };
            println!(
                "Block {}: Subspace {}: {} -> {}",
                meta.block_number,
                action_type,
                hex::encode(&trust_event.event.source_space_id),
                get_extension_type(&trust_event.event)
            );
        }

        // Emit edits
        for event in &edits.events {
            self.emitter.emit(event)?;
            println!(
                "Block {}: Edit published: {} (space: {}, ops: {})",
                meta.block_number,
                event.name,
                event.space_id,
                event.ops.len()
            );
        }

        // Log cache issues
        if edits.cache_misses > 0 {
            println!(
                "Block {}: {} edit cache misses (retries exhausted)",
                meta.block_number, edits.cache_misses
            );
        }
        if edits.errored_entries > 0 {
            println!(
                "Block {}: {} edit entries errored in cache",
                meta.block_number, edits.errored_entries
            );
        }
        if edits.fetch_failures > 0 {
            println!(
                "Block {}: {} edit fetch failures",
                meta.block_number, edits.fetch_failures
            );
        }

        // =========================================================================
        // Log block summary
        // =========================================================================

        let space_count = spaces.events.len() as u64;
        let trust_count = trust.events.len() as u64;
        let edit_count = edits.events.len() as u64;

        let total = space_count + trust_count + edit_count;
        if total > 0 || edits.cache_misses > 0 || edits.errored_entries > 0 {
            let drift = utils::format_drift(&relay_meta);
            println!(
                "Block {} summary: {} spaces, {} trust (+{}/-{}), {} edits ({} misses, {} errored, {} failed) (drift: {})",
                meta.block_number,
                space_count,
                trust_count,
                trust.added,
                trust.removed,
                edit_count,
                edits.cache_misses,
                edits.errored_entries,
                edits.fetch_failures,
                drift
            );
        }

        Ok(())
    }

    fn process_block_undo_signal(
        &self,
        undo_signal: &hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockUndoSignal,
    ) -> std::result::Result<(), Self::Error> {
        // For now, just log the undo signal
        // In a production system, we would delete any data recorded after this block
        println!(
            "Block undo signal received: rolling back to block {}",
            undo_signal
                .last_valid_block
                .as_ref()
                .map_or(0, |b| b.number)
        );

        // TODO: Implement actual rollback logic when cursor persistence is added
        // This would involve deleting Kafka messages or updating state

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Hermes Pipeline starting...");

    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());

    println!("Configuration:");
    println!("  Kafka broker: {}", broker);

    // Create Kafka producer and wrap in Emitter
    println!("\nConnecting to Kafka broker...");
    let producer = create_producer(&broker, "hermes-pipeline")?;
    let emitter = Emitter::new(producer);
    println!("Connected to Kafka broker");

    // Create the pipeline
    let pipeline = Pipeline::new(emitter);

    println!("\nStarting pipeline with mock data...");
    println!("Subscribing to module: {}", HermesModule::Actions);
    println!("Processing: SPACE_REGISTERED, SUBSPACE_ADDED, SUBSPACE_REMOVED, EDITS_PUBLISHED");
    println!("Output topics:");
    println!("  - {} (spaces)", topics::SPACE_CREATIONS);
    println!("  - {} (trust)", topics::TRUST_EXTENSIONS);
    println!("  - {} (edits)", topics::EDITS);
    println!("\nPipeline mode: Parallel transform, sequential emit");
    println!("Edit cache: MockIpfsCache (in-memory, 6 test edits)");
    println!(
        "Retry config: {}ms initial, {}x factor, {}s max, {} retries",
        pipeline.retry_config.initial_delay_ms,
        pipeline.retry_config.factor,
        pipeline.retry_config.max_delay.as_secs(),
        pipeline.retry_config.max_retries
    );
    println!();

    // Run the pipeline with mock data
    pipeline.run(StreamSource::mock()).await?;

    println!("\nPipeline finished.");

    Ok(())
}
