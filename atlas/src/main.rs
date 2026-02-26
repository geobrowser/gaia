//! Atlas - Space Topology Processor
//!
//! Entry point for the Atlas graph processing pipeline.
//! Consumes space topology events from hermes-relay, computes canonical graphs,
//! and publishes updates to Kafka.
//!
//! ## Configuration
//!
//! Environment variables:
//!
//! ### Substream Configuration
//! - `USE_MOCK` - Use mock data source instead of live substream (default: false)
//! - `SUBSTREAMS_ENDPOINT` - Substream endpoint URL (default: geotest.substreams.pinax.network:443)
//! - `SUBSTREAMS_START_BLOCK` - Start block number (default: 82655, Space Registry deployment)
//! - `SUBSTREAMS_END_BLOCK` - End block number (default: u64::MAX for continuous streaming)
//! - `SUBSTREAMS_API_TOKEN` - Optional API token for authenticated endpoints
//!
//! ### Graph Configuration
//! - `ROOT_SPACE_ID` - **Required.** Root space ID as a 32-char hex string (16 bytes). Varies per environment.
//!
//! ### Kafka Configuration
//! - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
//! - `KAFKA_TOPIC` - Output topic for canonical graph updates (default: topology.canonical)
//!
//! ### Telemetry Configuration
//! - `SENTRY_DSN` - Sentry DSN/ingest URL
//! - `SENTRY_TRACES_SAMPLE_RATE` - Sampling rate (0.0 - 1.0)
//! - `SENTRY_SEND_DEFAULT_PII` - Set to "true" to include PII
//! - `SENTRY_ENVIRONMENT` - Environment tag (e.g., "prod", "staging")
//! - `SENTRY_RELEASE` - Release name (e.g., "service@1.2.3")
//! - `SENTRY_DEBUG` - Set to "true" to also emit spans to stdout

use std::env;
use std::sync::Mutex;

use atlas::convert::convert_action;
use atlas::events::{BlockMetadata, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload};
use atlas::graph::{CanonicalProcessor, DiffTracker, GraphState, TransitiveProcessor};
use atlas::kafka::{AtlasProducer, CanonicalGraphEmitter};
use atlas::persistence::{CheckpointConfig, CheckpointManager, PersistedGraphState};
use hermes_instrumentation::{debug, info, info_span, Instrument};
use hermes_relay::{Actions, HermesModule, Sink, StreamSource};
use prost::Message;

/// Mutable pipeline state behind a single Mutex.
///
/// Processing is single-threaded (one block, one event at a time), so all
/// mutable state can live behind one lock. The Mutex exists only because
/// `Sink` requires `&self` for `Send + Sync`.
struct PipelineState {
    graph: GraphState,
    transitive: TransitiveProcessor,
    canonical: CanonicalProcessor,
    diff_tracker: DiffTracker,
    event_count: usize,
    emit_count: usize,
}

/// Atlas topology processor that implements the hermes-relay Sink trait.
struct AtlasSink {
    /// All mutable pipeline state behind a single lock
    state: Mutex<PipelineState>,
    /// Kafka emitter for canonical graph updates (internally thread-safe)
    emitter: CanonicalGraphEmitter,
    /// Checkpoint manager for restore/persist/fail-open handling
    checkpoint_manager: CheckpointManager,
}

impl AtlasSink {
    async fn new(root_space: SpaceId, emitter: CanonicalGraphEmitter) -> anyhow::Result<Self> {
        let mut checkpoint_manager = CheckpointManager::from_env(root_space, 1)?;
        let restored_state = checkpoint_manager.restore_checkpoint_on_startup().await?;

        let graph = restored_state.unwrap_or_else(GraphState::new);
        let mut transitive = TransitiveProcessor::new();
        let mut canonical = CanonicalProcessor::new(root_space);
        let mut diff_tracker = DiffTracker::new();

        if checkpoint_manager.restored_cursor().is_some() {
            if let Some(restored_graph) = canonical.compute_if_changed(&graph, &mut transitive) {
                let _ = diff_tracker.track(&restored_graph);
                info!(
                    restored_cursor = checkpoint_manager.restored_cursor().is_some(),
                    warmed_nodes = restored_graph.len(),
                    "Warmed canonical/transitive/diff caches from restored checkpoint state"
                );
            }
        }

        Ok(Self {
            state: Mutex::new(PipelineState {
                graph,
                transitive,
                canonical,
                diff_tracker,
                event_count: 0,
                emit_count: 0,
            }),
            emitter,
            checkpoint_manager,
        })
    }

    fn summary(&self) {
        let s = self.state.lock().unwrap();

        info!(
            spaces = s.graph.space_count(),
            explicit_edges = s.graph.explicit_edge_count(),
            topic_edges = s.graph.topic_edge_count(),
            kafka_messages = s.emit_count,
            "Processing complete"
        );
    }
}

#[derive(Debug, thiserror::Error)]
enum AtlasError {
    #[error("Failed to decode actions: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("Kafka error: {0}")]
    Kafka(#[from] atlas::kafka::ProducerError),
    #[error("Checkpoint error: {0}")]
    Checkpoint(String),
}

impl Sink for AtlasSink {
    type Error = AtlasError;

    async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
        Ok(self.checkpoint_manager.restored_cursor())
    }

    async fn process_block_scoped_data(
        &self,
        data: &hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData,
    ) -> Result<(), Self::Error> {
        self.checkpoint_manager
            .wait_for_persistence_recovery_if_paused()
            .await
            .map_err(|err| AtlasError::Checkpoint(err.to_string()))?;

        // Extract block metadata
        let clock = data.clock.as_ref();
        let block_number = clock.map(|c| c.number).unwrap_or(0);
        let block_timestamp = clock
            .and_then(|c| c.timestamp.as_ref())
            .map(|t| t.seconds as u64)
            .unwrap_or(0);

        let meta = BlockMetadata {
            block_number,
            block_timestamp,
            tx_hash: String::new(),
            cursor: data.cursor.clone(),
        };

        // Decode actions from the block output
        let output = data
            .output
            .as_ref()
            .and_then(|o| o.map_output.as_ref())
            .map(|a| a.value.as_slice())
            .unwrap_or(&[]);

        if output.is_empty() {
            return Ok(());
        }

        let actions = Actions::decode(output)?;

        // Process all events in the block, then compute and emit once.
        //
        // This batching model is intentional and load-bearing:
        // - Avoids per-event intermediate diffs within the same block.
        // - Emits exactly one net diff for the block's final state.
        // - Keeps consumer replay stable (apply one atomic batch per block).
        //
        // Pipeline shape (per block):
        //   for event in block:
        //     1) affects_canonical(event) on pre-mutation canonical set
        //     2) transitive.handle_event(event, pre-mutation graph)
        //     3) graph.apply_event(event)
        //   then:
        //     compute canonical once -> diff once -> emit once
        let action_count = actions.actions.len();
        let processed_events = async {
            let mut s = self.state.lock().unwrap();
            let PipelineState {
                graph,
                transitive,
                canonical,
                diff_tracker,
                event_count,
                emit_count,
            } = &mut *s;

            // Phase 1: Apply all events to graph state and transitive cache.
            // Track whether any event in this block may affect the canonical graph.
            // This flag is intentionally conservative: false means "safe to skip",
            // true means "compute once at end of block and decide from hashes/diff".
            let mut block_may_affect = false;
            let mut processed_events = 0usize;

            for action in &actions.actions {
                if let Some(event) = convert_action(action, &meta) {
                    processed_events += 1;
                    let event_type = match &event.payload {
                        SpaceTopologyPayload::SpaceCreated(_) => "SpaceCreated",
                        SpaceTopologyPayload::TrustExtended(_) => "TrustExtended",
                    };

                    let _span = info_span!("apply_event", event_type).entered();

                    log_event(*event_count, &event);
                    *event_count += 1;

                    // Check before mutation — affects_canonical reads pre-mutation
                    // canonical set (same as the transitive cache invalidation order)
                    if canonical.affects_canonical(&event) {
                        block_may_affect = true;
                    }

                    // Order matters: transitive reads pre-mutation state for cache
                    // invalidation, then graph state is updated.
                    transitive.handle_event(&event, graph);
                    graph.apply_event(&event);
                }
            }

            // Phase 2: Compute canonical graph once and emit a single diff for the block.
            if !block_may_affect {
                return Ok(processed_events);
            }

            if let Some(new_graph) = canonical.compute_if_changed(graph, transitive) {
                let diff = diff_tracker.track(&new_graph);

                if !diff.is_empty() {
                    let change_count = diff.len();
                    self.emitter.emit_diff(&new_graph.root, &diff, &meta)?;
                    *emit_count += 1;

                    let added = diff
                        .changes
                        .iter()
                        .filter(|c| c.change_type == atlas::graph::ChangeType::Added)
                        .count();
                    let removed = diff
                        .changes
                        .iter()
                        .filter(|c| c.change_type == atlas::graph::ChangeType::Removed)
                        .count();
                    let moved = diff
                        .changes
                        .iter()
                        .filter(|c| c.change_type == atlas::graph::ChangeType::Moved)
                        .count();

                    // Show up to 5 affected space IDs for debuggability; truncate for large diffs
                    let sample: Vec<String> = diff
                        .changes
                        .iter()
                        .take(5)
                        .map(|c| format!("{}:{:?}", hex::encode(c.space_id), c.change_type))
                        .collect();
                    let truncated = if diff.len() > 5 {
                        format!(" (+{} more)", diff.len() - 5)
                    } else {
                        String::new()
                    };

                    info!(
                        block_number,
                        change_count,
                        added,
                        removed,
                        moved,
                        node_count = new_graph.len(),
                        changes = %format!("[{}]{}", sample.join(", "), truncated),
                        "Emitted canonical graph diff"
                    );
                }
            }

            Ok::<usize, AtlasError>(processed_events)
        }
        .instrument(info_span!(
            "process_block",
            block_number,
            cursor = %meta.cursor,
            action_count
        ))
        .await?;

        if processed_events > 0 {
            let persisted_snapshot = {
                let state = self.state.lock().unwrap();
                PersistedGraphState::from(&state.graph)
            };

            self.checkpoint_manager
                .persist_block_checkpoint(block_number, meta.cursor.clone(), persisted_snapshot)
                .await;
        }

        Ok(())
    }
}

/// Build telemetry configuration from environment variables.
///
/// Environment variables:
/// - `SENTRY_DSN` - Sentry DSN/ingest URL
/// - `SENTRY_TRACES_SAMPLE_RATE` - Sampling rate (0.0 - 1.0)
/// - `SENTRY_SEND_DEFAULT_PII` - Set to "true" to include PII
/// - `SENTRY_ENVIRONMENT` - Environment tag (e.g., "prod", "staging")
/// - `SENTRY_RELEASE` - Release name (e.g., "service@1.2.3")
/// - `SENTRY_DEBUG` - Set to "true" to also emit spans to stdout
///
/// If `SENTRY_DSN` is not set, falls back to Console backend.
fn build_telemetry_config() -> hermes_instrumentation::Config {
    use hermes_instrumentation::{Backend, Config};

    let backend = match env::var("SENTRY_DSN") {
        Ok(dsn) => {
            let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            let send_default_pii = env::var("SENTRY_SEND_DEFAULT_PII")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);
            let environment = env::var("SENTRY_ENVIRONMENT").ok();
            let release = env::var("SENTRY_RELEASE").ok();
            let debug = env::var("SENTRY_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            println!(
                "Telemetry: Sentry (env: {}, release: {}, debug: {})",
                environment.as_deref().unwrap_or("none"),
                release.as_deref().unwrap_or("none"),
                if debug { "yes" } else { "no" }
            );

            Backend::Sentry {
                dsn,
                traces_sample_rate,
                send_default_pii,
                environment,
                release,
                debug,
                axiom: hermes_instrumentation::AxiomConfig::from_env(),
            }
        }
        _ => {
            println!("Telemetry: Console (set SENTRY_DSN to enable Sentry)");
            Backend::Console
        }
    };

    Config::new("atlas", backend)
}

/// Parse ROOT_SPACE_ID from environment variable.
///
/// Expects a 32-character hex string (16 bytes) representing the root space ID.
/// This varies per environment and must be set explicitly.
fn parse_root_space_id() -> anyhow::Result<SpaceId> {
    let hex_str = env::var("ROOT_SPACE_ID")
        .map_err(|_| anyhow::anyhow!("ROOT_SPACE_ID env var is required but not set"))?;

    let bytes = hex::decode(&hex_str).map_err(|e| {
        anyhow::anyhow!(
            "ROOT_SPACE_ID must be a valid hex string: {e} (got {len} chars)",
            len = hex_str.len()
        )
    })?;

    bytes.try_into().map_err(|v: Vec<u8>| {
        anyhow::anyhow!(
            "ROOT_SPACE_ID must be exactly 16 bytes (32 hex chars), got {} bytes",
            v.len()
        )
    })
}

fn main() -> anyhow::Result<()> {
    // Load .env file if present (ignored in production)
    dotenvy::dotenv().ok();

    // Initialize telemetry BEFORE tokio runtime starts.
    // Keep the guard alive until the end of main to ensure spans are flushed.
    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    // Create and run the tokio runtime manually (instead of #[tokio::main])
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

// Re-export get_topic_prefix from hermes-kafka
use hermes_kafka::get_topic_prefix;

async fn async_main() -> anyhow::Result<()> {
    // Kafka configuration
    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    // Topic contract: ENVIRONMENT controls prefixing (staging. vs production "").
    let topic_prefix = get_topic_prefix();
    let base_topic = env::var("KAFKA_TOPIC").unwrap_or_else(|_| "topology.canonical".to_string());
    let topic = format!("{}{}", topic_prefix, base_topic);

    // Substream configuration
    let use_mock = env::var("USE_MOCK")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    let endpoint = env::var("SUBSTREAMS_ENDPOINT")
        .unwrap_or_else(|_| "geotest.substreams.pinax.network:443".to_string());
    let start_block: i64 = env::var("SUBSTREAMS_START_BLOCK")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(82655); // Space Registry deployment block
    let end_block: u64 = env::var("SUBSTREAMS_END_BLOCK")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(u64::MAX);

    // Root space configuration
    let root_space_id = parse_root_space_id()?;

    // Validate checkpoint environment invariants before external connections.
    // This catches misconfiguration (e.g. missing ATLAS_INDEXER_ID when checkpoint
    // persistence is enabled) before we connect to Kafka.
    let _checkpoint_preflight = CheckpointConfig::from_env(root_space_id, 1)?;

    info!("Atlas Topology Processor starting");
    info!(
        kafka_broker = %broker,
        kafka_topic = %topic,
        topic_prefix = %topic_prefix,
        use_mock,
        "Configuration loaded"
    );

    // Set up Kafka producer
    debug!("Connecting to Kafka broker");
    let producer = AtlasProducer::new(&broker, &topic)?;
    let emitter = CanonicalGraphEmitter::new(producer);
    info!("Connected to Kafka broker");

    let sink = AtlasSink::new(root_space_id, emitter).await?;

    info!("Starting event processing");

    // Create stream source based on configuration
    let source = if use_mock {
        info!("Using mock data source");
        StreamSource::mock()
    } else {
        info!(
            endpoint = %endpoint,
            start_block,
            end_block,
            "Using live substream"
        );
        StreamSource::live(endpoint, HermesModule::Actions, start_block, end_block)
    };

    sink.run(source).await?;

    sink.summary();

    info!("Atlas processing complete");

    Ok(())
}

/// Format a space ID as a short hex string (with friendly test names in test builds)
fn format_space_id(id: SpaceId) -> String {
    #[cfg(test)]
    {
        let last_byte = id[15];
        let name = match last_byte {
            0x01 => "Root",
            0x0A => "A",
            0x0B => "B",
            0x0C => "C",
            0x0D => "D",
            0x0E => "E",
            0x0F => "F",
            0x10 => "G",
            0x11 => "H",
            0x12 => "I",
            0x13 => "J",
            0x20 => "X",
            0x21 => "Y",
            0x22 => "Z",
            0x23 => "W",
            0x30 => "P",
            0x31 => "Q",
            0x40 => "S",
            _ => return format!("{:.8}...", hex::encode(id)),
        };
        format!("{} (0x{:02x})", name, last_byte)
    }
    #[cfg(not(test))]
    format!("{:.8}...", hex::encode(id))
}

/// Format a topic ID as a short hex string (with friendly test names in test builds)
fn format_topic_id(id: &[u8; 16]) -> String {
    #[cfg(test)]
    {
        let last_byte = id[15];
        let name = match last_byte {
            0x02 => "T_Root",
            0x8A => "T_A",
            0x8B => "T_B",
            0x8C => "T_C",
            0x8D => "T_D",
            0x8E => "T_E",
            0x8F => "T_F",
            0x90 => "T_G",
            0x91 => "T_H",
            0x92 => "T_I",
            0x93 => "T_J",
            0xA0 => "T_X",
            0xA1 => "T_Y",
            0xA2 => "T_Z",
            0xA3 => "T_W",
            0xB0 => "T_P",
            0xB1 => "T_Q",
            0xC0 => "T_S",
            0xF0 => "T_SHARED",
            _ => return format!("{:.8}...", hex::encode(id)),
        };
        format!("{} (0x{:02x})", name, last_byte)
    }
    #[cfg(not(test))]
    format!("{:.8}...", hex::encode(id))
}

/// Log a topology event with structured fields
fn log_event(index: usize, event: &SpaceTopologyEvent) {
    match &event.payload {
        SpaceTopologyPayload::SpaceCreated(created) => {
            debug!(
                index,
                space_id = %format_space_id(created.space_id),
                topic_id = %format_topic_id(&created.topic_id),
                "SpaceCreated"
            );
        }
        SpaceTopologyPayload::TrustExtended(extended) => {
            let (extension_type, target) = match &extended.extension {
                atlas::events::TrustExtension::Verified { target_space_id } => {
                    ("verified", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Related { target_space_id } => {
                    ("related", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Subtopic { target_topic_id } => {
                    ("topic", format_topic_id(target_topic_id))
                }
                atlas::events::TrustExtension::EditorAdded { member_space_id } => {
                    ("editor_added", format_space_id(*member_space_id))
                }
                atlas::events::TrustExtension::MemberAdded { member_space_id } => {
                    ("member_added", format_space_id(*member_space_id))
                }
                atlas::events::TrustExtension::VerifiedRemoved { target_space_id } => {
                    ("verified_removed", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::RelatedRemoved { target_space_id } => {
                    ("related_removed", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::EditorRemoved { member_space_id } => {
                    ("editor_removed", format_space_id(*member_space_id))
                }
                atlas::events::TrustExtension::MemberRemoved { member_space_id } => {
                    ("member_removed", format_space_id(*member_space_id))
                }
                atlas::events::TrustExtension::SubtopicRemoved { target_topic_id } => {
                    ("topic_removed", format_topic_id(target_topic_id))
                }
            };
            debug!(
                index,
                source_space_id = %format_space_id(extended.source_space_id),
                extension_type,
                target = %target,
                "TrustExtended"
            );
        }
    }
}
