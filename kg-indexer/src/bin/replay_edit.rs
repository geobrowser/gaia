//! One-off manual replay for a single edit that `hermes-pipeline` permanently
//! dropped due to a false-positive `ipfs_cache.is_errored` (see
//! `ipfs::IpfsClient::get_bytes` — no CID-hash verification on fetch, so a
//! transient truncated/corrupted read gets marked errored forever with no
//! retry). Once the cache row is corrected (real payload + `is_errored =
//! false`), the edit itself still never reaches `kg-indexer` on its own —
//! `hermes-pipeline` is a one-shot, event-driven consumer with no backfill
//! path for edits it already dropped.
//!
//! This bypasses Kafka entirely: it reads the (now-fixed) cached payload
//! directly, decodes it, and runs it through the exact same `handle_edit` +
//! storage-write sequence `kg-indexer`'s real Kafka consumer uses for a
//! `KgMessage::Edit`, in one transaction.
//!
//! Idempotency is keyed on `edit_id` (embedded in the edit payload itself,
//! not on block/sequence), via `edit_versions`'s `ON CONFLICT (edit_id) DO
//! NOTHING` — so re-running this for an edit that already landed is a safe
//! no-op, not a duplicate.
//!
//! CAUTION — this does NOT check whether the edit's target (entity,
//! property, space) tuples have been touched by later, real edits since the
//! original block. Blindly replaying a stale edit can incorrectly stomp on
//! more-recent legitimate state (the versioned-write path always closes
//! whatever version is currently open, regardless of chronological order).
//! Verify the target values/version history by hand before running this for
//! any edit that *sets* or *unsets* values on a pre-existing entity —
//! creation-only edits (new entities/relations with no prior state) are safe
//! by construction.
//!
//! Usage (a single DATABASE_URL is correct here — `ipfs_cache` and the KG
//! tables this replays into are defined in the same migration/schema, see
//! `api/drizzle/0000_handy_omega_red.sql`):
//!   DATABASE_URL=... cargo run -p kg-indexer --bin replay_edit -- \
//!     --uri ipfs://Qm... \
//!     --space f3dab79c-b5a3-d9d1-7596-56dd5361d1c6 \
//!     --block 162685 \
//!     [--sequence 0] \
//!     [--created-at 1752781200]
//!
//! This writes Postgres ONLY. It ends by printing the paired `geodb-replay-edit` command
//! (geodb/RUNBOOK.md section 2b-quater); geo-db does not get the edit until that Job runs.

use std::env;

use grc_20::decode_edit;
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use kg_indexer::error::IndexerError;
use kg_indexer::geodb_twin::{twin_notice, TwinArgs};
use kg_indexer::handlers;
use kg_indexer::storage::Storage;
use uuid::Uuid;

/// Must match `main.rs`'s `MAX_EDIT_NAME_LENGTH` — this tool mirrors the real
/// Kafka consumer's storage-write behavior exactly, name truncation included.
const MAX_EDIT_NAME_LENGTH: usize = 256;

struct Args {
    uri: String,
    space: Uuid,
    block: u64,
    sequence: u32,
    created_at: u64,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let get = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let uri = get("--uri").expect("--uri is required");
    let space = get("--space")
        .expect("--space is required")
        .parse()
        .expect("--space must be a valid UUID");
    let block: u64 = get("--block")
        .expect("--block is required")
        .parse()
        .expect("--block must be a number");
    let sequence: u32 = get("--sequence")
        .map(|s| s.parse().expect("--sequence must be a number"))
        .unwrap_or(0);
    let created_at: u64 = match get("--created-at") {
        Some(s) => s.parse().expect("--created-at must be a unix timestamp"),
        None => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    };

    Args {
        uri,
        space,
        block,
        sequence,
        created_at,
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = parse_args();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let storage = Storage::new(&database_url).await?;

    // 1. Read the (already-fixed) cached payload directly — this must be a
    //    valid, non-errored row, or something upstream is still broken.
    let row: (Option<Vec<u8>>, bool) =
        sqlx::query_as("SELECT data, is_errored FROM ipfs_cache WHERE uri = $1")
            .bind(&args.uri)
            .fetch_one(&mut *storage.begin_operator_tx().await?)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => {
                    IndexerError::Config(format!("uri not found in ipfs_cache: {}", args.uri))
                }
                other => IndexerError::Database(other),
            })?;
    let (data, is_errored) = row;
    if is_errored {
        return Err(IndexerError::Config(
            "ipfs_cache row is still marked is_errored — fix that first".into(),
        ));
    }
    let payload = data.ok_or_else(|| IndexerError::Config("ipfs_cache row has no data".into()))?;

    // 2. Decode once here to fail fast with a clear error before touching the
    //    DB, and to log what we're about to replay.
    let decoded = decode_edit(&payload)
        .map_err(|e| IndexerError::Config(format!("payload does not decode: {e}")))?;
    println!(
        "Replaying edit {:?} ({} ops) into space {}",
        decoded.name,
        decoded.ops.len(),
        args.space
    );

    // 3. Build the exact HermesEdit hermes-pipeline would have produced.
    let edit = HermesEdit {
        id: decoded.id.to_vec(),
        name: decoded.name.to_string(),
        payload: payload.clone(),
        authors: decoded.authors.iter().map(|a| a.to_vec()).collect(),
        language: None,
        space_id: args.space.as_bytes().to_vec(),
        is_canonical: true,
        meta: Some(BlockchainMetadata {
            created_at: args.created_at,
            created_by: vec![],
            block_number: args.block,
            cursor: String::new(),
            sequence: args.sequence,
            is_last: false,
        }),
    };

    // 4. Replicate kg-indexer's real process_message(KgMessage::Edit) path
    //    exactly (see main.rs), in one transaction.
    let mut tx = storage.begin_operator_tx().await?;
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await?;

    // The same write path as the live consumer (`Storage::write_edit`), including how it treats
    // `DeleteEntity` under `KG_INDEXER_APPLY_DELETE_ENTITY`.
    let result = handlers::edits::handle_edit_for(&edit, storage.apply_delete_entity())?;
    let edit_id = result.edit_id;

    // Mirror main.rs's extract_edit_metadata exactly, so a replayed
    // edit's stored name matches what the real Kafka consumer would
    // have written for the same payload.
    let name = if edit.name.is_empty() {
        None
    } else if edit.name.len() > MAX_EDIT_NAME_LENGTH {
        Some(&edit.name[..edit.name.floor_char_boundary(MAX_EDIT_NAME_LENGTH)])
    } else {
        Some(edit.name.as_str())
    };
    let created_by_id = edit.authors.first().and_then(|a| Uuid::from_slice(a).ok());

    let outcome = storage
        .write_edit(&edit, result, name, created_by_id, &mut tx)
        .await?;
    if outcome.entities_deleted > 0 {
        println!(
            "DeleteEntity {}: {} entities, {:?}",
            if outcome.deletes.applied {
                "applied"
            } else {
                "would delete (KG_INDEXER_APPLY_DELETE_ENTITY is off)"
            },
            outcome.entities_deleted,
            outcome.deletes
        );
    }
    match outcome.version_key {
        Some(version_key) => println!("Wrote {} operations, version_key={version_key}", outcome.ops),
        None => println!(
            "edit_id {edit_id} already has a version (or carries no metadata) — skipped value/relation version rows (live-table writes above already ran, idempotently)"
        ),
    }

    tx.commit().await?;
    println!("Done.");
    // Always last, on every path (a fresh write and an already-versioned rerun alike): geo-db has
    // not got this edit until the paired geodb-replay-edit Job runs.
    println!(
        "{}",
        twin_notice(&TwinArgs {
            uri: &args.uri,
            space: &args.space.simple().to_string(),
            block: args.block,
            sequence: args.sequence,
            created_at: args.created_at,
            edit_id: &result.edit_id.simple().to_string(),
        })
    );
    Ok(())
}
