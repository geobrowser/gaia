//! One-off/scheduled backfill for Ranking Block and Rank entities that never
//! made it into the `ranks` schema because their `TYPES` relation arrived in
//! a different edit than their own creation (`detect()` only classifies an
//! entity from the current edit's own ops).
//!
//! For blocks, the live consumer has a reactive one-shot recovery (issue
//! #738/#739, `get_block_config_from_kg`) that runs the moment a rank links
//! to an unregistered block — but it never retries, so anything that raced
//! ahead of kg-indexer indexing the type, or that nothing has linked to
//! since, is stuck forever. For ranks, there is no reactive recovery at all;
//! this binary is the only thing that ever finds them.
//!
//! Finds every entity typed `Ranking Block`/`Rank` in the public graph
//! that's missing from `ranks.ranking_blocks`/`ranks.rankings`, recovers its
//! config from the same source those tables are meant to reflect, and
//! recomputes the aggregate for every block touched (directly, or via a
//! recovered rank that links to one). Also finds blocks that already have a
//! row but are stuck with a stale `ranking_type` — the identical gap one
//! step later: a block created static and tagged Rolling by a subsequent,
//! separate edit never gets its existing row updated, since `detect()` only
//! reads a `CreateEntity`'s own ops (found investigating GEO-2328/PR#821).
//! Idempotent — safe to re-run; blocks/ranks with nothing stale are never
//! touched.
//!
//! Exits non-zero if anything failed to recover, so a scheduled run (see
//! `k8s/*/backfill-cronjob.yaml`) surfaces failures as a failed Job instead
//! of a silently-green one.
//!
//! Usage: `DATABASE_URL=... cargo run -p ranking-indexer --bin backfill_blocks`
//! Add `DRY_RUN=true` to list candidates without writing anything.

use std::collections::HashSet;
use std::env;

use chrono::Utc;
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

    let block_candidates = storage.find_unregistered_ranking_blocks().await?;
    let stale_blocks = storage.find_stale_ranking_type_blocks().await?;
    let rank_candidates = storage.find_unregistered_ranks().await?;
    info!(
        blocks = block_candidates.len(),
        stale_blocks = stale_blocks.len(),
        ranks = rank_candidates.len(),
        dry_run,
        "found unregistered/stale blocks and unregistered ranks"
    );

    if dry_run {
        for id in &block_candidates {
            info!(block_id = %id, "would recover block");
        }
        for id in &stale_blocks {
            info!(block_id = %id, "would resync stale ranking_type");
        }
        for id in &rank_candidates {
            info!(rank_id = %id, "would recover rank");
        }
        return Ok(());
    }

    if block_candidates.is_empty() && stale_blocks.is_empty() && rank_candidates.is_empty() {
        return Ok(());
    }

    let meta = storage.current_chain_meta().await?;
    let now = Utc::now();

    let mut recovered_blocks = 0;
    let mut still_unregistered_blocks = 0;
    let mut resynced_stale_blocks = 0;
    let mut recovered_ranks = 0;
    let mut still_unregistered_ranks = 0;
    let mut failed = 0;

    // Blocks first: a recovered rank's block_id is guaranteed to already be
    // registered by the time we get to it below, since this scan already
    // covers every currently-unregistered block, not just ones ranks
    // happen to point at.
    for block_id in block_candidates {
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
                if let Err(e) = recompute_block(block_id, meta, now, &storage).await {
                    error!(block_id = %block_id, error = %e, "recovered block but recompute failed");
                    failed += 1;
                    continue;
                }
                info!(block_id = %block_id, "recovered + recomputed block");
                recovered_blocks += 1;
            }
            Ok(None) => {
                // Typed in `relations` a moment ago, not typed now — a
                // concurrent edit removed the type between the scan and here.
                warn!(block_id = %block_id, "no longer typed as Ranking Block — skipping");
                still_unregistered_blocks += 1;
            }
            Err(e) => {
                error!(block_id = %block_id, error = %e, "failed to read block config from kg");
                failed += 1;
            }
        }
    }

    // Already-registered blocks stuck with a stale `ranking_type` — same
    // recovery as above (`get_block_config_from_kg` + `upsert_ranking_block`
    // is already an upsert, so this safely overwrites the stale row), just
    // starting from a row that already exists.
    for block_id in stale_blocks {
        match storage.get_block_config_from_kg(block_id).await {
            Ok(Some(block)) => {
                if let Err(e) = storage.upsert_ranking_block(&block).await {
                    error!(block_id = %block_id, error = %e, "failed to upsert resynced block");
                    failed += 1;
                    continue;
                }
                if let Err(e) = recompute_block(block_id, meta, now, &storage).await {
                    error!(block_id = %block_id, error = %e, "resynced block but recompute failed");
                    failed += 1;
                    continue;
                }
                info!(block_id = %block_id, "resynced stale ranking_type + recomputed block");
                resynced_stale_blocks += 1;
            }
            Ok(None) => {
                // No longer typed as a Ranking Block at all — leave its
                // existing row alone; that's a different (currently
                // unhandled) case, not this gap.
                warn!(block_id = %block_id, "no longer typed as Ranking Block — skipping resync");
            }
            Err(e) => {
                error!(block_id = %block_id, error = %e, "failed to read block config from kg for resync");
                failed += 1;
            }
        }
    }

    let mut affected_blocks: HashSet<uuid::Uuid> = HashSet::new();
    for rank_id in rank_candidates {
        match storage.get_rank_config_from_kg(rank_id).await {
            Ok(Some(rank)) => {
                let block_id = rank.block_id;
                if let Err(e) = storage.upsert_ranking(&rank).await {
                    error!(rank_id = %rank_id, error = %e, "failed to upsert recovered rank");
                    failed += 1;
                    continue;
                }
                info!(rank_id = %rank_id, block_id = ?block_id, "recovered rank");
                recovered_ranks += 1;
                if let Some(block_id) = block_id {
                    affected_blocks.insert(block_id);
                }
            }
            Ok(None) => {
                warn!(rank_id = %rank_id, "no longer typed as Rank — skipping");
                still_unregistered_ranks += 1;
            }
            Err(e) => {
                error!(rank_id = %rank_id, error = %e, "failed to read rank config from kg");
                failed += 1;
            }
        }
    }

    // Recompute every block a recovered rank links to — its aggregate needs
    // to include the rank that just appeared.
    for block_id in affected_blocks {
        if let Err(e) = recompute_block(block_id, meta, now, &storage).await {
            error!(
                block_id = %block_id,
                error = %e,
                "recompute failed for block affected by a recovered rank"
            );
            failed += 1;
        }
    }

    info!(
        recovered_blocks,
        still_unregistered_blocks,
        resynced_stale_blocks,
        recovered_ranks,
        still_unregistered_ranks,
        failed,
        "backfill complete"
    );
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
