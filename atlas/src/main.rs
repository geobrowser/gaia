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
//! - `SUBSTREAMS_END_BLOCK` - End block number (default: 0, the Substreams
//!   sentinel for "stream forever"). Do not set this to `u64::MAX`.
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
use atlas::persistence::{
    CheckpointConfig, CheckpointManager, PersistedEmissionBaseline, PersistedGraphState,
    RestoredCheckpoint,
};
use atlas::stall::{self, StreamProgress};
use hermes_instrumentation::{debug, info, info_span, warn, Instrument};
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
    /// True until atlas has persisted an emission baseline at least once.
    /// Drives the "force-write on first canonical compute" requirement: a
    /// quiet startup must still leave a baseline on disk so the next deploy
    /// has something to load. See GEO-645.
    baseline_force_write_pending: bool,
}

/// Atlas topology processor that implements the hermes-relay Sink trait.
struct AtlasSink {
    /// All mutable pipeline state behind a single lock
    state: Mutex<PipelineState>,
    /// Kafka emitter for canonical graph updates (internally thread-safe)
    emitter: CanonicalGraphEmitter,
    /// Checkpoint manager for restore/persist/fail-open handling
    checkpoint_manager: CheckpointManager,
    /// Last block the substream delivered, watched by the stall detector.
    /// Lock-free so the block handler pays almost nothing to update it.
    progress: StreamProgress,
}

impl AtlasSink {
    async fn new(root_space: SpaceId, emitter: CanonicalGraphEmitter) -> anyhow::Result<Self> {
        let mut checkpoint_manager = CheckpointManager::from_env(root_space, 1)?;
        let RestoredCheckpoint {
            graph_state,
            baseline,
        } = checkpoint_manager.restore_checkpoint_on_startup().await?;

        let graph = graph_state.unwrap_or_else(GraphState::new);
        let mut transitive = TransitiveProcessor::new();
        let mut canonical = CanonicalProcessor::new(root_space);

        // If we restored a baseline, prime DiffTracker from it — the first
        // post-restart track() will diff against what consumers last saw,
        // emitting REMOVED for orphans across a rules change. Otherwise the
        // tracker starts empty; we'll fall back to "warm from restored
        // canonical" below (legacy behaviour) and mark a force-write so the
        // baseline gets persisted ASAP.
        let baseline_initially_present = baseline.is_some();
        let mut diff_tracker = match &baseline {
            Some(b) => {
                info!(
                    baseline_node_count = b.len(),
                    "DiffTracker primed from persisted emission baseline"
                );
                DiffTracker::from_baseline(b)
            }
            None => DiffTracker::new(),
        };

        if checkpoint_manager.restored_cursor().is_some() {
            if let Some(restored_graph) = canonical.compute_if_changed(&graph, &mut transitive) {
                if baseline.is_none() {
                    // No persisted baseline. Use the just-computed canonical
                    // as the emission baseline so we don't emit a spurious
                    // bootstrap-all-ADDED diff on the next track(). This
                    // matches the pre-GEO-645 behaviour for the
                    // "checkpoint exists, no baseline column yet" case
                    // (i.e. the first run after this migration ships).
                    let _ = diff_tracker.track(&restored_graph);
                    info!(
                        warmed_nodes = restored_graph.len(),
                        "Warmed DiffTracker from restored canonical (no persisted baseline yet)"
                    );
                } else {
                    // Baseline already loaded; don't burn the one-shot
                    // pending_baseline diff on the warming compute. The
                    // first real per-block track() handles it.
                    info!(
                        restored_canonical_nodes = restored_graph.len(),
                        baseline_node_count = baseline.as_ref().map(|b| b.len()).unwrap_or(0),
                        "Restored canonical computed; DiffTracker stays primed from persisted baseline"
                    );
                }
            }
        }

        let baseline_force_write_pending = !baseline_initially_present;

        let mut sink = Self {
            state: Mutex::new(PipelineState {
                graph,
                transitive,
                canonical,
                diff_tracker,
                event_count: 0,
                emit_count: 0,
                baseline_force_write_pending,
            }),
            emitter,
            // Seed from the restored checkpoint. The stall detector measures lag
            // against the chain tip, so an unseeded restart during an idle chain
            // would compute the entire chain height as lag and restart a
            // perfectly healthy process on a loop.
            progress: match checkpoint_manager.restored_block_number() {
                Some(block_number) => StreamProgress::seeded(block_number),
                None => StreamProgress::new(),
            },
            checkpoint_manager,
        };

        // Force-write on startup: if there's no baseline on disk yet but we
        // already have an emission state (from warming above), persist it
        // immediately. Without this, an atlas with no incoming events between
        // startup and the next restart would never write a baseline, leaving
        // the next rules-change deploy nothing to load. See GEO-645.
        sink.force_write_baseline_if_pending().await;

        Ok(sink)
    }

    /// Persist the current emission state as a baseline if one hasn't been
    /// persisted yet. Idempotent — clears the pending flag only on a
    /// confirmed successful DB write, and is safe to call repeatedly. Skips
    /// if there is no restored cursor (fresh start; the first per-block
    /// persist handles it instead).
    ///
    /// On persist failure the pending flag stays `true` so the per-block
    /// retry path in `process_block_scoped_data` can try again on a quiet
    /// stream, even when no canonical-affecting events arrive.
    async fn force_write_baseline_if_pending(&mut self) {
        let needs_write = {
            let state = self.state.lock().unwrap();
            state.baseline_force_write_pending
        };
        if !needs_write {
            return;
        }
        let Some(cursor) = self.checkpoint_manager.restored_cursor() else {
            // Fresh start: no cursor yet, defer to the per-block persist path.
            return;
        };
        let Some(block_number) = self.checkpoint_manager.restored_block_number() else {
            return;
        };

        let (snapshot, baseline) = {
            let state = self.state.lock().unwrap();
            (
                PersistedGraphState::from(&state.graph),
                PersistedEmissionBaseline::from_diff_tracker(&state.diff_tracker),
            )
        };

        let Some(baseline) = baseline else {
            // Nothing to persist yet (no canonical compute has happened).
            return;
        };

        info!(
            block_number,
            cursor = %cursor,
            baseline_node_count = baseline.len(),
            "Force-writing emission baseline on startup (none persisted before)"
        );

        match self
            .checkpoint_manager
            .persist_block_checkpoint(block_number, cursor, snapshot, Some(&baseline))
            .await
        {
            Ok(()) => {
                let mut state = self.state.lock().unwrap();
                state.baseline_force_write_pending = false;
                info!(
                    block_number,
                    baseline_node_count = baseline.len(),
                    "Emission baseline persisted for the first time"
                );
            }
            Err(err) => {
                // Keep `baseline_force_write_pending = true` so the per-block
                // retry path can try again. CheckpointManager has already
                // recorded the failure for fail-open accounting.
                warn!(
                    block_number,
                    reason = %err,
                    "Startup force-write of emission baseline failed; will retry on next block"
                );
            }
        }
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

    /// Per-block retry of the GEO-645 baseline force-write.
    ///
    /// Runs on every incoming block (empty-output blocks, quiet blocks with no
    /// decoded actions, and quiet blocks where `processed_events == 0`) so a
    /// fully quiet stream can still recover from a transient DB failure that
    /// caused the startup force-write (and any earlier per-block writes) to
    /// fail. Without this hoist, a stream that produced no events after a
    /// failed startup force-write would never persist a baseline.
    ///
    /// The current block's `block_number` / `cursor` are passed in so the
    /// persisted checkpoint cursor matches the in-memory graph_state +
    /// baseline snapshot we are writing — using `restored_cursor` /
    /// `restored_block_number` here would rewind the resume cursor on disk
    /// even though earlier blocks may have mutated the graph (fail-open).
    ///
    /// Semantics:
    /// - No-op when `baseline_force_write_pending` is already false.
    /// - No-op when no emission baseline is available yet (canonical compute
    ///   has not produced output, so `from_diff_tracker` returns None).
    /// - On success: clears the flag and logs
    ///   `"Emission baseline persisted for the first time"`.
    /// - On failure: leaves the flag set; the next block will try again.
    async fn retry_force_write_baseline(&self, block_number: u64, cursor: &str) {
        let (snapshot, baseline) = {
            let state = self.state.lock().unwrap();
            if !state.baseline_force_write_pending {
                return;
            }
            let baseline = PersistedEmissionBaseline::from_diff_tracker(&state.diff_tracker);
            let Some(baseline) = baseline else {
                // No canonical compute yet — nothing to persist.
                return;
            };
            let snapshot = PersistedGraphState::from(&state.graph);
            (snapshot, baseline)
        };

        info!(
            block_number,
            cursor = %cursor,
            baseline_node_count = baseline.len(),
            "Retrying force-write of emission baseline on quiet block"
        );

        match self
            .checkpoint_manager
            .persist_block_checkpoint(block_number, cursor.to_string(), snapshot, Some(&baseline))
            .await
        {
            Ok(()) => {
                let mut state = self.state.lock().unwrap();
                if state.baseline_force_write_pending {
                    state.baseline_force_write_pending = false;
                    info!(
                        block_number,
                        baseline_node_count = baseline.len(),
                        "Emission baseline persisted for the first time"
                    );
                }
            }
            Err(err) => {
                warn!(
                    block_number,
                    reason = %err,
                    "Quiet-block retry of emission baseline force-write failed; will retry on next block"
                );
            }
        }
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

        // Mark progress before any work: the watchdog compares this against the
        // chain tip, and cares only about our position. Blocks with no events and
        // long idle gaps between blocks are both normal on this chain; falling
        // behind blocks that exist is the failure.
        self.progress.record_block(block_number);

        // Same gauge the hermes services publish, so the existing
        // HermesBehindChainTip alert covers atlas too rather than needing a
        // parallel rule. This is the outside-the-process check: it still fires if
        // the stall detector itself fails to notice.
        hermes_instrumentation::metrics::set_latest_processed_block(block_number);
        if block_timestamp > 0 {
            hermes_instrumentation::metrics::set_latest_processed_block_timestamp(
                block_timestamp as i64,
            );
        }

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
            // Empty-output block: no decoded actions at all. Still attempt the
            // baseline force-write retry so a fully quiet stream can recover
            // from a failed startup force-write without waiting for an
            // eventful block.
            self.retry_force_write_baseline(block_number, &meta.cursor)
                .await;
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
                // The force-write flag is read/written on the per-block
                // persist path below, which re-locks the state — not in this
                // event loop. Keeping it out of the destructure documents
                // that intent.
                baseline_force_write_pending: _,
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
            let (persisted_snapshot, emission_baseline) = {
                let state = self.state.lock().unwrap();
                (
                    PersistedGraphState::from(&state.graph),
                    PersistedEmissionBaseline::from_diff_tracker(&state.diff_tracker),
                )
            };

            // Atomicity: graph_state and baseline are written in the same
            // INSERT, so they cannot diverge. If `emission_baseline` is None
            // (no canonical compute has ever happened yet, e.g. on a
            // fresh-start atlas waiting for the first canonical-affecting
            // event), the SQL `COALESCE` preserves whatever baseline is
            // already on disk — so we never overwrite a good baseline with
            // NULL.
            let persist_result = self
                .checkpoint_manager
                .persist_block_checkpoint(
                    block_number,
                    meta.cursor.clone(),
                    persisted_snapshot,
                    emission_baseline.as_ref(),
                )
                .await;

            // Clear the force-write flag only after a confirmed successful
            // write. Cheap, idempotent — the flag exists to drive the
            // explicit startup force-write; once the per-block path has
            // actually persisted a baseline there's nothing further to do.
            // On failure we leave the flag set so a subsequent quiet block
            // (handled below) or the next eventful block can retry.
            if persist_result.is_ok() {
                if let Some(baseline) = emission_baseline.as_ref() {
                    let mut state = self.state.lock().unwrap();
                    if state.baseline_force_write_pending {
                        state.baseline_force_write_pending = false;
                        info!(
                            baseline_node_count = baseline.len(),
                            block_number, "Emission baseline persisted for the first time"
                        );
                    }
                }
            }
        } else {
            // Quiet block (decoded actions present but none were convertible):
            // per-block persist is gated on `processed_events > 0`, so without
            // this retry a fully quiet stream that hit a transient DB failure
            // during the startup force-write would never recover. The helper
            // uses the CURRENT block's cursor/number to match the in-memory
            // snapshot it persists.
            self.retry_force_write_baseline(block_number, &meta.cursor)
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

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring crypto provider");

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
                           // `0` is the Substreams protocol's sentinel for "never stop". Passing
                           // `u64::MAX` instead makes the server plan a bounded job of ~18 quintillion
                           // blocks: it backprocesses forever, emitting progress but never a single
                           // `BlockScopedData`.
    let end_block: u64 = env::var("SUBSTREAMS_END_BLOCK")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Root space configuration
    let root_space_id = parse_root_space_id()?;

    // Validate checkpoint environment invariants before external connections.
    // This catches misconfiguration (e.g. missing ATLAS_INDEXER_ID when checkpoint
    // persistence is enabled) before we connect to Kafka.
    let _checkpoint_preflight = CheckpointConfig::from_env(root_space_id, 1)?;

    // Serves /metrics so Prometheus can see how far behind the chain tip we are.
    // Without this atlas was invisible to monitoring: it wedged for two days and
    // no alert could have fired, because nothing published its block height.
    let metrics_port: Option<u16> = env::var("METRICS_PORT").ok().and_then(|s| s.parse().ok());
    hermes_instrumentation::metrics::install("atlas", metrics_port)?;

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

    // Armed before the stream starts so a substream that never delivers its
    // first block is caught too, not just one that dies mid-flight. Progress is
    // seeded from the restored checkpoint, so an idle chain after a restart
    // reads as caught-up rather than as the whole chain height of lag.
    match stall::config_from_env() {
        Some(config) if !use_mock => stall::spawn(sink.progress.clone(), config),
        Some(_) => info!("Mock source in use; substream stall detection not armed"),
        None => {}
    }

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
