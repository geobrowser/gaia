//! Periodic sweep for Rolling ranking blocks (GEO-2328).
//!
//! Recompute is otherwise purely event-driven (design §7, `recompute.rs`) —
//! it only runs in reaction to an edit or a membership event. A Rolling
//! block's aggregate needs to change purely from elapsed time too: once a
//! submission ages past its `submission_frequency`, nothing on-chain happens,
//! so without this the block's projection stays frozen at whatever it was
//! the last time an edit happened to touch it (see `eligibility::rolling_admits`).
//! This binary is that missing time-based trigger: find every registered
//! Rolling block and recompute it against the current instant.
//!
//! Structurally mirrors `backfill_blocks.rs`'s one-shot-per-invocation
//! CronJob wiring, but a different purpose: that one is a data-integrity
//! self-heal for split-edit races (#738/#739), this one is the actual timing
//! mechanism Rolling rankings need to function at all.
//!
//! Cadence is intentionally NOT hardcoded here — how often this needs to run
//! is a product question (how stale an expired Rolling ranking is allowed to
//! look, confirmed dynamic-with-`submission_frequency` and easily
//! configurable per the GEO-2328 spec discussion), not an engineering one.
//! Tune it via the CronJob's `schedule` field (see
//! `k8s/*/rolling-sweep-cronjob.yaml`) to roughly track the shortest
//! `submission_frequency` in use across Rolling blocks — no code change
//! required to retune it.
//!
//! Cost scales with (# Rolling blocks × ticks/day), independent of edit
//! volume — unlike everything else in this indexer today.
//!
//! Usage: `DATABASE_URL=... cargo run -p ranking-indexer --bin rolling_sweep`

use std::env;

use chrono::Utc;
use tracing::{error, info};

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

    let storage = Storage::new(&database_url).await?;

    let blocks = storage.find_rolling_ranking_blocks().await?;
    info!(blocks = blocks.len(), "rolling sweep: found Rolling blocks");

    if blocks.is_empty() {
        return Ok(());
    }

    // Same stamping convention as backfill_blocks: this recompute isn't
    // reacting to a specific edit, so there's no natural block/timestamp to
    // attach to whatever the projection mints — use the chain tip the
    // indexer has actually caught up to, and the wall-clock instant this
    // sweep is evaluating expiry against.
    let meta = storage.current_chain_meta().await?;
    let now = Utc::now();

    let mut recomputed = 0;
    let mut failed = 0;
    for block_id in blocks {
        match recompute_block(block_id, meta, now, &storage).await {
            Ok(()) => recomputed += 1,
            Err(e) => {
                error!(block_id = %block_id, error = %e, "rolling sweep: recompute failed");
                failed += 1;
            }
        }
    }

    info!(recomputed, failed, "rolling sweep complete");
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
