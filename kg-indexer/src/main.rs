use std::env;

use futures::StreamExt;
use rdkafka::Message;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod consumer;
mod error;
mod handlers;
mod models;
mod storage;

use consumer::{KafkaConsumer, KgMessage, parse_message};
use error::IndexerError;
use storage::Storage;

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

    info!("Starting message processing loop");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down...");
                break;
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
                                    match process_message(kg_msg, &storage).await {
                                        Ok(ops) => {
                                            debug!(
                                                topic = %topic,
                                                partition = partition,
                                                offset = offset,
                                                ops = ops,
                                                "Processed message"
                                            );
                                            processed_count += 1;

                                            // Commit offset after successful processing
                                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                                error!(error = %e, "Failed to commit offset");
                                            }
                                        }
                                        Err(e) => {
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
                                        }
                                    }

                                    if processed_count % 100 == 0 && processed_count > 0 {
                                        info!(
                                            processed = processed_count,
                                            errors = error_count,
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

/// Process a single Kafka message within its own transaction.
/// Returns the number of database operations performed.
async fn process_message(msg: KgMessage, storage: &Storage) -> Result<usize, IndexerError> {
    use handlers::membership::MembershipChange;
    use models::relations::RelationOp;
    use models::values::ValueChangeType;

    let mut tx = storage.pool.begin().await?;

    let ops = match msg {
        KgMessage::Edit(edit) => {
            let result = handlers::edits::handle_edit(&edit)?;

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
            storage.insert_properties(&result.properties, &mut tx).await?;
            storage.insert_values(&set_values, &mut tx).await?;
            storage.delete_values(&delete_value_ids, &mut tx).await?;
            storage.insert_relations(&set_relations, &mut tx).await?;
            storage.update_relations(&update_relations, &mut tx).await?;
            storage.unset_relation_fields(&unset_relations, &mut tx).await?;
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
    };

    tx.commit().await?;

    Ok(ops)
}
