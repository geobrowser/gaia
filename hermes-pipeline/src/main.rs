//! Hermes Pipeline
//!
//! Consumes space-related events from hermes-substream via hermes-relay and
//! transforms them into Hermes protobuf messages for publication to Kafka.
//!
//! ## Event Types Handled
//!
//! - `SPACE_REGISTERED` - new space registrations -> `space.creations` topic
//! - `SUBSPACE_VERIFIED/RELATED/TOPIC_DECLARED/REMOVED` - trust events -> `space.trust.extensions` topic
//! - `EDITOR/MEMBER_ADDED/REMOVED`, `SPACE_LEFT` - membership -> `space.membership` topic
//! - `EDITOR_FLAGGED/UNFLAGGED`, `FLAGGED/UNFLAGGED` - moderation -> `space.moderation` topic
//! - `TOPIC_DECLARED` - topic declarations -> `space.topics` topic
//! - `PROPOSAL_CREATED/VOTED/EXECUTED` - governance -> `space.governance` topic
//! - `UPVOTED/DOWNVOTED/UNVOTED` - curation voting -> `curation.votes` topic
//! - `EDITS_PUBLISHED` - edit publications -> `knowledge.edits` topic
//!
//! ## Architecture
//!
//! The pipeline processes blocks in two phases:
//! 1. **Transform**: All pipelines run concurrently. Edits pipeline (async with IPFS fetch)
//!    is kicked off first so network I/O happens in parallel with sync transforms.
//! 2. **Emit**: Send events to Kafka in order (spaces, membership, trust, moderation,
//!    topics, governance, voting, edits)
//!
//! ## Configuration
//!
//! Environment variables:
//! - `USE_MOCK` - Set to "true" or "1" to use mock data (default: false)
//! - `SUBSTREAMS_ENDPOINT` - Substreams endpoint URL (default: geotest.substreams.pinax.network:443)
//! - `SUBSTREAMS_API_TOKEN` - API token for substreams authentication
//! - `SUBSTREAMS_START_BLOCK` - First block to consume (default: 88109)
//! - `SUBSTREAMS_END_BLOCK` - Last block to consume (default: u64::MAX for continuous)
//! - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
//! - `KAFKA_USERNAME` - SASL username for managed Kafka (optional)
//! - `KAFKA_PASSWORD` - SASL password for managed Kafka (optional)
//! - `KAFKA_SSL_CA_PEM` - Custom CA cert for SSL (optional)
//! - `DATABASE_URL` - PostgreSQL URL for IPFS cache (required when USE_MOCK=false)

mod emit;

use std::env;
use std::fmt;
use std::sync::Arc;

use hermes_instrumentation::{Instrument, debug, info, info_span, warn};
use prost::Message;

use hermes_kafka::create_producer;
use hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData;
use hermes_relay::stream::utils;
use hermes_relay::{Actions, HermesModule, Sink, StreamSource};

use hermes_pipeline::cache::{CacheSource, IpfsCache};
use hermes_pipeline::pipelines;
use hermes_pipeline::pipelines::BlockMetadata;
use hermes_pipeline::pipelines::edits::RetryConfig;
use hermes_pipeline::pipelines::trust::get_extension_type;
use hermes_pipeline::pipelines::voting::get_vote_direction;

use emit::{Emitter, topics};

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
/// - `SUBSPACE_VERIFIED/RELATED/TOPIC_DECLARED/REMOVED` -> trust pipeline
/// - `EDITOR/MEMBER_ADDED/REMOVED`, `SPACE_LEFT` -> membership pipeline
/// - `EDITOR_FLAGGED/UNFLAGGED`, `FLAGGED/UNFLAGGED` -> moderation pipeline
/// - `TOPIC_DECLARED` -> topics pipeline
/// - `PROPOSAL_CREATED/VOTED/EXECUTED` -> governance pipeline
/// - `UPVOTED/DOWNVOTED/UNVOTED` -> voting pipeline
/// - `EDITS_PUBLISHED` -> edits pipeline (with IPFS cache lookup)
///
/// The edits pipeline is kicked off first (async) so IPFS fetching happens
/// in parallel with the sync transforms. All events are emitted to Kafka in order.
pub struct Pipeline {
    emitter: Emitter,
    cache: Arc<dyn IpfsCache>,
    retry_config: RetryConfig,
}

impl Pipeline {
    pub fn new(emitter: Emitter, cache: Arc<dyn IpfsCache>) -> Self {
        Self {
            emitter,
            cache,
            retry_config: RetryConfig::default(),
        }
    }

    /// Create a pipeline with custom retry configuration.
    #[allow(dead_code)]
    pub fn with_retry_config(
        emitter: Emitter,
        cache: Arc<dyn IpfsCache>,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            emitter,
            cache,
            retry_config,
        }
    }

    /// Flush all pending messages to Kafka.
    pub fn flush(&self, timeout: std::time::Duration) {
        self.emitter.flush(timeout);
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

        // Kick off edits transform FIRST - it has async IPFS fetching that can
        // happen in parallel with the sync transforms below
        let edits_future =
            pipelines::edits::transform(actions, &meta, &self.cache, &self.retry_config)
                .instrument(info_span!("transform.edits", action_count = actions.len()));

        // Sync transforms - fast, no I/O, run while edits fetches from IPFS
        let mut spaces = info_span!("transform.spaces", action_count = actions.len())
            .in_scope(|| pipelines::spaces::transform(actions, &meta))?;

        let mut membership = info_span!("transform.membership", action_count = actions.len())
            .in_scope(|| pipelines::membership::transform(actions, &meta))?;

        let mut trust = info_span!("transform.trust", action_count = actions.len())
            .in_scope(|| pipelines::trust::transform(actions, &meta))?;

        let mut moderation = info_span!("transform.moderation", action_count = actions.len())
            .in_scope(|| pipelines::moderation::transform(actions, &meta))?;

        let mut topics = info_span!("transform.topics", action_count = actions.len())
            .in_scope(|| pipelines::topics::transform(actions, &meta))?;

        let mut governance = info_span!("transform.governance", action_count = actions.len())
            .in_scope(|| pipelines::governance::transform(actions, &meta))?;

        let mut voting = info_span!("transform.voting", action_count = actions.len())
            .in_scope(|| pipelines::voting::transform(actions, &meta))?;

        // Now await the edits - IPFS fetch should have been happening in parallel
        let mut edits = edits_future.await?;

        // =========================================================================
        // Phase 1.5: Mark the last event in the block
        // =========================================================================
        // Find max sequence across all events and mark that event with is_last = true.
        // This allows consumers to know when they've received all events for a block.
        {
            use pipelines::{mark_sequence_as_last, max_sequence};

            let max_seq = [
                max_sequence(&spaces.events),
                max_sequence(&membership.roles_granted),
                max_sequence(&membership.roles_revoked),
                max_sequence(&membership.spaces_left),
                max_sequence(&trust.events),
                max_sequence(&moderation.editors_flagged),
                max_sequence(&moderation.editors_unflagged),
                max_sequence(&moderation.content_flagged),
                max_sequence(&moderation.content_unflagged),
                max_sequence(&topics.topics_declared),
                max_sequence(&governance.proposals_created),
                max_sequence(&governance.proposals_voted),
                max_sequence(&governance.proposals_executed),
                max_sequence(&voting.votes),
                max_sequence(&edits.events),
            ]
            .into_iter()
            .max()
            .unwrap_or(0);

            // Try to mark the event with max_seq as last (only one will match)
            let _ = mark_sequence_as_last(&mut spaces.events, max_seq)
                || mark_sequence_as_last(&mut membership.roles_granted, max_seq)
                || mark_sequence_as_last(&mut membership.roles_revoked, max_seq)
                || mark_sequence_as_last(&mut membership.spaces_left, max_seq)
                || mark_sequence_as_last(&mut trust.events, max_seq)
                || mark_sequence_as_last(&mut moderation.editors_flagged, max_seq)
                || mark_sequence_as_last(&mut moderation.editors_unflagged, max_seq)
                || mark_sequence_as_last(&mut moderation.content_flagged, max_seq)
                || mark_sequence_as_last(&mut moderation.content_unflagged, max_seq)
                || mark_sequence_as_last(&mut topics.topics_declared, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_created, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_voted, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_executed, max_seq)
                || mark_sequence_as_last(&mut voting.votes, max_seq)
                || mark_sequence_as_last(&mut edits.events, max_seq);
        }

        // =========================================================================
        // Phase 2: Emit events to Kafka in order
        // =========================================================================
        // Ordering matters here:
        // 1. Spaces must be emitted first since all other events reference spaces
        // 2. Membership events next (who can do what in spaces)
        // 3. Trust events define the space topology
        // 4. Moderation events (flagging)
        // 5. Topic declarations
        // 6. Governance events (proposals reference spaces)
        // 7. Voting events (social layer)
        // 8. Edits come last as they may reference entities across trusted spaces

        {
            let _emit_span = info_span!("emit").entered();

            // 1. Emit spaces
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

            // 2. Emit membership events
            {
                let _span = info_span!("emit.membership", count = membership.total()).entered();
                for event in &membership.roles_granted {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        account = %hex::encode(&event.account),
                        "Role granted"
                    );
                }
                for event in &membership.roles_revoked {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        account = %hex::encode(&event.account),
                        "Role revoked"
                    );
                }
                for event in &membership.spaces_left {
                    self.emitter.emit(event)?;
                    debug!(
                        member_id = %hex::encode(&event.member_id),
                        space_id = %hex::encode(&event.space_id),
                        "Space left"
                    );
                }
            }

            // 3. Emit trust events
            {
                let _span = info_span!("emit.trust", count = trust.total()).entered();
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

            // 4. Emit moderation events
            {
                let _span = info_span!("emit.moderation", count = moderation.total()).entered();
                for event in &moderation.editors_flagged {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        editor = %hex::encode(&event.editor_account),
                        "Editor flagged"
                    );
                }
                for event in &moderation.editors_unflagged {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        editor = %hex::encode(&event.editor_account),
                        "Editor unflagged"
                    );
                }
                for event in &moderation.content_flagged {
                    self.emitter.emit(event)?;
                    debug!(
                        flagger_id = %hex::encode(&event.flagger_id),
                        target_space_id = %hex::encode(&event.target_space_id),
                        "Content flagged"
                    );
                }
                for event in &moderation.content_unflagged {
                    self.emitter.emit(event)?;
                    debug!(
                        unflagger_id = %hex::encode(&event.unflagger_id),
                        target_space_id = %hex::encode(&event.target_space_id),
                        "Content unflagged"
                    );
                }
            }

            // 5. Emit topic declarations
            {
                let _span = info_span!("emit.topics", count = topics.total()).entered();
                for event in &topics.topics_declared {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        topic_id = %hex::encode(&event.topic_id),
                        "Topic declared"
                    );
                }
            }

            // 6. Emit governance events
            {
                let _span = info_span!("emit.governance", count = governance.total()).entered();
                for event in &governance.proposals_created {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal created"
                    );
                }
                for event in &governance.proposals_voted {
                    self.emitter.emit(event)?;
                    debug!(
                        voter_id = %hex::encode(&event.voter_id),
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal voted"
                    );
                }
                for event in &governance.proposals_executed {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal executed"
                    );
                }
            }

            // 7. Emit voting events
            {
                let _span = info_span!("emit.voting", count = voting.total()).entered();
                for event in &voting.votes {
                    self.emitter.emit(event)?;
                    debug!(
                        voter_id = %hex::encode(&event.voter_id),
                        object_id = %hex::encode(&event.object_id),
                        direction = get_vote_direction(event),
                        "Vote cast"
                    );
                }
            }

            // 8. Emit edits
            {
                let _span = info_span!("emit.edits", count = edits.events.len()).entered();
                for event in &edits.events {
                    self.emitter.emit(event)?;
                    let space_id_display = if event.space_id.len() == 16 {
                        uuid::Uuid::from_bytes(
                            event.space_id.as_slice().try_into().unwrap_or([0; 16]),
                        )
                        .to_string()
                    } else {
                        hex::encode(&event.space_id)
                    };
                    debug!(
                        name = %event.name,
                        space_id = %space_id_display,
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
        let membership_count = membership.total() as u64;
        let trust_count = trust.total();
        let moderation_count = moderation.total() as u64;
        let topics_count = topics.total() as u64;
        let governance_count = governance.total() as u64;
        let voting_count = voting.total() as u64;
        let edit_count = edits.events.len() as u64;
        let total = space_count
            + membership_count
            + trust_count
            + moderation_count
            + topics_count
            + governance_count
            + voting_count
            + edit_count;

        if total > 0 || edits.cache_misses > 0 || edits.errored_entries > 0 {
            info!(
                spaces = space_count,
                membership = membership_count,
                trust_verified = trust.verified,
                trust_related = trust.related,
                trust_topic = trust.topic_declared,
                trust_removed = trust.removed,
                moderation = moderation_count,
                topics = topics_count,
                governance = governance_count,
                voting_up = voting.upvotes,
                voting_down = voting.downvotes,
                voting_unvote = voting.unvotes,
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

    // Determine if we're using mock data
    let use_mock = env::var("USE_MOCK")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    info!(kafka_broker = %broker, use_mock = use_mock, "Configuration loaded");

    // Create Kafka producer and wrap in Emitter
    debug!("Connecting to Kafka broker");
    let producer = create_producer(&broker, "hermes-pipeline")?;
    let emitter = Emitter::new(producer);
    info!("Connected to Kafka broker");

    // Create the IPFS cache: mock for testing, PostgreSQL for production
    let cache = if use_mock {
        info!("Using mock IPFS cache");
        CacheSource::mock().into_cache().await?
    } else {
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL must be set when USE_MOCK is not true");
        info!("Connecting to IPFS cache database");
        CacheSource::live(&database_url).into_cache().await?
    };
    info!("IPFS cache initialized");

    // Create the pipeline
    let pipeline = Pipeline::new(emitter, cache);

    // Determine stream source: mock or live substreams
    let source = if use_mock {
        StreamSource::mock()
    } else {
        let endpoint = env::var("SUBSTREAMS_ENDPOINT")
            .unwrap_or_else(|_| "geotest.substreams.pinax.network:443".to_string());
        let start_block: i64 = env::var("SUBSTREAMS_START_BLOCK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(81809);
        let end_block: u64 = env::var("SUBSTREAMS_END_BLOCK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(u64::MAX);

        info!(
            endpoint = %endpoint,
            start_block = start_block,
            end_block = end_block,
            "Using live substreams source"
        );

        StreamSource::live(endpoint, HermesModule::Actions, start_block, end_block)
    };

    info!(
        module = %HermesModule::Actions,
        topics.spaces = topics::SPACE_CREATIONS,
        topics.membership = topics::MEMBERSHIP,
        topics.trust = topics::TRUST_EXTENSIONS,
        topics.moderation = topics::MODERATION,
        topics.topics = topics::TOPICS,
        topics.governance = topics::GOVERNANCE,
        topics.voting = topics::VOTING,
        topics.edits = topics::EDITS,
        retry_initial_ms = pipeline.retry_config.initial_delay_ms,
        retry_factor = pipeline.retry_config.factor,
        retry_max_secs = pipeline.retry_config.max_delay.as_secs(),
        retry_max_count = pipeline.retry_config.max_retries,
        "Starting pipeline"
    );

    // Run the pipeline
    pipeline.run(source).await?;

    // Flush all pending messages to Kafka before exiting
    info!("Flushing Kafka producer");
    pipeline.flush(std::time::Duration::from_secs(30));

    info!("Pipeline finished");

    Ok(())
}
