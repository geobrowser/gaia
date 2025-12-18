use std::env;

use futures::StreamExt;
use rdkafka::Message;
use tracing::{error, info, warn};
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
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::config("DATABASE_URL not set"))?;
    let kafka_broker = env::var("KAFKA_BROKER")
        .unwrap_or_else(|_| "localhost:9092".to_string());
    let kafka_group_id = env::var("KAFKA_GROUP_ID")
        .unwrap_or_else(|_| "kg-indexer".to_string());

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
                        let topic = msg.topic();
                        let partition = msg.partition();
                        let offset = msg.offset();

                        if let Some(payload) = msg.payload() {
                            match parse_message(topic, payload) {
                                Ok(kg_msg) => {
                                    let result = process_message(kg_msg, &storage).await;

                                    if let Err(e) = result {
                                        error!(
                                            topic = %topic,
                                            partition = partition,
                                            offset = offset,
                                            error = %e,
                                            "Failed to process message"
                                        );
                                        error_count += 1;
                                    } else {
                                        processed_count += 1;

                                        if processed_count % 100 == 0 {
                                            info!(
                                                processed = processed_count,
                                                errors = error_count,
                                                "Progress update"
                                            );
                                        }
                                    }

                                    // Commit offset after processing (success or failure)
                                    if let Err(e) = consumer.commit_message(topic, partition, offset) {
                                        error!(error = %e, "Failed to commit offset");
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
                                    if let Err(e) = consumer.commit_message(topic, partition, offset) {
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

async fn process_message(msg: KgMessage, storage: &Storage) -> Result<(), IndexerError> {
    match msg {
        KgMessage::Edit(edit) => {
            handlers::edits::handle_edit(&edit, storage).await
        }
        KgMessage::CreateSpace(space) => {
            handlers::spaces::handle_create_space(&space, storage).await
        }
        KgMessage::RoleGranted(event) => {
            handlers::membership::handle_role_granted(&event, storage).await
        }
        KgMessage::RoleRevoked(event) => {
            handlers::membership::handle_role_revoked(&event, storage).await
        }
        KgMessage::TrustExtension(event) => {
            handlers::subspaces::handle_trust_extension(&event, storage).await
        }
    }
}
