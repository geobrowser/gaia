use std::collections::HashMap;
use std::env;
use std::time::{Duration, Instant};

use futures::StreamExt;
use hermes_instrumentation::{debug, error, info, info_span, warn, Instrument};
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::space::hermes_space_trust_extension::Extension as TrustExtensionType;
use rdkafka::message::Headers;
use rdkafka::Message;
use std::sync::OnceLock;
use tracing::field::display;

mod consumer;
mod error;
mod handlers;
mod models;
mod storage;

use consumer::{get_event_type, parse_message, KafkaConsumer, KgMessage};
use error::IndexerError;
use storage::Storage;

/// Spaces whose edits should be dropped by the indexer.
/// These spaces produced corrupt or unwanted data that was manually cleaned from the database.
const BLOCKED_SPACES: &[uuid::Uuid] = &[
    uuid::uuid!("d24e4d32-3f4e-b6cc-4eaa-757cdd653857"),
    uuid::uuid!("2df9f305-6ccc-2875-e610-2ed299883371"),
    uuid::uuid!("655d6077-dc49-e1f9-0e85-74dd57c3164e"),
];

/// A buffered event with its Kafka metadata for later commit.
struct BufferedEvent {
    msg: KgMessage,
    topic: String,
    partition: i32,
    offset: i64,
    event_type: Option<String>,
    event_id: Option<String>,
}

/// Buffer for events by block number.
struct BlockBuffer {
    /// Events grouped by block number.
    events: HashMap<u64, Vec<BufferedEvent>>,
    /// When each block was first observed.
    first_seen: HashMap<u64, Instant>,
    /// Block summaries keyed by block number.
    summaries: HashMap<u64, BlockSummaryInfo>,
    /// Timeout before force-processing an incomplete block.
    stale_timeout: Duration,
}

impl BlockBuffer {
    fn new(stale_timeout: Duration) -> Self {
        Self {
            events: HashMap::new(),
            first_seen: HashMap::new(),
            summaries: HashMap::new(),
            stale_timeout,
        }
    }

    /// Add an event to the buffer.
    fn push(&mut self, block_number: u64, event: BufferedEvent) {
        self.first_seen
            .entry(block_number)
            .or_insert_with(Instant::now);
        self.events.entry(block_number).or_default().push(event);
    }

    fn insert_summary(
        &mut self,
        block_number: u64,
        summary: hermes_schema::pb::block_summary::HermesBlockSummary,
        expected_count: usize,
    ) {
        self.summaries.insert(
            block_number,
            BlockSummaryInfo {
                summary,
                expected_count,
                received_at: Instant::now(),
            },
        );
    }

    fn take_summary(&mut self, block_number: u64) -> Option<BlockSummaryInfo> {
        self.summaries.remove(&block_number)
    }

    fn buffered_count(&self, block_number: u64) -> usize {
        self.events.get(&block_number).map(|e| e.len()).unwrap_or(0)
    }

    /// Remove and return all events for a block, sorted by sequence.
    fn take_block(&mut self, block_number: u64) -> Vec<BufferedEvent> {
        self.first_seen.remove(&block_number);
        let mut events = self.events.remove(&block_number).unwrap_or_default();
        events.sort_by_key(|e| e.msg.sequence());
        events
    }

    /// Lowest block number still buffered (has events and/or a summary).
    /// Used to enforce strict in-order flushing across blocks.
    fn min_pending_block(&self) -> Option<u64> {
        self.first_seen
            .keys()
            .chain(self.summaries.keys())
            .min()
            .copied()
    }

    /// A block is complete once its summary has arrived and every expected
    /// event for the indexed topics has been buffered.
    fn is_complete(&self, block_number: u64) -> bool {
        match self.summaries.get(&block_number) {
            Some(info) => self.buffered_count(block_number) >= info.expected_count,
            None => false,
        }
    }

    /// A block is stale once it (or its summary) has been buffered longer than
    /// the stale timeout, the fallback that guarantees forward progress.
    fn is_stale(&self, block_number: u64) -> bool {
        let now = Instant::now();
        let events_stale = self
            .first_seen
            .get(&block_number)
            .map(|first_seen| now.duration_since(*first_seen) > self.stale_timeout)
            .unwrap_or(false);
        let summary_stale = self
            .summaries
            .get(&block_number)
            .map(|summary| now.duration_since(summary.received_at) > self.stale_timeout)
            .unwrap_or(false);
        events_stale || summary_stale
    }
}

#[derive(Clone)]
struct BlockSummaryInfo {
    summary: hermes_schema::pb::block_summary::HermesBlockSummary,
    expected_count: usize,
    received_at: Instant,
}

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

    Config::new("kg-indexer", backend)
}

fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    let _telemetry = hermes_instrumentation::init(build_telemetry_config())
        .map_err(|e| IndexerError::config(format!("telemetry init failed: {}", e)))?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| IndexerError::config(format!("failed to build tokio runtime: {}", e)))?
        .block_on(async_main())
}

async fn async_main() -> Result<(), IndexerError> {
    info!("Starting kg-indexer");

    // Load configuration from environment
    let database_url =
        env::var("DATABASE_URL").map_err(|_| IndexerError::config("DATABASE_URL not set"))?;
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let kafka_group_id = env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "kg-indexer".to_string());
    let stale_timeout_ms: u64 = env::var("BLOCK_STALE_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            env::var("BLOCK_STALE_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(|secs| secs * 1000)
        })
        .unwrap_or(1000);

    // Initialize storage
    let storage = Storage::new(&database_url).await?;
    info!("Connected to database");

    // Initialize Kafka consumer
    let consumer = KafkaConsumer::new(&kafka_broker, &kafka_group_id)?;
    consumer.subscribe()?;

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
    let mut tally_shutdown_rx = shutdown_tx.subscribe();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Spawn background worker to process proposal vote tally updates
    // This decouples the vote write path from tally computation for better performance
    let tally_storage = storage.clone();
    let tally_interval_ms: u64 = env::var("TALLY_WORKER_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000); // Default: 1 second
    let tally_batch_size: i64 = env::var("TALLY_WORKER_BATCH_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100); // Default: 100 proposals per batch

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(tally_interval_ms));
        // Skip missed ticks to avoid thundering herd after pause/delay
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        info!(
            interval_ms = tally_interval_ms,
            batch_size = tally_batch_size,
            "Starting proposal tally worker"
        );

        // Log queue depth every N ticks (roughly every minute at default 5s interval)
        let mut tick_count: u64 = 0;
        let depth_log_interval: u64 = 12; // Log depth every ~60 seconds

        loop {
            tokio::select! {
                _ = tally_shutdown_rx.recv() => {
                    info!("Tally worker shutting down");
                    break;
                }
                _ = interval.tick() => {
                    tick_count += 1;

                    match tally_storage.process_tally_queue(tally_batch_size).await {
                        Ok(0) => {
                            // No proposals to process, nothing to log
                        }
                        Ok(count) => {
                            debug!(
                                count = count,
                                "Processed proposal tally updates"
                            );
                        }
                        Err(e) => {
                            error!(
                                error = %e,
                                "Failed to process proposal tally queue"
                            );
                        }
                    }

                    // Periodically log queue depth for monitoring
                    if tick_count.is_multiple_of(depth_log_interval) {
                        match tally_storage.get_tally_queue_depth().await {
                            Ok(depth) => {
                                if depth > 0 {
                                    info!(
                                        queue_depth = depth,
                                        "Proposal tally queue depth"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    "Failed to get tally queue depth"
                                );
                            }
                        }
                    }
                }
            }
        }
    });

    // Main processing loop
    //
    // Events are buffered by block number and normally flushed only once we have the
    // block summary and the expected number of events for the topics kg-indexer consumes.
    // Timeout is the fallback when the summary never arrives or delivery stays incomplete.
    //
    // To handle these cases, we use `tokio::select!` with a periodic tick that checks
    // for stale blocks (buffered longer than `stale_timeout`). The tick runs independently
    // of the Kafka stream, so even if no messages arrive, timed out blocks get processed.
    let mut stream = consumer.stream();
    let stale_timeout = Duration::from_millis(stale_timeout_ms);
    let mut buffer = BlockBuffer::new(stale_timeout);
    let mut processed_count: u64 = 0;
    let mut error_count: u64 = 0;
    let mut blocks_processed: u64 = 0;
    let mut stale_check_interval = tokio::time::interval(Duration::from_millis(100));

    info!(
        stale_timeout_ms = stale_timeout_ms,
        "Starting message processing loop"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down...");
                break;
            }

            _ = stale_check_interval.tick() => {
                let (processed, errors, blocks) =
                    drain_ready_blocks(&mut buffer, &storage, &consumer).await;
                processed_count += processed;
                error_count += errors;
                blocks_processed += blocks;
            }

            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();
                        let event_type = get_event_type(msg.headers());
                        let event_id_header = get_header_value(msg.headers(), "event-id");

                        // Parse message first to determine if we should create a span
                        let payload = match msg.payload() {
                            Some(p) => p,
                            None => continue,
                        };

                        let kg_msg = match parse_message(&topic, payload, event_type.as_deref()) {
                            Ok(msg) => msg,
                            Err(e) => {
                                warn!(
                                    topic = %topic,
                                    partition = partition,
                                    offset = offset,
                                    event_id = event_id_header.as_deref().unwrap_or(""),
                                    error = %e,
                                    "Failed to parse message"
                                );
                                error_count += 1;
                                // Still commit to avoid getting stuck
                                if let Err(e) =
                                    consumer.commit_message(&topic, partition, offset)
                                {
                                    error!(
                                        event_id = event_id_header.as_deref().unwrap_or(""),
                                        error = %e,
                                        "Failed to commit offset"
                                    );
                                }
                                continue;
                            }
                        };

                        // Skip edits from blocked spaces
                        if let KgMessage::Edit(ref edit) = kg_msg {
                            if let Ok(space_id) = uuid::Uuid::from_slice(&edit.space_id) {
                                if BLOCKED_SPACES.contains(&space_id) {
                                    warn!(
                                        space_id = %space_id,
                                        "Skipping edit from blocked space"
                                    );
                                    continue;
                                }
                            }
                        }

                        // Skip empty blocks entirely - no span created
                        if let KgMessage::BlockSummary(ref summary) = kg_msg {
                            let expected_count = expected_count_for_indexer(summary);
                            if expected_count == 0 {
                                continue;
                            }
                        }

                        // Now create span only for meaningful work
                        let span = info_span!(
                            "kg_indexer.poll",
                            topic = %topic,
                            partition = partition,
                            offset = offset,
                            event_type = event_type.as_deref().unwrap_or(""),
                            event_id = tracing::field::Empty,
                            block_number = tracing::field::Empty,
                            is_last = tracing::field::Empty,
                            "otel.status_code" = tracing::field::Empty,
                            "otel.status_message" = tracing::field::Empty
                        );

                        let fut = async {
                            if let KgMessage::BlockSummary(summary) = kg_msg {
                                let expected_count = expected_count_for_indexer(&summary);
                                let summary_block_number = summary.block_number;

                                info!(
                                    event = "kg_indexer.block_summary_received",
                                    block_number = summary_block_number,
                                    expected_count = expected_count,
                                    total_events = summary.total_events,
                                    "Block summary received"
                                );
                                if log_event_ids_enabled() {
                                    info!(
                                        event = "kg_indexer.event_id",
                                        topic = %topic,
                                        event_id = event_id_header.as_deref().unwrap_or(""),
                                        event_type = "BLOCK_SUMMARY",
                                        block_number = summary_block_number,
                                        "Received event"
                                    );
                                }

                                buffer.insert_summary(summary_block_number, summary, expected_count);

                                let (processed, errors, blocks) =
                                    drain_ready_blocks(&mut buffer, &storage, &consumer).await;
                                processed_count += processed;
                                error_count += errors;
                                blocks_processed += blocks;

                                return;
                            }

                            // Non-BLOCK_SUMMARY message handling
                            let event_id = event_id_header.or_else(|| {
                                kg_msg
                                    .meta()
                                    .map(|meta| event_id_from_meta(meta, &topic))
                            });
                            if let Some(ref event_id) = event_id {
                                tracing::Span::current().record("event_id", event_id.as_str());
                            }

                            if log_event_ids_enabled() {
                                info!(
                                    event = "kg_indexer.event_id",
                                    topic = %topic,
                                    event_id = event_id.as_deref().unwrap_or(""),
                                    event_type = event_type.as_deref().unwrap_or(""),
                                    block_number = kg_msg.block_number().unwrap_or(0),
                                    "Received event"
                                );
                            }

                            // Get block number from metadata
                            let block_number = match kg_msg.block_number() {
                                Some(bn) => bn,
                                None => {
                                    // Fall back to immediate processing if no metadata
                                    warn!(
                                        topic = %topic,
                                        "Message has no block metadata, processing immediately"
                                    );
                                    match process_message(
                                        kg_msg,
                                        &storage,
                                        event_id.as_deref(),
                                    )
                                    .await
                                    {
                                        Ok(_) => {
                                            processed_count += 1;
                                        }
                                        Err(e) => {
                                            tracing::Span::current()
                                                .record("otel.status_code", "ERROR");
                                            tracing::Span::current()
                                                .record("otel.status_message", e.to_string().as_str());
                                            error!(
                                                event_id = event_id.as_deref().unwrap_or(""),
                                                error = %e,
                                                "Failed to process message"
                                            );
                                            error_count += 1;
                                        }
                                    }
                                    if let Err(e) =
                                        consumer.commit_message(&topic, partition, offset)
                                    {
                                        error!(
                                            event_id = event_id.as_deref().unwrap_or(""),
                                            error = %e,
                                            "Failed to commit offset"
                                        );
                                    }
                                    return;
                                }
                            };

                            let is_last = kg_msg.is_last();
                            tracing::Span::current().record("block_number", block_number);
                            tracing::Span::current().record("is_last", is_last);

                            // Buffer the message
                            buffer.push(
                                block_number,
                                BufferedEvent {
                                    msg: kg_msg,
                                    topic,
                                    partition,
                                    offset,
                                    event_type: event_type.clone(),
                                    event_id: event_id.clone(),
                                },
                            );

                            let (processed, errors, blocks) =
                                drain_ready_blocks(&mut buffer, &storage, &consumer).await;
                            processed_count += processed;
                            error_count += errors;
                            blocks_processed += blocks;

                            // `is_last` is assigned by the producer, but Kafka can still deliver
                            // that event before lower-sequence messages from other topics in the
                            // same block. Treat it as a hint only; summary completion or idle
                            // timeout are the safe completion signals.
                            if is_last {
                                debug!(
                                    block_number = block_number,
                                    buffered_event_count = buffer.buffered_count(block_number),
                                    "Received is_last marker; waiting for block summary or stale timeout"
                                );
                            }
                        };
                        fut.instrument(span).await;
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "Kafka error");
                    }
                    None => {
                        info!("Stream ended");
                        break;
                    }
                }
            }
        }
    }

    info!(
        processed = processed_count,
        errors = error_count,
        "Shutdown complete"
    );

    Ok(())
}

fn event_id_from_meta(meta: &BlockchainMetadata, topic: &str) -> String {
    format!(
        "{}:{}:{}:{}",
        topic, meta.block_number, meta.sequence, meta.cursor
    )
}

fn get_header_value(
    headers: Option<&rdkafka::message::BorrowedHeaders>,
    key: &str,
) -> Option<String> {
    headers.and_then(|h| {
        for header in h.iter() {
            if header.key.eq_ignore_ascii_case(key) {
                if let Some(value) = header.value {
                    if let Ok(value_str) = std::str::from_utf8(value) {
                        return Some(value_str.to_string());
                    }
                }
            }
        }
        None
    })
}

fn log_event_ids_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("LOG_EVENT_IDS")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    })
}

fn sentry_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("SENTRY_DSN").ok().is_some())
}

const INDEXER_TOPICS: &[&str] = &[
    "knowledge.edits",
    "space.creations",
    "space.membership",
    "space.trust.extensions",
    "space.topics",
    "space.governance",
];

const EXPECTED_EVENT_TYPES: &[&str] = &[
    "EDITS_PUBLISHED",
    "SPACE_REGISTERED",
    "ROLE_GRANTED",
    "ROLE_REVOKED",
    "TRUST_EXTENSION",
    "TOPIC_DECLARED",
    "TOPIC_REMOVED",
    "PROPOSAL_CREATED",
    "PROPOSAL_UPDATED",
    "PROPOSAL_VOTED",
    "PROPOSAL_EXECUTED",
    "PROPOSAL_SETTINGS_UPDATED",
    "VOTING_SETTINGS_UPDATED",
];

fn expected_count_for_indexer(
    summary: &hermes_schema::pb::block_summary::HermesBlockSummary,
) -> usize {
    INDEXER_TOPICS
        .iter()
        .map(|topic| summary.counts_by_topic.get(*topic).copied().unwrap_or(0))
        .sum::<u64>() as usize
}

enum BlockProcessReason {
    Summary,
    Stale,
}

impl BlockProcessReason {
    fn as_str(&self) -> &'static str {
        match self {
            BlockProcessReason::Summary => "summary",
            BlockProcessReason::Stale => "stale",
        }
    }
}

fn event_type_label(event: &BufferedEvent) -> String {
    if let Some(ref event_type) = event.event_type {
        return event_type.clone();
    }

    match &event.msg {
        KgMessage::Edit(_) => "EDITS_PUBLISHED".to_string(),
        KgMessage::CreateSpace(_) => "SPACE_REGISTERED".to_string(),
        KgMessage::RoleGranted(_) => "ROLE_GRANTED".to_string(),
        KgMessage::RoleRevoked(_) => "ROLE_REVOKED".to_string(),
        KgMessage::TrustExtension(_) => "TRUST_EXTENSION".to_string(),
        KgMessage::TopicDeclared(_) => "TOPIC_DECLARED".to_string(),
        KgMessage::TopicRemoved(_) => "TOPIC_REMOVED".to_string(),
        KgMessage::ProposalCreated(_) => "PROPOSAL_CREATED".to_string(),
        KgMessage::ProposalUpdated(_) => "PROPOSAL_UPDATED".to_string(),
        KgMessage::ProposalVoted(_) => "PROPOSAL_VOTED".to_string(),
        KgMessage::ProposalExecuted(_) => "PROPOSAL_EXECUTED".to_string(),
        KgMessage::ProposalSettingsUpdated(_) => "PROPOSAL_SETTINGS_UPDATED".to_string(),
        KgMessage::VotingSettingsUpdated(_) => "VOTING_SETTINGS_UPDATED".to_string(),
        KgMessage::BlockSummary(_) => "BLOCK_SUMMARY".to_string(),
    }
}

fn blockchain_metadata_to_strings(
    meta: Option<&hermes_schema::pb::blockchain_metadata::BlockchainMetadata>,
) -> (String, String) {
    meta.map_or_else(
        || ("0".to_string(), "0".to_string()),
        |m| (m.created_at.to_string(), m.block_number.to_string()),
    )
}

fn make_topic_entity(
    topic_id: uuid::Uuid,
    meta: Option<&hermes_schema::pb::blockchain_metadata::BlockchainMetadata>,
) -> models::entities::EntityItem {
    let (created_at, created_at_block) = blockchain_metadata_to_strings(meta);

    models::entities::EntityItem {
        id: topic_id,
        created_at: created_at.clone(),
        created_at_block: created_at_block.clone(),
        updated_at: created_at,
        updated_at_block: created_at_block,
    }
}

fn apply_pending_space_topic(
    space: &mut models::spaces::SpaceItem,
    pending_space_topics: &HashMap<uuid::Uuid, Option<uuid::Uuid>>,
) {
    if let Some(topic_id) = pending_space_topics.get(&space.id).copied() {
        space.topic_id = topic_id;
    }
}

/// Maximum length for edit names stored in the database.
const MAX_EDIT_NAME_LENGTH: usize = 256;

/// Extract edit metadata (name and creator ID) from a protobuf Edit message.
///
/// - Converts empty name to None (protobuf defaults to "")
/// - Truncates name to MAX_EDIT_NAME_LENGTH at a char boundary
/// - Parses first author as a UUID (16-byte author entries)
fn extract_edit_metadata(
    edit: &hermes_schema::pb::knowledge::HermesEdit,
) -> (Option<String>, Option<uuid::Uuid>) {
    let name = if edit.name.is_empty() {
        None
    } else if edit.name.len() > MAX_EDIT_NAME_LENGTH {
        // Truncate at a char boundary to avoid splitting multi-byte characters
        let truncated = &edit.name[..edit.name.floor_char_boundary(MAX_EDIT_NAME_LENGTH)];
        Some(truncated.to_string())
    } else {
        Some(edit.name.clone())
    };

    let created_by_id = edit
        .authors
        .first()
        .and_then(|a| uuid::Uuid::from_slice(a).ok());

    (name, created_by_id)
}

/// Flush buffered blocks in strict ascending order, lowest first, while ready.
/// Returns the `(processed, errors, blocks_processed)` deltas for the caller.
async fn drain_ready_blocks(
    buffer: &mut BlockBuffer,
    storage: &Storage,
    consumer: &KafkaConsumer,
) -> (u64, u64, u64) {
    let mut processed_count = 0;
    let mut error_count = 0;
    let mut blocks_processed = 0;

    while let Some(block_number) = buffer.min_pending_block() {
        let reason = if buffer.is_complete(block_number) {
            BlockProcessReason::Summary
        } else if buffer.is_stale(block_number) {
            BlockProcessReason::Stale
        } else {
            break;
        };

        let is_stale = matches!(reason, BlockProcessReason::Stale);
        let summary = buffer.take_summary(block_number);
        let events = buffer.take_block(block_number);

        if events.is_empty() {
            continue;
        }

        if is_stale {
            warn!(
                block_number = block_number,
                event_count = events.len(),
                "Force-processing stale block"
            );
        }

        if let Some((processed, errors)) =
            process_buffered_block(events, storage, consumer, summary, reason).await
        {
            processed_count += processed;
            error_count += errors;
            blocks_processed += 1;
        }
    }

    (processed_count, error_count, blocks_processed)
}

async fn process_buffered_block(
    events: Vec<BufferedEvent>,
    storage: &Storage,
    consumer: &KafkaConsumer,
    summary_info: Option<BlockSummaryInfo>,
    reason: BlockProcessReason,
) -> Option<(u64, u64)> {
    if events.is_empty() {
        return None;
    }

    let block_number = events[0].msg.block_number().unwrap_or(0);

    let mut counts_by_event_type: HashMap<String, u64> = HashMap::new();
    let mut counts_by_topic: HashMap<String, u64> = HashMap::new();
    let mut partition_set: Vec<i32> = Vec::new();
    let mut offset_min = i64::MAX;
    let mut offset_max = i64::MIN;

    for event in &events {
        *counts_by_event_type
            .entry(event_type_label(event))
            .or_insert(0) += 1;
        *counts_by_topic.entry(event.topic.clone()).or_insert(0) += 1;
        if !partition_set.contains(&event.partition) {
            partition_set.push(event.partition);
        }
        offset_min = offset_min.min(event.offset);
        offset_max = offset_max.max(event.offset);
    }
    partition_set.sort_unstable();

    let expected_event_count = summary_info
        .as_ref()
        .map(|s| s.expected_count as u64)
        .unwrap_or(0);
    let expected_counts_by_type: HashMap<String, u64> = summary_info
        .as_ref()
        .map(|s| {
            EXPECTED_EVENT_TYPES
                .iter()
                .map(|name| {
                    (
                        name.to_string(),
                        *s.summary.counts_by_event_type.get(*name).unwrap_or(&0),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let missing_event_types: Vec<String> = summary_info
        .as_ref()
        .map(|s| {
            EXPECTED_EVENT_TYPES
                .iter()
                .filter_map(|name| {
                    let expected = s
                        .summary
                        .counts_by_event_type
                        .get(*name)
                        .copied()
                        .unwrap_or(0);
                    let actual = counts_by_event_type.get(*name).copied().unwrap_or(0);
                    if expected > 0 && actual == 0 {
                        Some((*name).to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    info!(
        event = "kg_indexer.batch_start",
        block_number = block_number,
        reason = reason.as_str(),
        buffered_event_count = events.len(),
        expected_event_count = expected_event_count,
        counts_by_event_type = ?counts_by_event_type,
        counts_by_topic = ?counts_by_topic,
        expected_counts_by_type = ?expected_counts_by_type,
        offset_min = offset_min,
        offset_max = offset_max,
        partitions = ?partition_set,
        "Batch start"
    );

    let event_len = events.len();
    let span = info_span!(
        "kg_indexer.process_block",
        block_number = block_number,
        event_count = event_len,
        reason = reason.as_str(),
        "otel.status_code" = tracing::field::Empty,
        "otel.status_message" = tracing::field::Empty
    );
    let start = Instant::now();

    let result = process_block(events, storage, consumer)
        .instrument(span.clone())
        .await;

    match result {
        Ok(result) => {
            let duration_ms = start.elapsed().as_millis();
            if sentry_enabled() {
                info!(
                    event = "kg_indexer.batch_end",
                    block_number = block_number,
                    db_ops_total = result.ops as u64,
                    commit_offsets_failed = result.commit_failures,
                    counts_by_event_type = ?counts_by_event_type,
                    counts_by_topic = ?counts_by_topic,
                    expected_counts_by_type = ?expected_counts_by_type,
                    missing_event_types = ?missing_event_types,
                    "Batch end"
                );
            } else {
                info!(
                    event = "kg_indexer.batch_end",
                    block_number = block_number,
                    duration_ms = duration_ms,
                    db_tx_duration_ms = result.db_tx_duration_ms,
                    db_ops_total = result.ops as u64,
                    commit_offsets_failed = result.commit_failures,
                    counts_by_event_type = ?counts_by_event_type,
                    counts_by_topic = ?counts_by_topic,
                    expected_counts_by_type = ?expected_counts_by_type,
                    missing_event_types = ?missing_event_types,
                    "Batch end"
                );
            }
            Some((event_len as u64, 0))
        }
        Err(e) => {
            span.record("otel.status_code", "ERROR");
            span.record("otel.status_message", e.to_string().as_str());

            let duration_ms = start.elapsed().as_millis();
            if sentry_enabled() {
                error!(
                    event = "kg_indexer.batch_end",
                    block_number = block_number,
                    error = %e,
                    counts_by_event_type = ?counts_by_event_type,
                    counts_by_topic = ?counts_by_topic,
                    expected_counts_by_type = ?expected_counts_by_type,
                    missing_event_types = ?missing_event_types,
                    "Batch failed"
                );
            } else {
                error!(
                    event = "kg_indexer.batch_end",
                    block_number = block_number,
                    duration_ms = duration_ms,
                    error = %e,
                    counts_by_event_type = ?counts_by_event_type,
                    counts_by_topic = ?counts_by_topic,
                    expected_counts_by_type = ?expected_counts_by_type,
                    missing_event_types = ?missing_event_types,
                    "Batch failed"
                );
            }
            Some((0, event_len as u64))
        }
    }
}

/// Process a single Kafka message within its own transaction.
/// Returns the number of database operations performed.
async fn process_message(
    msg: KgMessage,
    storage: &Storage,
    _event_id: Option<&str>,
) -> Result<usize, IndexerError> {
    use handlers::membership::MembershipChange;
    use models::relations::RelationOp;
    use models::values::ValueChangeType;

    let mut tx = storage.pool.begin().await?;
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await?;

    let ops = match msg {
        KgMessage::BlockSummary(_) => 0,
        KgMessage::Edit(edit) => {
            let result = handlers::edits::handle_edit(&edit)?;

            // Keep copies for versioned writes before partitioning
            let values_for_versioning = result.values.clone();
            let relations_for_versioning = result.relations.clone();

            // Partition values into sets and deletes
            let (set_values, delete_values): (Vec<_>, Vec<_>) = result
                .values
                .into_iter()
                .partition(|v| matches!(v.change_type, ValueChangeType::Set));

            let delete_value_ids: Vec<_> = delete_values
                .into_iter()
                .map(|v| (v.id, v.space_id))
                .collect();

            // Partition relations by operation type
            let mut set_relations = Vec::new();
            let mut update_relations = Vec::new();
            let mut unset_relations = Vec::new();
            let mut delete_relations = Vec::new();

            for op in result.relations {
                match op {
                    RelationOp::Create(r) => set_relations.push(r),
                    RelationOp::Update(r) => update_relations.push(r),
                    RelationOp::Unset(r) => unset_relations.push(r),
                    RelationOp::Delete(r) => delete_relations.push((r.id, r.space_id)),
                }
            }

            let ops = result.entities.len()
                + set_values.len()
                + delete_value_ids.len()
                + set_relations.len()
                + update_relations.len()
                + unset_relations.len()
                + delete_relations.len();

            // Bulk insert all operations (live tables)
            storage.insert_entities(&result.entities, &mut tx).await?;
            storage.insert_values(&set_values, &mut tx).await?;
            storage.delete_values(&delete_value_ids, &mut tx).await?;
            storage.insert_relations(&set_relations, &mut tx).await?;
            storage.update_relations(&update_relations, &mut tx).await?;
            storage
                .unset_relation_fields(&unset_relations, &mut tx)
                .await?;
            storage.delete_relations(&delete_relations, &mut tx).await?;

            // Versioned writes (temporal tables)
            // Only write versions if this edit hasn't been processed before (idempotency)
            if let Some(meta) = edit.meta.as_ref() {
                let (edit_name, created_by_id) = extract_edit_metadata(&edit);

                if let Some(version_key) = storage
                    .insert_edit_version(
                        result.edit_id,
                        meta.block_number as i64,
                        meta.sequence as i64,
                        meta.created_at as i64,
                        edit_name.as_deref(),
                        created_by_id,
                        &mut tx,
                    )
                    .await?
                {
                    storage
                        .insert_value_versions(&values_for_versioning, version_key, &mut tx)
                        .await?;
                    storage
                        .insert_relation_versions(&relations_for_versioning, version_key, &mut tx)
                        .await?;
                }
            }

            ops
        }
        KgMessage::CreateSpace(space) => {
            let space_item = handlers::spaces::handle_create_space(&space)?;
            storage.insert_spaces(&[space_item], &mut tx).await?;

            // Create system entity in knowledge graph
            if let Some(meta) = space.meta.as_ref() {
                let system_result = handlers::system_entities::map_space_registered(&space, meta)?;
                storage
                    .insert_entities(&system_result.entities, &mut tx)
                    .await?;
                let set_values: Vec<_> =
                    system_result.values_to_set().into_iter().cloned().collect();
                storage.insert_values(&set_values, &mut tx).await?;
                let set_relations: Vec<_> = system_result
                    .relations_to_create()
                    .into_iter()
                    .cloned()
                    .collect();
                storage.insert_relations(&set_relations, &mut tx).await?;
            }
            1
        }
        KgMessage::RoleGranted(event) => {
            match handlers::membership::handle_role_granted(&event)? {
                MembershipChange::AddEditor(e) => {
                    storage.insert_editors(&[e], &mut tx).await?;
                }
                MembershipChange::AddMember(m) => {
                    storage.insert_members(&[m], &mut tx).await?;
                }
                _ => {} // Shouldn't happen for granted
            }
            1
        }
        KgMessage::RoleRevoked(event) => {
            match handlers::membership::handle_role_revoked(&event)? {
                MembershipChange::RemoveEditor(e) => {
                    storage.remove_editors(&[e], &mut tx).await?;
                }
                MembershipChange::RemoveMember(m) => {
                    storage.remove_members(&[m], &mut tx).await?;
                }
                _ => {} // Shouldn't happen for revoked
            }
            1
        }
        KgMessage::TrustExtension(event) => {
            match handlers::subspaces::handle_trust_extension(&event)? {
                Some(models::subspaces::SubspaceChange::InsertExplicit(item)) => {
                    storage.insert_subspaces(&[item], &mut tx).await?;
                    1
                }
                Some(models::subspaces::SubspaceChange::RemoveExplicit(item)) => {
                    storage.remove_subspaces(&[item], &mut tx).await?;
                    1
                }
                Some(models::subspaces::SubspaceChange::InsertTopic(item)) => {
                    storage.insert_subspace_topics(&[item], &mut tx).await?;
                    1
                }
                Some(models::subspaces::SubspaceChange::RemoveTopic(item)) => {
                    storage.remove_subspace_topics(&[item], &mut tx).await?;
                    1
                }
                None => 0,
            }
        }
        KgMessage::TopicDeclared(event) => {
            let assignment = handlers::topics::handle_topic_declared(&event)?;
            let topic_entity = make_topic_entity(assignment.topic_id, event.meta.as_ref());
            storage.insert_entities(&[topic_entity], &mut tx).await?;
            storage
                .update_space_topic(assignment.space_id, assignment.topic_id, &mut tx)
                .await?;
            2
        }
        KgMessage::TopicRemoved(event) => {
            let removal = handlers::topics::handle_topic_removed(&event)?;
            storage
                .clear_space_topic(removal.space_id, removal.topic_id, &mut tx)
                .await?;
            1
        }
        KgMessage::ProposalCreated(event) => {
            let result = handlers::governance::handle_proposal_created(&event)?;
            debug!(
                proposal_id = %result.identity.id,
                actions = result.actions.len(),
                "Processing ProposalCreated"
            );
            storage
                .insert_proposal_identity(&result.identity, &mut tx)
                .await?;
            storage
                .insert_proposal_version_initial(result.identity.id, &result.version, &mut tx)
                .await?;
            if !result.actions.is_empty() {
                storage
                    .insert_proposal_actions(&result.actions, &mut tx)
                    .await?;
            }

            // Create system entity in knowledge graph
            if let Some(meta) = event.meta.as_ref() {
                let system_result = handlers::system_entities::map_proposal_created(&event, meta)?;
                storage
                    .insert_entities(&system_result.entities, &mut tx)
                    .await?;
                let set_values: Vec<_> =
                    system_result.values_to_set().into_iter().cloned().collect();
                storage.insert_values(&set_values, &mut tx).await?;
                let set_relations: Vec<_> = system_result
                    .relations_to_create()
                    .into_iter()
                    .cloned()
                    .collect();
                storage.insert_relations(&set_relations, &mut tx).await?;
            }
            1 + result.actions.len()
        }
        KgMessage::ProposalUpdated(event) => {
            let result = handlers::governance::handle_proposal_updated(&event)?;
            debug!(
                proposal_id = %result.proposal_id,
                actions = result.actions.len(),
                "Processing ProposalUpdated"
            );
            // Append new version row + atomically bump proposals.current_version.
            let new_version = storage
                .insert_new_proposal_version(result.proposal_id, &result.version, &mut tx)
                .await?;
            // Stamp the assigned version onto the actions before writing them.
            // Actions are version-scoped (PK = proposal_id, proposal_version, index),
            // so prior-version actions remain as history rather than being deleted.
            let actions: Vec<_> = result
                .actions
                .into_iter()
                .map(|mut a| {
                    a.proposal_version = new_version;
                    a
                })
                .collect();
            if !actions.is_empty() {
                storage.insert_proposal_actions(&actions, &mut tx).await?;
            }
            1 + actions.len()
        }
        KgMessage::ProposalVoted(event) => {
            let vote = handlers::governance::handle_proposal_voted(&event)?;
            let proposal_id = vote.proposal_id;
            debug!(
                proposal_id = %vote.proposal_id,
                voter_id = %vote.voter_id,
                "Processing ProposalVoted"
            );
            storage.insert_proposal_votes(&[vote], &mut tx).await?;
            // Queue proposal for tally update (processed by background worker)
            storage.queue_tally_update(proposal_id, &mut tx).await?;
            1
        }
        KgMessage::ProposalExecuted(event) => {
            let execution = handlers::governance::handle_proposal_executed(&event)?;
            debug!(
                proposal_id = %execution.proposal_id,
                "Processing ProposalExecuted"
            );
            storage
                .update_proposal_executed(execution.proposal_id, execution.executed_at, &mut tx)
                .await?;
            1
        }
        KgMessage::ProposalSettingsUpdated(event) => {
            let result = handlers::governance::handle_proposal_settings_updated(&event)?;
            let voting_mode = match result.voting_mode {
                models::governance::VotingMode::Fast => "Fast",
                models::governance::VotingMode::Slow => "Slow",
            };
            debug!(
                proposal_id = %result.proposal_id,
                voting_mode = voting_mode,
                "Processing ProposalSettingsUpdated"
            );
            storage
                .update_proposal_settings(
                    result.proposal_id,
                    voting_mode,
                    result.start_time,
                    result.end_time,
                    result.quorum,
                    result.threshold,
                    result.partial_percentage_support_threshold,
                    result.universal_percentage_support_threshold,
                    result.flat_support_threshold,
                    result.execute_by,
                    &mut tx,
                )
                .await?;
            1
        }
        KgMessage::VotingSettingsUpdated(event) => {
            let item = handlers::governance::handle_voting_settings_updated(&event)?;
            debug!(
                space_id = %item.space_id,
                "Processing VotingSettingsUpdated"
            );
            storage.upsert_space_voting_settings(&item, &mut tx).await?;
            1
        }
    };

    tx.commit().await?;

    Ok(ops)
}

/// Process all events in a block within a single transaction.
/// Events should already be sorted by sequence.
/// Returns the total number of database operations performed.
struct ProcessBlockResult {
    ops: usize,
    commit_failures: u64,
    db_tx_duration_ms: u128,
}

async fn process_block(
    events: Vec<BufferedEvent>,
    storage: &Storage,
    consumer: &KafkaConsumer,
) -> Result<ProcessBlockResult, IndexerError> {
    use handlers::membership::MembershipChange;
    use models::relations::RelationOp;
    use models::values::ValueChangeType;

    if events.is_empty() {
        return Ok(ProcessBlockResult {
            ops: 0,
            commit_failures: 0,
            db_tx_duration_ms: 0,
        });
    }

    let mut tx = storage.pool.begin().await?;
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await?;
    let mut total_ops = 0;
    let tx_start = Instant::now();
    let mut pending_space_topics: HashMap<uuid::Uuid, Option<uuid::Uuid>> = HashMap::new();

    // Process each message in sequence order
    for event in &events {
        let event_id = event.event_id.as_deref().unwrap_or("");

        // Create event-specific span with only the fields each event type needs
        let event_span = match &event.msg {
            KgMessage::Edit(_) => info_span!(
                "kg_indexer.handle_edit",
                event_id = event_id,
                edit_id = tracing::field::Empty,
                space_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::CreateSpace(_) => info_span!(
                "kg_indexer.handle_create_space",
                event_id = event_id,
                space_id = tracing::field::Empty,
                space_address = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::RoleGranted(_) | KgMessage::RoleRevoked(_) => info_span!(
                "kg_indexer.handle_role_change",
                event_id = event_id,
                space_id = tracing::field::Empty,
                account = tracing::field::Empty,
                role = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::TrustExtension(_) => info_span!(
                "kg_indexer.handle_trust_extension",
                event_id = event_id,
                extension_type = tracing::field::Empty,
                parent_space_id = tracing::field::Empty,
                child_space_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::TopicDeclared(_) => info_span!(
                "kg_indexer.handle_topic_declared",
                event_id = event_id,
                space_id = tracing::field::Empty,
                topic_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::TopicRemoved(_) => info_span!(
                "kg_indexer.handle_topic_removed",
                event_id = event_id,
                space_id = tracing::field::Empty,
                topic_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::ProposalCreated(_) | KgMessage::ProposalUpdated(_) => info_span!(
                "kg_indexer.handle_proposal",
                event_id = event_id,
                proposal_id = tracing::field::Empty,
                space_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::ProposalVoted(_) => info_span!(
                "kg_indexer.handle_proposal_voted",
                event_id = event_id,
                proposal_id = tracing::field::Empty,
                voter_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::ProposalExecuted(_) => info_span!(
                "kg_indexer.handle_proposal_executed",
                event_id = event_id,
                proposal_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::ProposalSettingsUpdated(_) => info_span!(
                "kg_indexer.handle_proposal_settings_updated",
                event_id = event_id,
                proposal_id = tracing::field::Empty,
                space_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::VotingSettingsUpdated(_) => info_span!(
                "kg_indexer.handle_voting_settings_updated",
                event_id = event_id,
                space_id = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
            KgMessage::BlockSummary(_) => info_span!(
                "kg_indexer.handle_block_summary",
                event_id = event_id,
                "otel.status_code" = tracing::field::Empty,
                "otel.status_message" = tracing::field::Empty
            ),
        };

        // Use instrument() instead of enter() for async code to properly track span across await points
        let ops = async {
            Ok::<usize, IndexerError>(match &event.msg {
                KgMessage::Edit(edit) => {
                    let result = handlers::edits::handle_edit(edit)?;

                    // Record trace context
                    event_span.record("edit_id", display(result.edit_id));
                    if let Ok(space_id) = uuid::Uuid::from_slice(&edit.space_id) {
                        event_span.record("space_id", display(space_id));
                    }

                    // Keep copies for versioned writes before partitioning
                    let values_for_versioning = result.values.clone();
                    let relations_for_versioning = result.relations.clone();

                    // Partition values into sets and deletes
                    let (set_values, delete_values): (Vec<_>, Vec<_>) = result
                        .values
                        .into_iter()
                        .partition(|v| matches!(v.change_type, ValueChangeType::Set));

                    let delete_value_ids: Vec<_> = delete_values
                        .into_iter()
                        .map(|v| (v.id, v.space_id))
                        .collect();

                    // Partition relations by operation type
                    let mut set_relations = Vec::new();
                    let mut update_relations = Vec::new();
                    let mut unset_relations = Vec::new();
                    let mut delete_relations = Vec::new();

                    for op in result.relations {
                        match op {
                            RelationOp::Create(r) => set_relations.push(r),
                            RelationOp::Update(r) => update_relations.push(r),
                            RelationOp::Unset(r) => unset_relations.push(r),
                            RelationOp::Delete(r) => delete_relations.push((r.id, r.space_id)),
                        }
                    }

                    let ops = result.entities.len()
                        + set_values.len()
                        + delete_value_ids.len()
                        + set_relations.len()
                        + update_relations.len()
                        + unset_relations.len()
                        + delete_relations.len();

                    // Bulk insert all operations (live tables)
                    storage.insert_entities(&result.entities, &mut tx).await?;
                    storage.insert_values(&set_values, &mut tx).await?;
                    storage.delete_values(&delete_value_ids, &mut tx).await?;
                    storage.insert_relations(&set_relations, &mut tx).await?;
                    storage.update_relations(&update_relations, &mut tx).await?;
                    storage
                        .unset_relation_fields(&unset_relations, &mut tx)
                        .await?;
                    storage.delete_relations(&delete_relations, &mut tx).await?;

                    // Versioned writes (temporal tables)
                    // Only write versions if this edit hasn't been processed before (idempotency)
                    if let Some(meta) = edit.meta.as_ref() {
                        let (edit_name, created_by_id) = extract_edit_metadata(edit);

                        if let Some(version_key) = storage
                            .insert_edit_version(
                                result.edit_id,
                                meta.block_number as i64,
                                meta.sequence as i64,
                                meta.created_at as i64,
                                edit_name.as_deref(),
                                created_by_id,
                                &mut tx,
                            )
                            .await?
                        {
                            storage
                                .insert_value_versions(&values_for_versioning, version_key, &mut tx)
                                .await?;
                            storage
                                .insert_relation_versions(
                                    &relations_for_versioning,
                                    version_key,
                                    &mut tx,
                                )
                                .await?;
                        }
                    }

                    ops
                }
                KgMessage::CreateSpace(space) => {
                    let mut space_item = handlers::spaces::handle_create_space(space)?;
                    apply_pending_space_topic(&mut space_item, &pending_space_topics);

                    // Record trace context
                    event_span.record("space_id", display(space_item.id));
                    event_span.record("space_address", space_item.address.as_str());

                    storage.insert_spaces(&[space_item], &mut tx).await?;

                    // Create system entity in knowledge graph
                    if let Some(meta) = space.meta.as_ref() {
                        let system_result =
                            handlers::system_entities::map_space_registered(space, meta)?;
                        storage
                            .insert_entities(&system_result.entities, &mut tx)
                            .await?;
                        let set_values: Vec<_> =
                            system_result.values_to_set().into_iter().cloned().collect();
                        storage.insert_values(&set_values, &mut tx).await?;
                        let set_relations: Vec<_> = system_result
                            .relations_to_create()
                            .into_iter()
                            .cloned()
                            .collect();
                        storage.insert_relations(&set_relations, &mut tx).await?;
                    }
                    1
                }
                KgMessage::RoleGranted(role_event) => {
                    // Record trace context
                    if let Ok(space_id) = uuid::Uuid::from_slice(&role_event.space_id) {
                        event_span.record("space_id", display(space_id));
                    }
                    if let Ok(member_id) = uuid::Uuid::from_slice(&role_event.member_space_id) {
                        event_span.record("account", display(member_id));
                    }
                    if let Ok(role) =
                        hermes_schema::pb::membership::MembershipRole::try_from(role_event.role)
                    {
                        event_span.record("role", role.as_str_name());
                    }

                    match handlers::membership::handle_role_granted(role_event)? {
                        MembershipChange::AddEditor(e) => {
                            storage.insert_editors(&[e], &mut tx).await?;
                        }
                        MembershipChange::AddMember(m) => {
                            storage.insert_members(&[m], &mut tx).await?;
                        }
                        _ => {}
                    }
                    1
                }
                KgMessage::RoleRevoked(role_event) => {
                    // Record trace context
                    if let Ok(space_id) = uuid::Uuid::from_slice(&role_event.space_id) {
                        event_span.record("space_id", display(space_id));
                    }
                    if let Ok(member_id) = uuid::Uuid::from_slice(&role_event.member_space_id) {
                        event_span.record("account", display(member_id));
                    }
                    if let Ok(role) =
                        hermes_schema::pb::membership::MembershipRole::try_from(role_event.role)
                    {
                        event_span.record("role", role.as_str_name());
                    }

                    match handlers::membership::handle_role_revoked(role_event)? {
                        MembershipChange::RemoveEditor(e) => {
                            storage.remove_editors(&[e], &mut tx).await?;
                        }
                        MembershipChange::RemoveMember(m) => {
                            storage.remove_members(&[m], &mut tx).await?;
                        }
                        _ => {}
                    }
                    1
                }
                KgMessage::TrustExtension(trust_event) => {
                    // Record trace context
                    let extension_type = match &trust_event.extension {
                        Some(TrustExtensionType::Verified(_)) => "verified",
                        Some(TrustExtensionType::Related(_)) => "related",
                        Some(TrustExtensionType::Subtopic(_)) => "subtopic",
                        Some(TrustExtensionType::VerifiedRemoval(_)) => "verified_removal",
                        Some(TrustExtensionType::RelatedRemoval(_)) => "related_removal",
                        Some(TrustExtensionType::SubtopicRemoval(_)) => "subtopic_removal",
                        None => "unknown",
                    };
                    event_span.record("extension_type", extension_type);

                    match handlers::subspaces::handle_trust_extension(trust_event)? {
                        Some(models::subspaces::SubspaceChange::InsertExplicit(item)) => {
                            event_span.record("parent_space_id", display(item.parent_space_id));
                            event_span.record("child_space_id", display(item.subspace_id));
                            storage.insert_subspaces(&[item], &mut tx).await?;
                            1
                        }
                        Some(models::subspaces::SubspaceChange::RemoveExplicit(item)) => {
                            event_span.record("parent_space_id", display(item.parent_space_id));
                            event_span.record("child_space_id", display(item.subspace_id));
                            storage.remove_subspaces(&[item], &mut tx).await?;
                            1
                        }
                        Some(models::subspaces::SubspaceChange::InsertTopic(item)) => {
                            event_span.record("space_id", display(item.space_id));
                            event_span.record("topic_id", display(item.topic_id));
                            let (created_at, created_at_block) =
                                blockchain_metadata_to_strings(trust_event.meta.as_ref());
                            let topic_entity = models::entities::EntityItem {
                                id: item.topic_id,
                                created_at: created_at.clone(),
                                created_at_block: created_at_block.clone(),
                                updated_at: created_at,
                                updated_at_block: created_at_block,
                            };
                            storage.insert_entities(&[topic_entity], &mut tx).await?;
                            storage.insert_subspace_topics(&[item], &mut tx).await?;
                            1
                        }
                        Some(models::subspaces::SubspaceChange::RemoveTopic(item)) => {
                            event_span.record("space_id", display(item.space_id));
                            event_span.record("topic_id", display(item.topic_id));
                            storage.remove_subspace_topics(&[item], &mut tx).await?;
                            1
                        }
                        None => 0,
                    }
                }
                KgMessage::TopicDeclared(topic_event) => {
                    let assignment = handlers::topics::handle_topic_declared(topic_event)?;

                    event_span.record("space_id", display(assignment.space_id));
                    event_span.record("topic_id", display(assignment.topic_id));

                    let topic_entity =
                        make_topic_entity(assignment.topic_id, topic_event.meta.as_ref());
                    storage.insert_entities(&[topic_entity], &mut tx).await?;
                    storage
                        .update_space_topic(assignment.space_id, assignment.topic_id, &mut tx)
                        .await?;
                    pending_space_topics.insert(assignment.space_id, Some(assignment.topic_id));
                    2
                }
                KgMessage::TopicRemoved(topic_event) => {
                    let removal = handlers::topics::handle_topic_removed(topic_event)?;

                    event_span.record("space_id", display(removal.space_id));
                    event_span.record("topic_id", display(removal.topic_id));

                    storage
                        .clear_space_topic(removal.space_id, removal.topic_id, &mut tx)
                        .await?;
                    pending_space_topics.insert(removal.space_id, None);
                    1
                }
                KgMessage::ProposalCreated(proposal_event) => {
                    let result = handlers::governance::handle_proposal_created(proposal_event)?;

                    // Record trace context
                    event_span.record("proposal_id", display(result.identity.id));
                    event_span.record("space_id", display(result.identity.space_id));

                    storage
                        .insert_proposal_identity(&result.identity, &mut tx)
                        .await?;
                    storage
                        .insert_proposal_version_initial(
                            result.identity.id,
                            &result.version,
                            &mut tx,
                        )
                        .await?;
                    if !result.actions.is_empty() {
                        storage
                            .insert_proposal_actions(&result.actions, &mut tx)
                            .await?;
                    }

                    // Create system entity in knowledge graph
                    if let Some(meta) = proposal_event.meta.as_ref() {
                        let system_result =
                            handlers::system_entities::map_proposal_created(proposal_event, meta)?;
                        storage
                            .insert_entities(&system_result.entities, &mut tx)
                            .await?;
                        let set_values: Vec<_> =
                            system_result.values_to_set().into_iter().cloned().collect();
                        storage.insert_values(&set_values, &mut tx).await?;
                        let set_relations: Vec<_> = system_result
                            .relations_to_create()
                            .into_iter()
                            .cloned()
                            .collect();
                        storage.insert_relations(&set_relations, &mut tx).await?;
                    }
                    1 + result.actions.len()
                }
                KgMessage::ProposalUpdated(proposal_event) => {
                    let result = handlers::governance::handle_proposal_updated(proposal_event)?;

                    // Record trace context
                    event_span.record("proposal_id", display(result.proposal_id));

                    // Append new version row + atomically bump proposals.current_version.
                    let new_version = storage
                        .insert_new_proposal_version(result.proposal_id, &result.version, &mut tx)
                        .await?;
                    // Stamp the assigned version onto actions before writing.
                    let actions: Vec<_> = result
                        .actions
                        .into_iter()
                        .map(|mut a| {
                            a.proposal_version = new_version;
                            a
                        })
                        .collect();
                    if !actions.is_empty() {
                        storage.insert_proposal_actions(&actions, &mut tx).await?;
                    }
                    1 + actions.len()
                }
                KgMessage::ProposalVoted(vote_event) => {
                    let vote = handlers::governance::handle_proposal_voted(vote_event)?;
                    let proposal_id = vote.proposal_id;

                    // Record trace context
                    event_span.record("proposal_id", display(vote.proposal_id));
                    event_span.record("voter_id", display(vote.voter_id));

                    storage.insert_proposal_votes(&[vote], &mut tx).await?;
                    // Queue proposal for tally update (processed by background worker)
                    storage.queue_tally_update(proposal_id, &mut tx).await?;
                    1
                }
                KgMessage::ProposalExecuted(exec_event) => {
                    let execution = handlers::governance::handle_proposal_executed(exec_event)?;

                    // Record trace context
                    event_span.record("proposal_id", display(execution.proposal_id));

                    storage
                        .update_proposal_executed(
                            execution.proposal_id,
                            execution.executed_at,
                            &mut tx,
                        )
                        .await?;
                    1
                }
                KgMessage::ProposalSettingsUpdated(settings_event) => {
                    let result =
                        handlers::governance::handle_proposal_settings_updated(settings_event)?;
                    let voting_mode = match result.voting_mode {
                        models::governance::VotingMode::Fast => "Fast",
                        models::governance::VotingMode::Slow => "Slow",
                    };

                    // Record trace context
                    event_span.record("proposal_id", display(result.proposal_id));
                    event_span.record("space_id", display(result.space_id));

                    storage
                        .update_proposal_settings(
                            result.proposal_id,
                            voting_mode,
                            result.start_time,
                            result.end_time,
                            result.quorum,
                            result.threshold,
                            result.partial_percentage_support_threshold,
                            result.universal_percentage_support_threshold,
                            result.flat_support_threshold,
                            result.execute_by,
                            &mut tx,
                        )
                        .await?;
                    1
                }
                KgMessage::VotingSettingsUpdated(voting_settings_event) => {
                    let item = handlers::governance::handle_voting_settings_updated(
                        voting_settings_event,
                    )?;

                    event_span.record("space_id", display(item.space_id));

                    storage.upsert_space_voting_settings(&item, &mut tx).await?;
                    1
                }
                KgMessage::BlockSummary(_) => 0,
            })
        }
        .instrument(event_span.clone())
        .await
        .map_err(|e| {
            event_span.record("otel.status_code", "ERROR");
            event_span.record("otel.status_message", e.to_string().as_str());
            error!(
                event_id = event.event_id.as_deref().unwrap_or(""),
                error = %e,
                "Failed to process event in block"
            );
            e
        })?;

        total_ops += ops;
    }

    // Commit the transaction
    let db_commit_span = info_span!(
        "kg_indexer.db_commit",
        "otel.status_code" = tracing::field::Empty,
        "otel.status_message" = tracing::field::Empty
    );
    let db_commit_result = async { tx.commit().await }
        .instrument(db_commit_span.clone())
        .await;

    let db_tx_duration_ms = tx_start.elapsed().as_millis();

    if let Err(e) = db_commit_result {
        db_commit_span.record("otel.status_code", "ERROR");
        db_commit_span.record("otel.status_message", e.to_string().as_str());
        return Err(e.into());
    }

    // Commit Kafka offsets for all processed messages
    let kafka_commit_span = info_span!(
        "kg_indexer.kafka_commit",
        event_count = events.len(),
        "otel.status_code" = tracing::field::Empty,
        "otel.status_message" = tracing::field::Empty
    );
    let _kafka_commit_guard = kafka_commit_span.enter();

    let mut commit_failures = 0;
    for event in events {
        if let Err(e) = consumer.commit_message(&event.topic, event.partition, event.offset) {
            commit_failures += 1;
            error!(
                topic = %event.topic,
                partition = event.partition,
                offset = event.offset,
                event_id = event.event_id.as_deref().unwrap_or(""),
                error = %e,
                "Failed to commit offset"
            );
        }
    }

    if commit_failures > 0 {
        kafka_commit_span.record("otel.status_code", "ERROR");
        kafka_commit_span.record(
            "otel.status_message",
            format!("{} offset commits failed", commit_failures).as_str(),
        );
    }

    Ok(ProcessBlockResult {
        ops: total_ops,
        commit_failures,
        db_tx_duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer::KgMessage;
    use crate::models::spaces::{SpaceItem, SpaceType};

    #[test]
    fn test_apply_pending_space_topic_sets_topic_id() {
        let space_id = uuid::Uuid::new_v4();
        let topic_id = uuid::Uuid::new_v4();
        let mut pending = HashMap::new();
        pending.insert(space_id, Some(topic_id));

        let mut space = SpaceItem {
            id: space_id,
            space_type: SpaceType::Dao,
            address: "0x123".to_string(),
            topic_id: None,
        };

        apply_pending_space_topic(&mut space, &pending);

        assert_eq!(space.topic_id, Some(topic_id));
    }

    /// A `TOPIC_REMOVED` for a space that's also being registered in the same
    /// batch must override any prior declared value (start `topic_id = Some(X)`
    /// on the in-memory SpaceItem, end with `None`).
    #[test]
    fn test_apply_pending_space_topic_clears_topic_id_on_removal() {
        let space_id = uuid::Uuid::new_v4();
        let prior_topic = uuid::Uuid::new_v4();
        let mut pending = HashMap::new();
        pending.insert(space_id, None);

        let mut space = SpaceItem {
            id: space_id,
            space_type: SpaceType::Dao,
            address: "0x123".to_string(),
            topic_id: Some(prior_topic),
        };

        apply_pending_space_topic(&mut space, &pending);

        assert_eq!(space.topic_id, None);
    }

    /// Spaces with no in-batch topic event are untouched.
    #[test]
    fn test_apply_pending_space_topic_noop_when_absent() {
        let space_id = uuid::Uuid::new_v4();
        let other_space_id = uuid::Uuid::new_v4();
        let topic_id = uuid::Uuid::new_v4();
        let mut pending = HashMap::new();
        pending.insert(other_space_id, Some(topic_id));

        let mut space = SpaceItem {
            id: space_id,
            space_type: SpaceType::Dao,
            address: "0x123".to_string(),
            topic_id: Some(topic_id),
        };

        apply_pending_space_topic(&mut space, &pending);

        assert_eq!(space.topic_id, Some(topic_id));
    }

    #[test]
    fn test_stale_blocks_use_fixed_deadline_from_first_seen() {
        let mut buffer = BlockBuffer::new(Duration::from_millis(10));
        let block_number = 42;

        buffer.push(
            block_number,
            BufferedEvent {
                msg: KgMessage::CreateSpace(hermes_schema::pb::space::HermesCreateSpace {
                    meta: Some(BlockchainMetadata {
                        created_at: 0,
                        created_by: vec![],
                        block_number,
                        cursor: "cursor".to_string(),
                        sequence: 0,
                        is_last: false,
                    }),
                    space_id: vec![0; 16],
                    payload: None,
                }),
                topic: "space.creations".to_string(),
                partition: 0,
                offset: 0,
                event_type: Some("SPACE_REGISTERED".to_string()),
                event_id: None,
            },
        );

        std::thread::sleep(Duration::from_millis(7));
        buffer.insert_summary(
            block_number,
            hermes_schema::pb::block_summary::HermesBlockSummary {
                block_number,
                cursor: "cursor".to_string(),
                created_at: 0,
                total_events: 1,
                counts_by_topic: HashMap::new(),
                counts_by_event_type: HashMap::new(),
            },
            1,
        );

        std::thread::sleep(Duration::from_millis(7));
        assert!(
            buffer.is_stale(block_number),
            "timeout should be measured from first block sighting, not reset by later summary arrival"
        );
    }

    #[test]
    fn test_stale_blocks_when_summary_arrives_before_events() {
        let mut buffer = BlockBuffer::new(Duration::from_millis(10));
        let block_number = 7;

        buffer.insert_summary(
            block_number,
            hermes_schema::pb::block_summary::HermesBlockSummary {
                block_number,
                cursor: "cursor".to_string(),
                created_at: 0,
                total_events: 1,
                counts_by_topic: HashMap::new(),
                counts_by_event_type: HashMap::new(),
            },
            1,
        );

        std::thread::sleep(Duration::from_millis(11));
        assert!(buffer.is_stale(block_number));
    }

    fn make_event(block_number: u64) -> BufferedEvent {
        BufferedEvent {
            msg: KgMessage::CreateSpace(hermes_schema::pb::space::HermesCreateSpace {
                meta: Some(BlockchainMetadata {
                    created_at: 0,
                    created_by: vec![],
                    block_number,
                    cursor: "cursor".to_string(),
                    sequence: 0,
                    is_last: false,
                }),
                space_id: vec![0; 16],
                payload: None,
            }),
            topic: "space.creations".to_string(),
            partition: 0,
            offset: 0,
            event_type: Some("SPACE_REGISTERED".to_string()),
            event_id: None,
        }
    }

    fn make_summary(block_number: u64) -> hermes_schema::pb::block_summary::HermesBlockSummary {
        hermes_schema::pb::block_summary::HermesBlockSummary {
            block_number,
            cursor: "cursor".to_string(),
            created_at: 0,
            total_events: 1,
            counts_by_topic: HashMap::new(),
            counts_by_event_type: HashMap::new(),
        }
    }

    #[test]
    fn test_min_pending_block_tracks_lowest_buffered_block() {
        let mut buffer = BlockBuffer::new(Duration::from_secs(60));
        assert_eq!(buffer.min_pending_block(), None);

        buffer.push(10, make_event(10));
        buffer.push(8, make_event(8));
        buffer.insert_summary(5, make_summary(5), 1);

        assert_eq!(buffer.min_pending_block(), Some(5));

        buffer.take_summary(5);
        buffer.take_block(5);
        assert_eq!(buffer.min_pending_block(), Some(8));
    }

    #[test]
    fn test_is_complete_requires_summary_and_all_events() {
        let mut buffer = BlockBuffer::new(Duration::from_secs(60));
        let block_number = 3;

        buffer.push(block_number, make_event(block_number));
        assert!(!buffer.is_complete(block_number));

        buffer.insert_summary(block_number, make_summary(block_number), 2);
        assert!(!buffer.is_complete(block_number));

        buffer.push(block_number, make_event(block_number));
        assert!(buffer.is_complete(block_number));
    }
}
