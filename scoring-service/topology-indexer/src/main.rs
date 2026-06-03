//! Topology indexer entry point.
//!
//! Consumes `CanonicalGraphDiff` messages from the `topology.canonical` Kafka topic
//! and persists space distances to PostgreSQL.

use std::env;

use futures::StreamExt;
use hermes_instrumentation::{debug, error, info, info_span, warn, Instrument};
use rdkafka::message::Message;

use topology_indexer::consumer::{parse_diff, KafkaConsumer, ParsedDiff};
use topology_indexer::error::IndexerError;
use topology_indexer::storage::Storage;

fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    info!("Starting topology-indexer");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| IndexerError::Config(format!("Failed to build tokio runtime: {}", e)))?
        .block_on(async_main())
}

/// Build telemetry configuration from environment variables.
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

    Config::new("topology-indexer", backend)
}

async fn async_main() -> Result<(), IndexerError> {
    // Load configuration from environment
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let kafka_group_id =
        env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "topology-indexer".to_string());
    let batch_size: usize = env::var("BATCH_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let batch_timeout_ms: u64 = env::var("BATCH_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    // Initialize storage
    let storage = Storage::connect(&database_url).await?;
    info!("Connected to database");

    // Initialize Kafka consumer
    let consumer = KafkaConsumer::new(&kafka_broker, &kafka_group_id)?;
    consumer.subscribe()?;

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    let _signal_handle = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Main processing loop with batching
    let mut stream = consumer.stream();
    let mut diff_buffer: Vec<ParsedDiff> = Vec::with_capacity(batch_size);
    let mut commit_info: Vec<(String, i32, i64)> = Vec::with_capacity(batch_size);
    let mut batch_timer =
        tokio::time::interval(tokio::time::Duration::from_millis(batch_timeout_ms));
    let mut processed_count: u64 = 0;
    let mut error_count: u64 = 0;
    let mut heartbeat_timer = tokio::time::interval(tokio::time::Duration::from_secs(60));
    heartbeat_timer.tick().await; // skip immediate first tick

    info!(
        batch_size = batch_size,
        batch_timeout_ms = batch_timeout_ms,
        "Starting message processing loop"
    );

    loop {
        tokio::select! {
            _ = heartbeat_timer.tick() => {
                info!(
                    processed = processed_count,
                    errors = error_count,
                    pending = diff_buffer.len(),
                    "Heartbeat"
                );
            }
            _ = shutdown_rx.recv() => {
                info!("Shutting down...");
                if !diff_buffer.is_empty() {
                    match process_diff_batch(&diff_buffer, &storage).await {
                        Ok(count) => {
                            processed_count += count as u64;
                            commit_offsets(&consumer, &commit_info);
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to process final batch");
                            error_count += diff_buffer.len() as u64;
                        }
                    }
                }
                break;
            }

            _ = batch_timer.tick() => {
                if !diff_buffer.is_empty() {
                    debug!(count = diff_buffer.len(), "Processing batch on timeout");
                    match process_diff_batch(&diff_buffer, &storage).await {
                        Ok(count) => {
                            processed_count += count as u64;
                            commit_offsets(&consumer, &commit_info);
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to process batch");
                            error_count += diff_buffer.len() as u64;
                        }
                    }
                    diff_buffer.clear();
                    commit_info.clear();
                }
            }

            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();

                        let Some(payload) = msg.payload() else {
                            // Null payload (tombstone) — commit and skip
                            debug!(partition = partition, offset = offset, "Skipping null payload");
                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                error!(error = %e, "Failed to commit offset");
                            }
                            continue;
                        };

                        match parse_diff(payload) {
                            Ok(Some(parsed)) => {
                                debug!(
                                    root_id = %parsed.root_id,
                                    changes = parsed.changes.len(),
                                    partition = partition,
                                    offset = offset,
                                    "Parsed topology diff"
                                );
                                diff_buffer.push(parsed);
                                commit_info.push((topic, partition, offset));

                                if diff_buffer.len() >= batch_size {
                                    debug!(count = diff_buffer.len(), "Processing full batch");
                                    match process_diff_batch(&diff_buffer, &storage).await {
                                        Ok(count) => {
                                            processed_count += count as u64;
                                            commit_offsets(&consumer, &commit_info);
                                        }
                                        Err(e) => {
                                            error!(error = %e, "Failed to process batch");
                                            error_count += diff_buffer.len() as u64;
                                        }
                                    }
                                    diff_buffer.clear();
                                    commit_info.clear();
                                }
                            }
                            Ok(None) => {
                                // Empty diff or invalid root_id — commit and skip
                                if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                    error!(error = %e, "Failed to commit offset");
                                }
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    partition = partition,
                                    offset = offset,
                                    "Failed to parse topology diff message"
                                );
                                error_count += 1;
                                if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                    error!(error = %e, "Failed to commit offset");
                                }
                            }
                        }
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

/// Process a batch of parsed diffs by applying changes to PostgreSQL.
async fn process_diff_batch(
    diffs: &[ParsedDiff],
    storage: &Storage,
) -> Result<usize, IndexerError> {
    if diffs.is_empty() {
        return Ok(0);
    }

    let total_changes: usize = diffs.iter().map(|d| d.changes.len()).sum();

    let span = info_span!(
        "topology_indexer.process_batch",
        diff_count = diffs.len(),
        total_changes = total_changes,
    );

    async {
        for diff in diffs {
            storage.apply_changes(diff.root_id, &diff.changes).await?;
        }

        debug!(
            diff_count = diffs.len(),
            total_changes = total_changes,
            "Processed topology batch"
        );

        Ok(diffs.len())
    }
    .instrument(span)
    .await
}

/// Commit all offsets in the batch.
fn commit_offsets(consumer: &KafkaConsumer, commit_info: &[(String, i32, i64)]) {
    for (topic, partition, offset) in commit_info {
        if let Err(e) = consumer.commit_message(topic, *partition, *offset) {
            error!(error = %e, topic = %topic, partition = partition, offset = offset, "Failed to commit offset");
        }
    }
}
