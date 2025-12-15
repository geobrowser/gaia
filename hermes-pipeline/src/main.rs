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

use hermes_instrumentation::{Instrument, debug, info, info_span, warn};
use prost::Message;

use hermes_kafka::create_producer;
use hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData;
use hermes_relay::stream::utils;
use hermes_relay::{Actions, HermesModule, Sink, StreamSource};

use cache::MockIpfsCache;
use emit::{Emitter, topics};
use pipelines::BlockMetadata;
use pipelines::edits::RetryConfig;
use pipelines::trust::get_extension_type;

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

impl Pipeline {
    async fn process_block_impl(
        &self,
        output_value: &[u8],
        relay_meta: hermes_relay::stream::utils::BlockMetadata,
        meta: BlockMetadata,
    ) -> Result<(), PipelineError> {
        // Decode the Actions message from the block output
        let actions_msg = Actions::decode(output_value)?;
        let actions = &actions_msg.actions;

        // =========================================================================
        // Phase 1: Transform actions into events
        // =========================================================================

        // Sync transforms - fast, no I/O, just run them inline
        let spaces = info_span!("transform.spaces", action_count = actions.len())
            .in_scope(|| pipelines::spaces::transform(actions, &meta))?;

        let trust = info_span!("transform.trust", action_count = actions.len())
            .in_scope(|| pipelines::trust::transform(actions, &meta))?;

        // Async transform - has cache lookups with retries
        let edits = pipelines::edits::transform(actions, &meta, &self.cache, &self.retry_config)
            .instrument(info_span!("transform.edits", action_count = actions.len()))
            .await?;

        // =========================================================================
        // Phase 3: Emit events to Kafka in order
        // =========================================================================
        // Ordering matters here:
        // 1. Spaces must be emitted first since trust and edit events reference spaces
        // 2. Trust events come next as they define the space topology
        // 3. Edits come last as they may reference entities across trusted spaces

        {
            let _emit_span = info_span!("emit").entered();

            // Emit spaces
            {
                let _span = info_span!("emit.spaces", count = spaces.events.len()).entered();
                for event in &spaces.events {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        "Space registered"
                    );
                }
            }

            // Emit trust events
            {
                let _span = info_span!("emit.trust", count = trust.events.len()).entered();
                for trust_event in &trust.events {
                    self.emitter.emit(&trust_event.event)?;
                    debug!(
                        source = %hex::encode(&trust_event.event.source_space_id),
                        extension_type = get_extension_type(&trust_event.event),
                        is_removal = trust_event.is_removal,
                        "Trust event emitted"
                    );
                }
            }

            // Emit edits
            {
                let _span = info_span!("emit.edits", count = edits.events.len()).entered();
                for event in &edits.events {
                    self.emitter.emit(event)?;
                    debug!(
                        name = %event.name,
                        space_id = %event.space_id,
                        ops_count = event.ops.len(),
                        "Edit published"
                    );
                }
            }
        }

        // Log cache issues
        if edits.cache_misses > 0 {
            warn!(
                count = edits.cache_misses,
                "Edit cache misses (retries exhausted)"
            );
        }
        if edits.errored_entries > 0 {
            warn!(
                count = edits.errored_entries,
                "Edit entries errored in cache"
            );
        }
        if edits.fetch_failures > 0 {
            warn!(count = edits.fetch_failures, "Edit fetch failures");
        }

        // Log block summary
        let space_count = spaces.events.len() as u64;
        let trust_count = trust.events.len() as u64;
        let edit_count = edits.events.len() as u64;
        let total = space_count + trust_count + edit_count;

        if total > 0 || edits.cache_misses > 0 || edits.errored_entries > 0 {
            info!(
                spaces = space_count,
                trust_added = trust.added,
                trust_removed = trust.removed,
                edits = edit_count,
                cache_misses = edits.cache_misses,
                errored_entries = edits.errored_entries,
                fetch_failures = edits.fetch_failures,
                drift = %utils::format_drift(&relay_meta),
                "Block processed"
            );
        }

        Ok(())
    }
}

impl Sink for Pipeline {
    type Error = PipelineError;

    async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
        let output = utils::output(data);
        let relay_meta = utils::block_metadata(data);
        let meta: BlockMetadata = relay_meta.clone().into();

        let span = info_span!(
            "process_block",
            block_number = meta.block_number,
            cursor = %meta.cursor
        );

        self.process_block_impl(output.value.as_slice(), relay_meta, meta)
            .instrument(span)
            .await
    }

    fn process_block_undo_signal(
        &self,
        undo_signal: &hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockUndoSignal,
    ) -> std::result::Result<(), Self::Error> {
        // For now, just log the undo signal
        // In a production system, we would delete any data recorded after this block
        let last_valid_block = undo_signal
            .last_valid_block
            .as_ref()
            .map_or(0, |b| b.number);
        warn!(
            last_valid_block,
            "Block undo signal received, rollback required"
        );

        // TODO: Implement actual rollback logic when cursor persistence is added
        // This would involve deleting Kafka messages or updating state

        Ok(())
    }
}

/// Build telemetry configuration from environment variables.
///
/// Environment variables:
/// - `OTEL_URL` - OTLP HTTP endpoint (e.g., "https://api.axiom.co/v1/traces")
/// - `OTEL_TOKEN` - Bearer token for authentication
/// - `OTEL_DATASET` - Dataset name (sent as X-Axiom-Dataset header)
/// - `OTEL_DEBUG` - Set to "true" to also emit spans to stdout
///
/// If `OTEL_URL` is not set, falls back to Console backend.
fn build_telemetry_config() -> hermes_instrumentation::Config {
    use hermes_instrumentation::{Backend, Config};

    let backend = match env::var("OTEL_URL") {
        Ok(endpoint) => {
            let mut headers = Vec::new();

            if let Ok(token) = env::var("OTEL_TOKEN") {
                headers.push(("Authorization".into(), format!("Bearer {}", token)));
            }

            let dataset = env::var("OTEL_DATASET").ok();
            if let Some(ref dataset) = dataset {
                headers.push(("X-Axiom-Dataset".into(), dataset.clone()));
            }

            let debug = env::var("OTEL_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            let has_auth = headers.iter().any(|(k, _)| k == "Authorization");
            println!(
                "Telemetry: OTLP HTTP -> {} (dataset: {}, auth: {}, debug: {})",
                endpoint,
                dataset.as_deref().unwrap_or("none"),
                if has_auth { "yes" } else { "no" },
                if debug { "yes" } else { "no" }
            );

            Backend::OtlpHttp {
                endpoint,
                headers,
                debug,
            }
        }
        _ => {
            println!("Telemetry: Console (set OTEL_URL to enable OTLP export)");
            Backend::Console
        }
    };

    Config::new("hermes-pipeline", backend)
}

fn main() -> anyhow::Result<()> {
    // Load .env file if present (ignored in production)
    dotenv::dotenv().ok();

    // Initialize telemetry BEFORE tokio runtime starts.
    // The OTLP HTTP backend uses a blocking HTTP client that creates its own
    // internal tokio runtime. Tokio runtimes cannot be nested, so we must
    // initialize telemetry before creating our application's runtime.
    //
    // Keep the guard alive until the end of main to ensure spans are flushed.
    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    // Create and run the tokio runtime manually (instead of #[tokio::main])
    // - new_multi_thread(): Uses a thread pool for parallel task execution,
    //   which is appropriate for I/O-bound services like this pipeline
    // - enable_all(): Enables both I/O and time drivers, required for
    //   network operations (Kafka, IPFS) and timeouts/delays
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    info!("Hermes Pipeline starting");

    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    info!(kafka_broker = %broker, "Configuration loaded");

    // Create Kafka producer and wrap in Emitter
    debug!("Connecting to Kafka broker");
    let producer = create_producer(&broker, "hermes-pipeline")?;
    let emitter = Emitter::new(producer);
    info!("Connected to Kafka broker");

    // Create the pipeline
    let pipeline = Pipeline::new(emitter);

    info!(
        module = %HermesModule::Actions,
        topics.spaces = topics::SPACE_CREATIONS,
        topics.trust = topics::TRUST_EXTENSIONS,
        topics.edits = topics::EDITS,
        retry_initial_ms = pipeline.retry_config.initial_delay_ms,
        retry_factor = pipeline.retry_config.factor,
        retry_max_secs = pipeline.retry_config.max_delay.as_secs(),
        retry_max_count = pipeline.retry_config.max_retries,
        "Starting pipeline"
    );

    // Run the pipeline with mock data
    pipeline.run(StreamSource::mock()).await?;

    info!("Pipeline finished");

    Ok(())
}
