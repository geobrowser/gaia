use std::env;
use std::time::{Duration, Instant};

use futures::StreamExt;
use rdkafka::Message;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod batch;
mod consumer;
mod error;
mod handlers;
mod models;
mod storage;

use batch::Batch;
use consumer::{KafkaConsumer, KgMessage, parse_message};
use error::IndexerError;
use storage::Storage;

/// Maximum number of operations to accumulate before flushing
const BATCH_SIZE_THRESHOLD: usize = 1000;

/// Maximum time to wait before flushing a batch
const BATCH_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting kg-indexer");

    // Load configuration from environment
    let database_url =
        env::var("DATABASE_URL").map_err(|_| IndexerError::config("DATABASE_URL not set"))?;
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let kafka_group_id = env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "kg-indexer".to_string());

    // Initialize storage
    let storage = Storage::new(&database_url).await?;
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

    // Main processing loop
    let mut stream = consumer.stream();
    let mut processed_count: u64 = 0;
    let mut error_count: u64 = 0;

    // Batch accumulator
    let mut batch = Batch::new();
    let mut last_flush = Instant::now();
    let mut pending_commits: Vec<(String, i32, i64)> = Vec::new();

    info!(
        batch_size_threshold = BATCH_SIZE_THRESHOLD,
        batch_timeout_secs = BATCH_TIMEOUT.as_secs(),
        "Starting message processing loop with batching"
    );

    loop {
        // Check if we should flush based on time
        let should_flush_time = !batch.is_empty() && last_flush.elapsed() >= BATCH_TIMEOUT;

        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down...");
                // Flush any remaining batch before shutdown
                if !batch.is_empty() {
                    if let Err(e) = batch.flush(&storage).await {
                        error!(error = %e, "Failed to flush final batch");
                    } else {
                        // Commit all pending offsets
                        for (topic, partition, offset) in &pending_commits {
                            if let Err(e) = consumer.commit_message(topic, *partition, *offset) {
                                error!(error = %e, "Failed to commit offset");
                            }
                        }
                    }
                }
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)), if should_flush_time => {
                // Time-based flush
                info!(batch_size = batch.len(), "Flushing batch due to timeout");
                if let Err(e) = batch.flush(&storage).await {
                    error!(error = %e, "Failed to flush batch");
                    error_count += pending_commits.len() as u64;
                } else {
                    processed_count += pending_commits.len() as u64;
                    // Commit all pending offsets
                    for (topic, partition, offset) in &pending_commits {
                        if let Err(e) = consumer.commit_message(topic, *partition, *offset) {
                            error!(error = %e, "Failed to commit offset");
                        }
                    }
                }
                pending_commits.clear();
                last_flush = Instant::now();
            }
            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();

                        if let Some(payload) = msg.payload() {
                            match parse_message(&topic, payload) {
                                Ok(kg_msg) => {
                                    let result = process_message(kg_msg, &mut batch);

                                    if let Err(e) = result {
                                        error!(
                                            topic = %topic,
                                            partition = partition,
                                            offset = offset,
                                            error = %e,
                                            "Failed to process message"
                                        );
                                        error_count += 1;
                                        // Still commit to avoid getting stuck
                                        if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                            error!(error = %e, "Failed to commit offset");
                                        }
                                    } else {
                                        // Track this message for commit after flush
                                        pending_commits.push((topic.clone(), partition, offset));

                                        // Check if we should flush based on size
                                        if batch.len() >= BATCH_SIZE_THRESHOLD {
                                            info!(batch_size = batch.len(), "Flushing batch due to size threshold");
                                            if let Err(e) = batch.flush(&storage).await {
                                                error!(error = %e, "Failed to flush batch");
                                                error_count += pending_commits.len() as u64;
                                            } else {
                                                processed_count += pending_commits.len() as u64;
                                                // Commit all pending offsets
                                                for (t, p, o) in &pending_commits {
                                                    if let Err(e) = consumer.commit_message(t, *p, *o) {
                                                        error!(error = %e, "Failed to commit offset");
                                                    }
                                                }
                                            }
                                            pending_commits.clear();
                                            last_flush = Instant::now();
                                        }
                                    }

                                    if processed_count % 100 == 0 && processed_count > 0 {
                                        info!(
                                            processed = processed_count,
                                            errors = error_count,
                                            pending = pending_commits.len(),
                                            "Progress update"
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        topic = %topic,
                                        partition = partition,
                                        offset = offset,
                                        error = %e,
                                        "Failed to parse message"
                                    );
                                    error_count += 1;

                                    // Still commit to avoid getting stuck
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
                        // Flush any remaining batch
                        if !batch.is_empty() {
                            if let Err(e) = batch.flush(&storage).await {
                                error!(error = %e, "Failed to flush final batch");
                            } else {
                                for (topic, partition, offset) in &pending_commits {
                                    if let Err(e) = consumer.commit_message(topic, *partition, *offset) {
                                        error!(error = %e, "Failed to commit offset");
                                    }
                                }
                            }
                        }
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

fn process_message(msg: KgMessage, batch: &mut Batch) -> Result<(), IndexerError> {
    use handlers::membership::MembershipChange;
    use models::relations::RelationOp;
    use models::values::ValueChangeType;

    match msg {
        KgMessage::Edit(edit) => {
            let result = handlers::edits::handle_edit(&edit)?;

            // Add entities and properties
            batch.entities.extend(result.entities);
            batch.properties.extend(result.properties);

            // Partition values into sets and deletes
            for op in result.values {
                match op.change_type {
                    ValueChangeType::Set => batch.set_values.push(op),
                    ValueChangeType::Delete => {
                        batch.delete_values.push((op.id, op.space_id));
                    }
                }
            }

            // Partition relations by operation type
            for op in result.relations {
                match op {
                    RelationOp::Create(r) => batch.set_relations.push(r),
                    RelationOp::Update(r) => batch.update_relations.push(r),
                    RelationOp::Unset(r) => batch.unset_relations.push(r),
                    RelationOp::Delete(r) => batch.delete_relations.push((r.id, r.space_id)),
                }
            }
        }
        KgMessage::CreateSpace(space) => {
            let space_item = handlers::spaces::handle_create_space(&space)?;
            batch.spaces.push(space_item);
        }
        KgMessage::RoleGranted(event) => {
            match handlers::membership::handle_role_granted(&event)? {
                MembershipChange::AddEditor(e) => batch.add_editors.push(e),
                MembershipChange::AddMember(m) => batch.add_members.push(m),
                _ => {} // Shouldn't happen for granted
            }
        }
        KgMessage::RoleRevoked(event) => {
            match handlers::membership::handle_role_revoked(&event)? {
                MembershipChange::RemoveEditor(e) => batch.remove_editors.push(e),
                MembershipChange::RemoveMember(m) => batch.remove_members.push(m),
                _ => {} // Shouldn't happen for revoked
            }
        }
        KgMessage::TrustExtension(event) => {
            if let Some(subspace) = handlers::subspaces::handle_trust_extension(&event)? {
                batch.add_subspaces.push(subspace);
            }
        }
    }

    Ok(())
}
