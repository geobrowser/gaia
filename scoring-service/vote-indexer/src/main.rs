//! Vote indexer entry point.
//!
//! Consumes vote events from the `curation.votes` Kafka topic and indexes them
//! into PostgreSQL with real-time aggregation.

use std::env;

use futures::StreamExt;
use hermes_instrumentation::{debug, error, info, warn};
use rdkafka::message::Message;

use vote_indexer::batch::process_vote_batch;
use vote_indexer::consumer::{parse_vote, KafkaConsumer};
use vote_indexer::error::IndexerError;
use vote_indexer::handlers::voting::handle_vote_cast;
use vote_indexer::metrics::{self, RejectReason};
use vote_indexer::models::voting::VoteItem;
use vote_indexer::storage::Storage;
use vote_indexer::write_retry::{self, BatchOutcome, RetryPolicy, SkipGuard};

fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    info!("Starting vote-indexer");

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

    Config::new("vote-indexer", backend)
}

async fn async_main() -> Result<(), IndexerError> {
    // Prometheus /metrics on 9464 (hermes_instrumentation::metrics::DEFAULT_PORT),
    // the port the ServiceMonitor scrapes; override with METRICS_PORT for local runs.
    let metrics_port: Option<u16> = env::var("METRICS_PORT").ok().and_then(|s| s.parse().ok());
    hermes_instrumentation::metrics::install("vote-indexer", metrics_port)
        .map_err(|e| IndexerError::Config(format!("metrics install failed: {}", e)))?;
    metrics::register();

    // Load configuration from environment
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let kafka_group_id = env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "vote-indexer".to_string());
    let batch_size: usize = env::var("BATCH_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let batch_timeout_ms: u64 = env::var("BATCH_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    // Initialize storage
    let storage = Storage::connect(&database_url).await?;
    info!("Connected to database");
    storage.mirror_votes_counts_snapshot().await;

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

    // Main processing loop with batching
    let mut stream = consumer.stream();
    let mut vote_buffer: Vec<VoteItem> = Vec::with_capacity(batch_size);
    // Every message read since the last flush, in order, including ones rejected
    // as malformed: committing a rejected message's offset at once would move its
    // partition past votes still in the buffer, so a batch that then halted would
    // be skipped by the restart anyway (GEO-3101).
    let mut commit_info: Vec<PendingOffset> = Vec::with_capacity(batch_size);
    let policy = RetryPolicy::from_env();
    let mut skip_guard = SkipGuard::from_env();
    let mut batch_timer =
        tokio::time::interval(tokio::time::Duration::from_millis(batch_timeout_ms));
    let mut processed_count: u64 = 0;
    let mut error_count: u64 = 0;

    info!(
        batch_size = batch_size,
        batch_timeout_ms = batch_timeout_ms,
        "Starting message processing loop"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down...");
                // Process any remaining votes before shutdown
                if !commit_info.is_empty() {
                    flush_batch(
                        &vote_buffer,
                        &commit_info,
                        &storage,
                        &consumer,
                        &policy,
                        &mut skip_guard,
                        &mut processed_count,
                        &mut error_count,
                    )
                    .await;
                }
                break;
            }

            _ = batch_timer.tick() => {
                // Process batch on timeout if we have votes (or rejected messages
                // whose offsets are waiting behind them)
                if !commit_info.is_empty() {
                    debug!(count = vote_buffer.len(), "Processing batch on timeout");
                    flush_batch(
                        &vote_buffer,
                        &commit_info,
                        &storage,
                        &consumer,
                        &policy,
                        &mut skip_guard,
                        &mut processed_count,
                        &mut error_count,
                    )
                    .await;
                    vote_buffer.clear();
                    commit_info.clear();
                }
            }

            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();

                        if let Some(payload) = msg.payload() {
                            match parse_vote(payload) {
                                Ok(vote_msg) => {
                                    match handle_vote_cast(&vote_msg) {
                                        Ok(vote_item) => {
                                            commit_info.push(PendingOffset {
                                                topic,
                                                partition,
                                                offset,
                                                vote: Some(vote_buffer.len()),
                                            });
                                            vote_buffer.push(vote_item);

                                            // Process batch if full
                                            if vote_buffer.len() >= batch_size {
                                                debug!(count = vote_buffer.len(), "Processing full batch");
                                                flush_batch(
                                                    &vote_buffer,
                                                    &commit_info,
                                                    &storage,
                                                    &consumer,
                                                    &policy,
                                                    &mut skip_guard,
                                                    &mut processed_count,
                                                    &mut error_count,
                                                )
                                                .await;
                                                vote_buffer.clear();
                                                commit_info.clear();
                                            }
                                        }
                                        Err(e) => {
                                            warn!(
                                                error = %e,
                                                partition = partition,
                                                offset = offset,
                                                "Failed to handle vote message"
                                            );
                                            error_count += 1;
                                            metrics::message_rejected(RejectReason::Invalid);
                                            // Committed past with the next flush, in order.
                                            commit_info.push(PendingOffset { topic, partition, offset, vote: None });
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        error = %e,
                                        partition = partition,
                                        offset = offset,
                                        "Failed to parse vote message"
                                    );
                                    error_count += 1;
                                    metrics::message_rejected(RejectReason::Undecodable);
                                    // Committed past with the next flush, in order.
                                    commit_info.push(PendingOffset { topic, partition, offset, vote: None });
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

/// One message read since the last flush, and the vote it became, if any.
struct PendingOffset {
    topic: String,
    partition: i32,
    offset: i64,
    /// Index into the batch's votes; `None` for a message rejected as malformed.
    vote: Option<usize>,
}

/// Write a batch, then commit its offsets.
///
/// The batch is one transaction. A transient failure is retried with backoff
/// (`write_retry`); if it persists, nothing is committed and the process halts,
/// so the restart re-reads the batch (GEO-3101). A permanent failure means some
/// vote in it can never be written: the votes are then written one at a time so
/// only that vote is skipped, counted in `vote_indexer_votes_dropped_total`.
#[allow(clippy::too_many_arguments)]
async fn flush_batch(
    votes: &[VoteItem],
    commit_info: &[PendingOffset],
    storage: &Storage,
    consumer: &KafkaConsumer,
    policy: &RetryPolicy,
    skip_guard: &mut SkipGuard,
    processed_count: &mut u64,
    error_count: &mut u64,
) {
    let outcome = write_retry::write_batch(
        votes.len(),
        policy,
        IndexerError::class,
        skip_guard,
        |attempt, e, delay| {
            warn!(
                error = %e,
                attempt = attempt,
                max_attempts = policy.max_attempts,
                delay_ms = delay.as_millis() as u64,
                "Vote batch failed transiently — retrying"
            );
            metrics::write_retried();
        },
        true,
        |range| async move { process_vote_batch(&votes[range], storage).await.map(|_| ()) },
    )
    .await;

    match outcome {
        BatchOutcome::Done(report) => {
            *processed_count += report.written as u64;
            metrics::votes_processed(report.written as u64);
            for (index, e) in &report.skipped {
                error!(
                    event = "vote_indexer.vote_dropped",
                    error = %e,
                    voter_id = %votes[*index].voter_id,
                    object_id = %votes[*index].object_id,
                    "Vote dropped after a PERMANENT write failure"
                );
            }
            *error_count += report.skipped.len() as u64;
            metrics::votes_dropped(report.skipped.len() as u64);
            commit_offsets(consumer, commit_info);
        }
        BatchOutcome::Halt {
            index,
            error,
            attempts,
            report,
        } => {
            metrics::votes_processed(report.written as u64);
            metrics::votes_dropped(report.skipped.len() as u64);
            // Everything read before the vote that failed is finished; commit it so
            // the restart does not write those votes a second time.
            let finished = commit_info
                .iter()
                .position(|p| p.vote == Some(index))
                .unwrap_or(0);
            commit_offsets(consumer, &commit_info[..finished]);
            error!(
                event = "vote_indexer.halting",
                error = %error,
                attempts = attempts,
                consecutive_skips = skip_guard.consecutive(),
                uncommitted = commit_info.len() - finished,
                "Vote write failed on every attempt (or too many in a row failed \
                 permanently) — halting WITHOUT committing so the restart re-reads it"
            );
            metrics::halted();
            write_retry::halt().await
        }
    }
}

/// Commit the given offsets, in order.
fn commit_offsets(consumer: &KafkaConsumer, commit_info: &[PendingOffset]) {
    for PendingOffset {
        topic,
        partition,
        offset,
        ..
    } in commit_info
    {
        if let Err(e) = consumer.commit_message(topic, *partition, *offset) {
            error!(error = %e, topic = %topic, partition = partition, offset = offset, "Failed to commit offset");
        }
    }
}
