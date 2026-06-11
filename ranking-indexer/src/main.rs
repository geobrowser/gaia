//! ranking-indexer binary entrypoint.
//!
//! Consumes `knowledge.edits`, keeps the rank-relevant ops, and runs the full
//! pipeline per edit (decode -> detect -> upsert -> recompute -> publish). The
//! design allows either per-edit or per-block batching (§10); this uses per-edit.

use std::env;

use futures::StreamExt;
use rdkafka::Message;
use tracing::{error, info, warn};
use uuid::Uuid;

use ranking_indexer::consumer::{decode_grc20, parse_edit, KafkaConsumer};
use ranking_indexer::detect::detect;
use ranking_indexer::error::IndexerError;
use ranking_indexer::recompute;
use ranking_indexer::storage::Storage;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let brokers = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".into());
    let group_id = env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "ranking-indexer".into());

    let storage = Storage::new(&database_url).await?;
    let consumer = KafkaConsumer::new(&brokers, &group_id)?;
    consumer.subscribe()?;

    info!("ranking-indexer started");

    let mut stream = consumer.stream();
    while let Some(result) = stream.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                error!(error = %e, "Kafka receive error");
                continue;
            }
        };

        let topic = msg.topic().to_string();
        let partition = msg.partition();
        let offset = msg.offset();

        let Some(payload) = msg.payload() else {
            // Tombstone / empty payload — nothing to do, advance past it.
            let _ = consumer.commit_message(&topic, partition, offset);
            continue;
        };

        match process_edit(payload, &storage).await {
            Ok(()) => {
                if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                    error!(error = %e, "Failed to commit offset");
                }
            }
            Err(e) => {
                // A malformed edit is logged and skipped rather than stalling the
                // partition (design §10). The offset is still advanced.
                warn!(error = %e, offset = offset, "Skipping unprocessable edit");
                let _ = consumer.commit_message(&topic, partition, offset);
            }
        }
    }

    Ok(())
}

/// Decode one edit and upsert its rank-relevant ops into the `ranks` schema.
async fn process_edit(payload: &[u8], storage: &Storage) -> Result<(), IndexerError> {
    let edit = parse_edit(payload)?;
    let space_id = Uuid::from_slice(&edit.space_id)
        .map_err(|e| IndexerError::decode(format!("space_id: {e}")))?;

    let grc20_edit = decode_grc20(&edit.payload)?;
    let (block_number, block_timestamp) = edit
        .meta
        .as_ref()
        .map(|m| (m.block_number as i64, m.created_at as i64))
        .unwrap_or((0, 0));
    let detected = detect(&grc20_edit, space_id, block_number, block_timestamp);
    recompute::apply_detected_edit(&detected, space_id, storage).await
}
