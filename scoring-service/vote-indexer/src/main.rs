//! Vote indexer entry point.
//!
//! Consumes vote events from the `curation.votes` Kafka topic and indexes them
//! into PostgreSQL with real-time aggregation.

use std::collections::HashMap;
use std::env;

use futures::StreamExt;
use hermes_instrumentation::{debug, error, info, info_span, warn, Instrument};
use rdkafka::message::Message;

use vote_indexer::consumer::{parse_vote, KafkaConsumer};
use vote_indexer::error::IndexerError;
use vote_indexer::handlers::voting::{
    build_score_values, calculate_vote_counts, get_latest_user_votes, handle_vote_cast,
};
use vote_indexer::models::voting::{UserVoteCriteria, VoteCountCriteria, VoteItem};
use vote_indexer::storage::Storage;

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
    let mut commit_info: Vec<(String, i32, i64)> = Vec::with_capacity(batch_size);
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
                if !vote_buffer.is_empty() {
                    match process_vote_batch(&vote_buffer, &storage).await {
                        Ok(count) => {
                            processed_count += count as u64;
                            commit_offsets(&consumer, &commit_info);
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to process final batch");
                            error_count += vote_buffer.len() as u64;
                        }
                    }
                }
                break;
            }

            _ = batch_timer.tick() => {
                // Process batch on timeout if we have votes
                if !vote_buffer.is_empty() {
                    debug!(count = vote_buffer.len(), "Processing batch on timeout");
                    match process_vote_batch(&vote_buffer, &storage).await {
                        Ok(count) => {
                            processed_count += count as u64;
                            commit_offsets(&consumer, &commit_info);
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to process batch");
                            error_count += vote_buffer.len() as u64;
                        }
                    }
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
                                            vote_buffer.push(vote_item);
                                            commit_info.push((topic, partition, offset));

                                            // Process batch if full
                                            if vote_buffer.len() >= batch_size {
                                                debug!(count = vote_buffer.len(), "Processing full batch");
                                                match process_vote_batch(&vote_buffer, &storage).await {
                                                    Ok(count) => {
                                                        processed_count += count as u64;
                                                        commit_offsets(&consumer, &commit_info);
                                                    }
                                                    Err(e) => {
                                                        error!(error = %e, "Failed to process batch");
                                                        error_count += vote_buffer.len() as u64;
                                                    }
                                                }
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
                                            // Commit to avoid getting stuck
                                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                                error!(error = %e, "Failed to commit offset");
                                            }
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
                                    // Commit to avoid getting stuck
                                    if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                        error!(error = %e, "Failed to commit offset");
                                    }
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

/// Process a batch of votes within a single transaction.
async fn process_vote_batch(votes: &[VoteItem], storage: &Storage) -> Result<usize, IndexerError> {
    if votes.is_empty() {
        return Ok(0);
    }

    let span = info_span!(
        "vote_indexer.process_batch",
        vote_count = votes.len(),
        user_votes = tracing::field::Empty,
        vote_counts = tracing::field::Empty,
    );

    async {
        let vote_count = votes.len();

        // Get deduplicated user votes from this batch
        let user_votes = get_latest_user_votes(votes);

        // Build criteria for fetching existing data
        let user_vote_criteria: Vec<UserVoteCriteria> = user_votes
            .iter()
            .map(|v| (v.voter_id, v.object_id, v.space_id, v.object_type))
            .collect();

        let vote_count_criteria: Vec<VoteCountCriteria> = user_votes
            .iter()
            .map(|v| (v.object_id, v.space_id, v.object_type))
            .collect();

        // Start transaction before reads to ensure consistency.
        // Reads use FOR UPDATE to lock rows and prevent concurrent modifications.
        let mut tx = storage.pool().begin().await?;

        // Fetch existing user votes and vote counts (with row locks)
        let stored_user_votes = storage
            .get_user_votes_tx(&user_vote_criteria, &mut tx)
            .await?;
        let stored_vote_counts = storage
            .get_votes_counts_tx(&vote_count_criteria, &mut tx)
            .await?;

        // Convert to HashMaps for lookup
        let stored_user_votes_map: HashMap<UserVoteCriteria, _> = stored_user_votes
            .into_iter()
            .map(|v| ((v.voter_id, v.object_id, v.space_id, v.object_type), v))
            .collect();

        let stored_vote_counts_map: HashMap<VoteCountCriteria, _> = stored_vote_counts
            .into_iter()
            .map(|v| ((v.object_id, v.space_id, v.object_type), v))
            .collect();

        // Calculate updated vote counts
        let updated_vote_counts =
            calculate_vote_counts(&user_votes, &stored_user_votes_map, &stored_vote_counts_map);

        // Record computed values in current span
        use tracing::Span;
        Span::current().record("user_votes", user_votes.len());
        Span::current().record("vote_counts", updated_vote_counts.len());

        // Insert raw votes (audit log)
        storage.insert_votes(votes, &mut tx).await?;

        // Upsert user votes (current state)
        storage.upsert_user_votes(&user_votes, &mut tx).await?;

        // Upsert vote counts (aggregates)
        storage
            .upsert_votes_counts(&updated_vote_counts, &mut tx)
            .await?;

        // Mirror entity net scores into `values` under the Score system property
        // so `entities_ordered_by_property` can sort by raw score with no SQL changes.
        let score_values = build_score_values(&updated_vote_counts);
        storage.upsert_score_values(&score_values, &mut tx).await?;

        tx.commit().await?;

        debug!(
            raw_votes = vote_count,
            user_votes = user_votes.len(),
            vote_counts = updated_vote_counts.len(),
            "Processed vote batch"
        );

        Ok(vote_count)
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
