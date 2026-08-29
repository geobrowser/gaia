//! Periodic recompute for entities that have been commented on.
//!
//! The ranking score's comment term (migration 0079) reads `count(DISTINCT space_id)` over
//! reply-to relations, but nothing recomputes a score when a comment arrives. The only
//! trigger in the system is `refresh_ranking_scores`, which runs after a vote batch and is
//! keyed to the entities that batch touched — and a comment is not a vote. So a commented
//! entity keeps its pre-comment score until some unrelated vote happens to land on it,
//! which is indistinguishable from the feature not working.
//!
//! This binary is that missing trigger, and it is deliberately the dumbest thing that
//! works: re-score every entity anyone has replied to. There is no "recently commented"
//! slice to take, because `relations` has no `created_at`/`updated_at` — and at present
//! volumes there is no need for one (243 entities across 534 reply-to relations when this
//! was written). `refresh_entity_ranking_scores` is an idempotent upsert, so re-scoring an
//! entity nothing has changed rewrites it to the same value.
//!
//! WHY A SWEEP AND NOT AN ENQUEUE. The alternative was publishing a recompute from whichever
//! service writes the relation. That is `kg-indexer`, which has no Kafka producer, no HTTP
//! client, no post-commit hook, and no notion of `Comment` or `REPLY_TO` — those ids are
//! used only by the notification service and by SQL. It would have needed teaching about
//! comments *and* would have been the first downstream producer in the repo; every other
//! Rust service here is consumer-only. A sweep needs none of that, and
//! `ranking-indexer`'s `rolling_sweep` is the same pattern solving the same problem —
//! a stored score whose second staleness cause emits no event.
//!
//! Cadence is not hardcoded: tune the CronJob's `schedule`, not this file. Cost scales with
//! the number of commented entities, independent of edit or vote volume.
//!
//! Usage: `DATABASE_URL=... cargo run -p vote-indexer --bin comment_sweep`

use std::env;

use hermes_instrumentation::{error, info, Backend, Config};

use vote_indexer::error::IndexerError;
use vote_indexer::storage::Storage;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    // Without this the `info!`/`error!` below are no-ops and the CronJob runs completely
    // silently — a sweep that scored nothing would be indistinguishable from one that
    // worked. Console rather than the Sentry backend `main.rs` builds: a one-shot job
    // wants its output in `kubectl logs`, and errors already surface via a non-zero exit
    // marking the Job failed.
    let _telemetry = hermes_instrumentation::init(Config::new("vote-indexer", Backend::Console))?;

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;

    let storage = Storage::connect(&database_url).await?;

    let entity_ids = storage.commented_entity_ids().await?;
    info!(
        entities = entity_ids.len(),
        "comment sweep: found commented entities"
    );

    if entity_ids.is_empty() {
        return Ok(());
    }

    match storage.refresh_ranking_scores_for(&entity_ids).await {
        Ok(scored) => {
            info!(scored, "comment sweep complete");
            Ok(())
        }
        Err(e) => {
            // Exit non-zero so the CronJob is marked failed rather than reporting a
            // successful run that scored nothing.
            error!(error = %e, "comment sweep: refresh failed");
            std::process::exit(1);
        }
    }
}
