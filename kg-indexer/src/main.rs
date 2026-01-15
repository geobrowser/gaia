use std::collections::HashMap;
use std::env;
use std::time::{Duration, Instant};

use futures::StreamExt;
use hermes_instrumentation::{debug, error, info, info_span, warn, Instrument};
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use opentelemetry::propagation::Extractor;
use rdkafka::message::Headers;
use rdkafka::Message;
use std::sync::OnceLock;
use tracing_opentelemetry::OpenTelemetrySpanExt;

mod consumer;
mod error;
mod handlers;
mod models;
mod storage;

use consumer::{get_event_type, parse_message, KafkaConsumer, KgMessage};
use error::IndexerError;
use storage::Storage;

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
    /// When each block was first seen.
    first_seen: HashMap<u64, Instant>,
    /// Block summaries keyed by block number.
    summaries: HashMap<u64, BlockSummaryInfo>,
    /// Timeout for waiting for is_last event.
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
                received_at: Instant::now(),
                expected_count,
            },
        );
    }

    fn take_summary(&mut self, block_number: u64) -> Option<BlockSummaryInfo> {
        self.summaries.remove(&block_number)
    }

    fn summary(&self, block_number: u64) -> Option<BlockSummaryInfo> {
        self.summaries.get(&block_number).cloned()
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

    /// Get block numbers that have been buffered longer than the stale timeout.
    fn stale_blocks(&self) -> Vec<u64> {
        let now = Instant::now();
        let mut blocks = Vec::new();

        for (block, first_seen) in &self.first_seen {
            if now.duration_since(*first_seen) > self.stale_timeout {
                blocks.push(*block);
            }
        }

        for (block, summary) in &self.summaries {
            if now.duration_since(summary.received_at) > self.stale_timeout {
                blocks.push(*block);
            }
        }

        blocks.sort();
        blocks.dedup();
        blocks
    }
}

#[derive(Clone)]
struct BlockSummaryInfo {
    summary: hermes_schema::pb::block_summary::HermesBlockSummary,
    received_at: Instant,
    expected_count: usize,
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
        .unwrap_or(1000);

    // Initialize storage
    let storage = Storage::new(&database_url).await?;
    info!("Connected to database");

    let mut property_types: HashMap<uuid::Uuid, models::properties::DataType> = HashMap::new();
    let existing_properties = storage.get_all_properties().await?;
    for property in existing_properties {
        property_types.insert(property.id, property.data_type);
    }
    info!(
        property_count = property_types.len(),
        "Loaded property data types"
    );

    // Initialize Kafka consumer
    let consumer = KafkaConsumer::new(&kafka_broker, &kafka_group_id)?;
    consumer.subscribe()?;

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Main processing loop
    //
    // Events are buffered by block number and processed together when `is_last=true`
    // arrives, ensuring correct ordering within a block. However, we can't rely solely
    // on `is_last` because:
    //   1. It may arrive on a topic we don't subscribe to (e.g., curation.votes)
    //   2. The producer may crash before sending it
    //   3. Network issues may cause it to be lost
    //
    // To handle these cases, we use `tokio::select!` with a periodic tick that checks
    // for stale blocks (buffered longer than `stale_timeout`). The tick runs independently
    // of the Kafka stream, so even if no messages arrive, stale blocks get processed.
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
                // Periodically check for stale blocks and force-process them
                for block_number in buffer.stale_blocks() {
                    let events = buffer.take_block(block_number);
                    let event_count = events.len();
                    if event_count == 0 {
                        // Nothing buffered; don't emit noisy stale logs.
                        continue;
                    }
                    warn!(
                        block_number = block_number,
                        event_count = event_count,
                        stale_timeout_ms = stale_timeout_ms,
                        "Force-processing stale block"
                    );
                    let summary = buffer.take_summary(block_number);
                    let result = process_buffered_block(
                        events,
                        &storage,
                        &consumer,
                        summary,
                        BlockProcessReason::Stale,
                        &mut property_types,
                    )
                    .await;
                    if let Some((processed, errors)) = result {
                        processed_count += processed;
                        error_count += errors;
                        blocks_processed += 1;
                    }
                }
            }

            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();
                        let event_type = get_event_type(msg.headers());
                        let event_id_header = get_header_value(msg.headers(), "event-id");
                        let parent_cx = extract_parent_context(msg.headers());

                        let span = info_span!(
                            "kg_indexer.poll",
                            topic = %topic,
                            partition = partition,
                            offset = offset,
                            event_type = event_type.as_deref().unwrap_or(""),
                            event_id = tracing::field::Empty,
                            block_number = tracing::field::Empty,
                            is_last = tracing::field::Empty
                        );
                        let _ = span.set_parent(parent_cx);

                        let fut = async {
                            if let Some(payload) = msg.payload() {
                                match parse_message(&topic, payload, event_type.as_deref()) {
                                    Ok(kg_msg) => {
                                        if let KgMessage::BlockSummary(summary) = kg_msg {
                                            let expected_count =
                                                expected_count_for_indexer(&summary);
                                            let summary_block_number = summary.block_number;

                                            if expected_count > 0 {
                                                info!(
                                                    event = "kg_indexer.block_summary_received",
                                                    block_number = summary_block_number,
                                                    expected_count = expected_count,
                                                    total_events = summary.total_events,
                                                    "Block summary received"
                                                );
                                            }
                                            if log_event_ids_enabled() && expected_count > 0 {
                                                info!(
                                                    event = "kg_indexer.event_id",
                                                    topic = %topic,
                                                    event_id = event_id_header.as_deref().unwrap_or(""),
                                                    event_type = "BLOCK_SUMMARY",
                                                    block_number = summary_block_number,
                                                    "Received event"
                                                );
                                            }

                                            if expected_count == 0 {
                                                return;
                                            }

                                            buffer.insert_summary(
                                                summary_block_number,
                                                summary,
                                                expected_count,
                                            );

                                            if buffer.buffered_count(summary_block_number)
                                                >= expected_count
                                            {
                                                let summary_info =
                                                    buffer.take_summary(summary_block_number);
                                                let events = buffer.take_block(summary_block_number);
                                                let result = process_buffered_block(
                                                    events,
                                                    &storage,
                                                    &consumer,
                                                    summary_info,
                                                    BlockProcessReason::Summary,
                                                    &mut property_types,
                                                )
                                                .await;
                                                if let Some((processed, errors)) = result {
                                                    processed_count += processed;
                                                    error_count += errors;
                                                    blocks_processed += 1;
                                                }
                                            }

                                            return;
                                        }

                                        let event_id = event_id_header.or_else(|| {
                                            kg_msg
                                                .meta()
                                                .map(|meta| event_id_from_meta(meta, &topic))
                                        });
                                        if let Some(ref event_id) = event_id {
                                            tracing::Span::current()
                                                .record("event_id", event_id.as_str());
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
                                                &mut property_types,
                                            )
                                            .await
                                            {
                                                Ok(_) => {
                                                    processed_count += 1;
                                                }
                                                Err(e) => {
                                                    error!(
                                                        event_id = event_id.as_deref().unwrap_or(""),
                                                        error = %e,
                                                        "Failed to process message"
                                                    );
                                                    error_count += 1;
                                                }
                                            }
                                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
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
                                        buffer.push(block_number, BufferedEvent {
                                            msg: kg_msg,
                                            topic,
                                            partition,
                                            offset,
                                            event_type: event_type.clone(),
                                            event_id: event_id.clone(),
                                        });

                                        if let Some(summary) = buffer.summary(block_number) {
                                            if buffer.buffered_count(block_number)
                                                >= summary.expected_count
                                            {
                                                let summary_info = buffer.take_summary(block_number);
                                                let events = buffer.take_block(block_number);
                                                let result = process_buffered_block(
                                                    events,
                                                    &storage,
                                                    &consumer,
                                                    summary_info,
                                                    BlockProcessReason::Summary,
                                                    &mut property_types,
                                                )
                                                .await;
                                                if let Some((processed, errors)) = result {
                                                    processed_count += processed;
                                                    error_count += errors;
                                                    blocks_processed += 1;
                                                }
                                                return;
                                            }
                                        }

                                        // If this is the last event in the block, process all buffered events
                                        if is_last {
                                            let events = buffer.take_block(block_number);
                                            let event_count = events.len();

                                            debug!(
                                                block_number = block_number,
                                                event_count = event_count,
                                                "Processing block"
                                            );

                                            let summary = buffer.take_summary(block_number);
                                            let result = process_buffered_block(
                                                events,
                                                &storage,
                                                &consumer,
                                                summary,
                                                BlockProcessReason::IsLast,
                                                &mut property_types,
                                            )
                                            .await;
                                            if let Some((processed, errors)) = result {
                                                processed_count += processed;
                                                error_count += errors;
                                                blocks_processed += 1;
                                            }

                                            if blocks_processed.is_multiple_of(10)
                                                && blocks_processed > 0
                                            {
                                                info!(
                                                    blocks = blocks_processed,
                                                    messages = processed_count,
                                                    errors = error_count,
                                                    "Progress update"
                                                );
                                            }
                                        }
                                    }
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
                                    if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                        error!(
                                            event_id = event_id_header.as_deref().unwrap_or(""),
                                            error = %e,
                                            "Failed to commit offset"
                                        );
                                    }
                                }
                            }
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

struct KafkaHeadersExtractor<'a> {
    headers: Option<&'a rdkafka::message::BorrowedHeaders>,
}

impl<'a> Extractor for KafkaHeadersExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.headers.and_then(|headers| {
            for header in headers.iter() {
                if header.key.eq_ignore_ascii_case(key) {
                    if let Some(value) = header.value {
                        if let Ok(value_str) = std::str::from_utf8(value) {
                            return Some(value_str);
                        }
                    }
                }
            }
            None
        })
    }

    fn keys(&self) -> Vec<&str> {
        self.headers
            .map(|headers| headers.iter().map(|header| header.key).collect())
            .unwrap_or_default()
    }
}

fn extract_parent_context(
    headers: Option<&rdkafka::message::BorrowedHeaders>,
) -> opentelemetry::Context {
    let extractor = KafkaHeadersExtractor { headers };
    opentelemetry::global::get_text_map_propagator(|prop| prop.extract(&extractor))
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
    "space.governance",
];

const EXPECTED_EVENT_TYPES: &[&str] = &[
    "EDITS_PUBLISHED",
    "SPACE_REGISTERED",
    "ROLE_GRANTED",
    "ROLE_REVOKED",
    "TRUST_EXTENSION",
    "PROPOSAL_CREATED",
    "PROPOSAL_UPDATED",
    "PROPOSAL_VOTED",
    "PROPOSAL_EXECUTED",
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
    IsLast,
    Stale,
}

impl BlockProcessReason {
    fn as_str(&self) -> &'static str {
        match self {
            BlockProcessReason::Summary => "summary",
            BlockProcessReason::IsLast => "is_last",
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
        KgMessage::ProposalCreated(_) => "PROPOSAL_CREATED".to_string(),
        KgMessage::ProposalUpdated(_) => "PROPOSAL_UPDATED".to_string(),
        KgMessage::ProposalVoted(_) => "PROPOSAL_VOTED".to_string(),
        KgMessage::ProposalExecuted(_) => "PROPOSAL_EXECUTED".to_string(),
        KgMessage::BlockSummary(_) => "BLOCK_SUMMARY".to_string(),
    }
}

async fn process_buffered_block(
    events: Vec<BufferedEvent>,
    storage: &Storage,
    consumer: &KafkaConsumer,
    summary_info: Option<BlockSummaryInfo>,
    reason: BlockProcessReason,
    property_types: &mut HashMap<uuid::Uuid, models::properties::DataType>,
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
        reason = reason.as_str()
    );
    let start = Instant::now();

    match process_block(events, storage, consumer, property_types)
        .instrument(span)
        .await
    {
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
    property_types: &mut HashMap<uuid::Uuid, models::properties::DataType>,
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
            let result = handlers::edits::handle_edit(&edit, property_types)?;

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
                + result.properties.len()
                + set_values.len()
                + delete_value_ids.len()
                + set_relations.len()
                + update_relations.len()
                + unset_relations.len()
                + delete_relations.len();

            // Bulk insert all operations
            storage.insert_entities(&result.entities, &mut tx).await?;
            storage
                .insert_properties(&result.properties, &mut tx)
                .await?;
            storage.insert_values(&set_values, &mut tx).await?;
            storage.delete_values(&delete_value_ids, &mut tx).await?;
            storage.insert_relations(&set_relations, &mut tx).await?;
            storage.update_relations(&update_relations, &mut tx).await?;
            storage
                .unset_relation_fields(&unset_relations, &mut tx)
                .await?;
            storage.delete_relations(&delete_relations, &mut tx).await?;

            ops
        }
        KgMessage::CreateSpace(space) => {
            let space_item = handlers::spaces::handle_create_space(&space)?;
            storage.insert_spaces(&[space_item], &mut tx).await?;
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
            if let Some(subspace) = handlers::subspaces::handle_trust_extension(&event)? {
                storage.insert_subspaces(&[subspace], &mut tx).await?;
                1
            } else {
                0
            }
        }
        KgMessage::ProposalCreated(event) => {
            let result = handlers::governance::handle_proposal_created(&event)?;
            debug!(
                proposal_id = %result.proposal.id,
                actions = result.actions.len(),
                "Processing ProposalCreated"
            );
            storage
                .insert_proposals(&[result.proposal], &mut tx)
                .await?;
            if !result.actions.is_empty() {
                storage
                    .insert_proposal_actions(&result.actions, &mut tx)
                    .await?;
            }
            1 + result.actions.len()
        }
        KgMessage::ProposalUpdated(event) => {
            let result = handlers::governance::handle_proposal_updated(&event)?;
            debug!(
                proposal_id = %result.proposal.id,
                actions = result.actions.len(),
                "Processing ProposalUpdated"
            );
            storage.update_proposal(&result.proposal, &mut tx).await?;
            storage
                .delete_proposal_actions(result.proposal.id, &mut tx)
                .await?;
            if !result.actions.is_empty() {
                storage
                    .insert_proposal_actions(&result.actions, &mut tx)
                    .await?;
            }
            1 + result.actions.len()
        }
        KgMessage::ProposalVoted(event) => {
            let vote = handlers::governance::handle_proposal_voted(&event)?;
            debug!(
                proposal_id = %vote.proposal_id,
                voter_id = %vote.voter_id,
                "Processing ProposalVoted"
            );
            storage.insert_proposal_votes(&[vote], &mut tx).await?;
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
    property_types: &mut HashMap<uuid::Uuid, models::properties::DataType>,
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

    // Process each message in sequence order
    for event in &events {
        let ops = async {
            Ok::<usize, IndexerError>(match &event.msg {
                KgMessage::Edit(edit) => {
                    let result = handlers::edits::handle_edit(edit, property_types)?;

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
                        + result.properties.len()
                        + set_values.len()
                        + delete_value_ids.len()
                        + set_relations.len()
                        + update_relations.len()
                        + unset_relations.len()
                        + delete_relations.len();

                    // Bulk insert all operations
                    storage.insert_entities(&result.entities, &mut tx).await?;
                    storage
                        .insert_properties(&result.properties, &mut tx)
                        .await?;
                    storage.insert_values(&set_values, &mut tx).await?;
                    storage.delete_values(&delete_value_ids, &mut tx).await?;
                    storage.insert_relations(&set_relations, &mut tx).await?;
                    storage.update_relations(&update_relations, &mut tx).await?;
                    storage
                        .unset_relation_fields(&unset_relations, &mut tx)
                        .await?;
                    storage.delete_relations(&delete_relations, &mut tx).await?;

                    ops
                }
                KgMessage::CreateSpace(space) => {
                    let space_item = handlers::spaces::handle_create_space(space)?;
                    storage.insert_spaces(&[space_item], &mut tx).await?;
                    1
                }
                KgMessage::RoleGranted(event) => {
                    match handlers::membership::handle_role_granted(event)? {
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
                KgMessage::RoleRevoked(event) => {
                    match handlers::membership::handle_role_revoked(event)? {
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
                KgMessage::TrustExtension(event) => {
                    if let Some(subspace) = handlers::subspaces::handle_trust_extension(event)? {
                        storage.insert_subspaces(&[subspace], &mut tx).await?;
                        1
                    } else {
                        0
                    }
                }
                KgMessage::ProposalCreated(event) => {
                    let result = handlers::governance::handle_proposal_created(event)?;
                    debug!(
                        proposal_id = %result.proposal.id,
                        actions = result.actions.len(),
                        "Processing ProposalCreated"
                    );
                    storage
                        .insert_proposals(&[result.proposal], &mut tx)
                        .await?;
                    if !result.actions.is_empty() {
                        storage
                            .insert_proposal_actions(&result.actions, &mut tx)
                            .await?;
                    }
                    1 + result.actions.len()
                }
                KgMessage::ProposalUpdated(event) => {
                    let result = handlers::governance::handle_proposal_updated(event)?;
                    debug!(
                        proposal_id = %result.proposal.id,
                        actions = result.actions.len(),
                        "Processing ProposalUpdated"
                    );
                    storage.update_proposal(&result.proposal, &mut tx).await?;
                    storage
                        .delete_proposal_actions(result.proposal.id, &mut tx)
                        .await?;
                    if !result.actions.is_empty() {
                        storage
                            .insert_proposal_actions(&result.actions, &mut tx)
                            .await?;
                    }
                    1 + result.actions.len()
                }
                KgMessage::ProposalVoted(event) => {
                    let vote = handlers::governance::handle_proposal_voted(event)?;
                    debug!(
                        proposal_id = %vote.proposal_id,
                        voter_id = %vote.voter_id,
                        "Processing ProposalVoted"
                    );
                    storage.insert_proposal_votes(&[vote], &mut tx).await?;
                    1
                }
                KgMessage::ProposalExecuted(event) => {
                    let execution = handlers::governance::handle_proposal_executed(event)?;
                    debug!(
                        proposal_id = %execution.proposal_id,
                        "Processing ProposalExecuted"
                    );
                    storage
                        .update_proposal_executed(
                            execution.proposal_id,
                            execution.executed_at,
                            &mut tx,
                        )
                        .await?;
                    1
                }
                KgMessage::BlockSummary(_) => 0,
            })
        }
        .await
        .map_err(|e| {
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
    tx.commit().await?;
    let db_tx_duration_ms = tx_start.elapsed().as_millis();

    // Commit Kafka offsets for all processed messages
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

    Ok(ProcessBlockResult {
        ops: total_ops,
        commit_failures,
        db_tx_duration_ms,
    })
}
