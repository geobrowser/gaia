//! ranking-indexer binary entrypoint.
//!
//! Consumes `knowledge.edits`, keeps the rank-relevant ops, and runs the full
//! pipeline per edit (decode -> detect -> upsert -> recompute -> publish). The
//! design allows either per-edit or per-block batching (§10); this uses per-edit.
//!
//! Also consumes `space.membership` to maintain the indexer's own view of the
//! space registry (`ranks.members` / `ranks.editors`) and recompute the blocks
//! a role grant/revoke affects.
//!
//! Error policy: poison messages (malformed input — retrying can never
//! succeed) are logged, skipped, and committed past, so one bad message never
//! stalls the partition. Transient errors (database, Kafka) are retried with
//! backoff; if they persist the process exits *without* committing, so Kafka
//! redelivers from the last committed offset on restart and the idempotent
//! writes converge. Transient failures are never skipped past — that would
//! silently lose an edit, or worse, a role revocation.

use std::env;
use std::time::Duration;

use futures::StreamExt;
use rdkafka::Message;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use ranking_indexer::consumer::{
    decode_grc20, get_event_type, parse_edit, parse_membership_event, KafkaConsumer, TopicKind,
};
use ranking_indexer::detect::detect;
use ranking_indexer::error::IndexerError;
use ranking_indexer::membership::apply_membership_event;
use ranking_indexer::recompute;
use ranking_indexer::storage::Storage;

/// Attempts per message for transient failures before the process exits to
/// force redelivery from the last committed offset.
const MAX_TRANSIENT_ATTEMPTS: u32 = 4;

/// First retry delay; doubles per attempt (1s, 2s, 4s).
const BACKOFF_BASE_MS: u64 = 1000;

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
    storage.check_membership_view().await?;
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

        match consumer.topic_kind(&topic) {
            Some(TopicKind::Edits) => {
                if let Err(e) =
                    with_transient_retry("edit", offset, || process_edit(payload, &storage)).await
                {
                    // Poison: a malformed edit is logged and skipped rather
                    // than stalling the partition (design §10).
                    warn!(error = %e, offset = offset, "Skipping unprocessable edit");
                }
                if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                    error!(error = %e, "Failed to commit offset");
                }
            }
            Some(TopicKind::Membership) => {
                let event_type = get_event_type(msg.headers());
                match parse_membership_event(payload, event_type.as_deref()) {
                    Ok(Some(event)) => {
                        if let Err(e) = with_transient_retry("membership event", offset, || {
                            apply_membership_event(&event, &storage)
                        })
                        .await
                        {
                            // Poison: unknown role or malformed ids.
                            warn!(error = %e, offset = offset, "Skipping unprocessable membership event");
                        }
                    }
                    // Known event type this indexer intentionally ignores
                    // (SPACE_LEFT, kg-indexer parity) — not warn-worthy.
                    Ok(None) => {
                        debug!(offset = offset, "Ignoring unhandled membership event type")
                    }
                    Err(e) => {
                        warn!(error = %e, offset = offset, "Skipping undecodable membership event");
                    }
                }
                if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                    error!(error = %e, "Failed to commit offset");
                }
            }
            None => {
                warn!(topic = %topic, "Message on unexpected topic — skipping");
                let _ = consumer.commit_message(&topic, partition, offset);
            }
        }
    }

    Ok(())
}

/// Run one message's processing, retrying transient errors with capped
/// exponential backoff.
///
/// `Ok(())` means processing succeeded; `Err(e)` means the error is poison
/// (retrying can never succeed) and the caller should skip + commit. A
/// transient error that survives every attempt exits the process instead:
/// the offset is never committed, so Kafka redelivers the message on restart
/// and the idempotent writes converge.
async fn with_transient_retry<F, Fut>(
    kind: &str,
    offset: i64,
    mut op: F,
) -> Result<(), IndexerError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), IndexerError>>,
{
    let mut attempt: u32 = 1;
    loop {
        match op().await {
            Ok(()) => return Ok(()),
            Err(e) if e.is_poison() => return Err(e),
            Err(e) if attempt < MAX_TRANSIENT_ATTEMPTS => {
                let delay_ms = BACKOFF_BASE_MS << (attempt - 1);
                warn!(
                    error = %e,
                    kind = kind,
                    offset = offset,
                    attempt = attempt,
                    delay_ms = delay_ms,
                    "Transient error — retrying"
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                attempt += 1;
            }
            Err(e) => {
                error!(
                    error = %e,
                    kind = kind,
                    offset = offset,
                    attempts = attempt,
                    "Transient error persisted — exiting so Kafka redelivers from the last \
                     committed offset"
                );
                std::process::exit(1);
            }
        }
    }
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
