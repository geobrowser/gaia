//! Vote indexer entry point.
//!
//! Consumes vote events from the `curation.votes` Kafka topic and indexes them
//! into PostgreSQL with real-time aggregation.

use std::collections::HashMap;
use std::env;

use futures::StreamExt;
use rdkafka::message::Message;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use vote_indexer::consumer::{parse_vote, KafkaConsumer};
use vote_indexer::error::IndexerError;
use vote_indexer::handlers::voting::{calculate_vote_counts, get_latest_user_votes, handle_vote_cast};
use vote_indexer::models::voting::{UserVoteCriteria, VoteCountCriteria, VoteItem};
use vote_indexer::storage::Storage;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer())
        .init();

    info!("Starting vote-indexer");

    // Load configuration from environment
    let database_url =
        env::var("DATABASE_URL").map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
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
    let mut batch_timer = tokio::time::interval(tokio::time::Duration::from_millis(batch_timeout_ms));
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

    // Fetch existing user votes and vote counts
    let stored_user_votes = storage.get_user_votes(&user_vote_criteria).await?;
    let stored_vote_counts = storage.get_votes_counts(&vote_count_criteria).await?;

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

    // Execute all writes in a transaction
    let mut tx = storage.pool().begin().await?;

    // Insert raw votes (audit log)
    storage.insert_votes(votes, &mut tx).await?;

    // Upsert user votes (current state)
    storage.upsert_user_votes(&user_votes, &mut tx).await?;

    // Upsert vote counts (aggregates)
    storage.upsert_votes_counts(&updated_vote_counts, &mut tx).await?;

    tx.commit().await?;

    debug!(
        raw_votes = vote_count,
        user_votes = user_votes.len(),
        vote_counts = updated_vote_counts.len(),
        "Processed vote batch"
    );

    Ok(vote_count)
}

/// Commit all offsets in the batch.
fn commit_offsets(consumer: &KafkaConsumer, commit_info: &[(String, i32, i64)]) {
    for (topic, partition, offset) in commit_info {
        if let Err(e) = consumer.commit_message(topic, *partition, *offset) {
            error!(error = %e, topic = %topic, partition = partition, offset = offset, "Failed to commit offset");
        }
    }
}
