//! ranking-indexer binary entrypoint.
//!
//! Consumes `knowledge.edits`, keeps the rank-relevant ops, and upserts them
//! into the private `ranks` working schema. Per-edit processing (the design
//! allows either per-edit or per-block batching, §10).
//!
//! NOT YET WIRED (follow-ups, gated on the design's open questions):
//!   - `detect()` op extraction (the rank op-pattern matching)
//!   - per-block recompute: dedup -> eligibility -> scoring
//!   - projection of `RANK_POSITION` relations into `public.relations`

use std::collections::HashMap;
use std::env;

use futures::StreamExt;
use rdkafka::Message;
use tracing::{error, info, warn};
use uuid::Uuid;

use ranking_indexer::consumer::{decode_grc20, parse_edit, KafkaConsumer};
use ranking_indexer::detect::detect;
use ranking_indexer::error::IndexerError;
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
    let detected = detect(&grc20_edit, space_id);
    if detected.is_empty() {
        return Ok(());
    }

    for block in &detected.blocks {
        storage.upsert_ranking_block(block).await?;
    }
    for ranking in &detected.rankings {
        storage.upsert_ranking(ranking).await?;
    }

    // Re-submission rebuilds a rank's items, so replace per ranking.
    let mut items_by_ranking: HashMap<Uuid, Vec<_>> = HashMap::new();
    for item in detected.items {
        items_by_ranking.entry(item.ranking_id).or_default().push(item);
    }
    for (ranking_id, items) in items_by_ranking {
        storage.replace_ranking_items(ranking_id, &items).await?;
    }

    for (ranking_id, block_id) in detected.block_links {
        storage.set_ranking_block(ranking_id, block_id).await?;
    }

    // TODO(ranking-indexer): recompute affected blocks (dedup -> eligibility ->
    // scoring) and project RANK_POSITION relations. Gated on the design's open
    // questions (eligibility for personal spaces, scoring normalization, the
    // public-projection / atlas coordination).

    Ok(())
}
