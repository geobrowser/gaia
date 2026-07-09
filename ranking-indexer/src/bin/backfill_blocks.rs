//! One-off backfill for Ranking Block entities that never made it into
//! `ranks.ranking_blocks` (issue #738/#739's recovery path only runs when a
//! rank happens to link to the block after kg-indexer has already indexed its
//! type/config — anything that never got a second chance is stuck forever).
//!
//! Finds every entity typed `Ranking Block` in the public graph that's
//! missing from `ranks.ranking_blocks`, recovers its config the same way the
//! live recovery path does, and recomputes its aggregate. Idempotent — safe
//! to re-run; already-registered blocks are never touched.
//!
//! Exits non-zero if any block failed to recover, so a scheduled run (see
//! `k8s/*/backfill-cronjob.yaml`) surfaces failures as a failed Job instead
//! of a silently-green one.
//!
//! Usage: `DATABASE_URL=... cargo run -p ranking-indexer --bin backfill_blocks`
//! Add `DRY_RUN=true` to list candidates without writing anything.

use std::env;

use tracing::{error, info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::recompute::recompute_block;
use ranking_indexer::storage::Storage;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let dry_run = env::var("DRY_RUN").is_ok_and(|v| v == "true" || v == "1");

    let storage = Storage::new(&database_url).await?;

    let candidates = storage.find_unregistered_ranking_blocks().await?;
    info!(
        count = candidates.len(),
        dry_run, "found unregistered blocks"
    );
    if candidates.is_empty() {
        return Ok(());
    }

    if dry_run {
        for id in &candidates {
            info!(block_id = %id, "would recover");
        }
        return Ok(());
    }

    let meta = storage.current_chain_meta().await?;

    let mut recovered = 0;
    let mut still_unregistered = 0;
    let mut failed = 0;

    for block_id in candidates {
        // Re-check under get_block_config_from_kg rather than trusting the
        // earlier scan — the two queries aren't in the same transaction, and
        // this mirrors exactly what the live recovery path does.
        match storage.get_block_config_from_kg(block_id).await {
            Ok(Some(block)) => {
                if let Err(e) = storage.upsert_ranking_block(&block).await {
                    error!(block_id = %block_id, error = %e, "failed to upsert recovered block");
                    failed += 1;
                    continue;
                }
                if let Err(e) = recompute_block(block_id, meta, &storage).await {
                    error!(block_id = %block_id, error = %e, "recovered block but recompute failed");
                    failed += 1;
                    continue;
                }
                info!(block_id = %block_id, "recovered + recomputed");
                recovered += 1;
            }
            Ok(None) => {
                // Typed in `relations` a moment ago, not typed now — a
                // concurrent edit removed the type between the scan and here.
                warn!(block_id = %block_id, "no longer typed as Ranking Block — skipping");
                still_unregistered += 1;
            }
            Err(e) => {
                error!(block_id = %block_id, error = %e, "failed to read block config from kg");
                failed += 1;
            }
        }
    }

    info!(recovered, still_unregistered, failed, "backfill complete");
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
