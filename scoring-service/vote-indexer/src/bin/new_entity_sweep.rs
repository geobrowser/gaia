//! Periodic first score for entities nobody has engaged with.
//!
//! The Best feed (`entities_ranked_for_feed`, and the typed walk over `entity_type_ranking`)
//! INNER JOINs the stored ranking score, and nothing creates that score when an entity is
//! created. A row appears only when the entity is voted on (`refresh_ranking_scores` after
//! a vote batch), commented on (`comment_sweep`), or swept up by the hand-run
//! `api/scripts/backfill-entity-ranking-scores.sh`. So an entity nobody has touched yet is
//! not ranked low — it is absent. On 2026-09-29 that was 2.27M of 51.2M entities,
//! every one created after the 2026-08-13 backfill, including ~92% of new Debate-tagged
//! items and ~63% of new news stories.
//!
//! This binary is the missing creation trigger, in the same shape as `comment_sweep`:
//! find entities with no score row and hand them to `refresh_entity_ranking_scores`, which
//! is an idempotent upsert that also reconciles `entity_type_ranking`. It scores every
//! type, as the 0074 backfill did, so the table stays complete rather than selectively so.
//!
//! WHY A WINDOW. "No score row" over all 51M entities is a hash anti-join over both tables
//! (tens of seconds, every hour). Restricting to recently created entities turns it into an
//! index range scan on `entities_created_at_id_idx`. The window is
//! `NEW_ENTITY_SWEEP_LOOKBACK_HOURS`, default 48 — see `DEFAULT_LOOKBACK_HOURS`. Anything
//! older that is unscored (the backlog from before this existed, or entities indexed with
//! an old block timestamp during a long replay) needs one run with a wider window; the
//! query only returns unscored entities, so re-running is always safe.
//!
//! WHY SCORE AT ALL BEFORE ENGAGEMENT. An unscored entity still has a meaningful score:
//! recency (`created_at / tau`), intrinsic (properties and relations), type weight and the
//! Wilson prior. That is what the backfill gave every entity; this keeps new ones level with
//! it. The score is written once here and then moved by votes and comments as before.
//!
//! Usage: `DATABASE_URL=... cargo run -p vote-indexer --bin new_entity_sweep`

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use hermes_instrumentation::{error, info, Backend, Config};
use uuid::Uuid;

use vote_indexer::error::IndexerError;
use vote_indexer::new_entity_sweep::{cutoff, lookback_hours};
use vote_indexer::storage::{Storage, RANKING_REFRESH_BATCH_SIZE};

/// Log progress every this many pages, so a backlog run shows it is moving without a
/// line per 500 entities.
const PROGRESS_EVERY_PAGES: u64 = 100;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    // Console, as in `comment_sweep`: a one-shot job wants its output in `kubectl logs`,
    // and a failure already surfaces as a non-zero exit.
    let _telemetry = hermes_instrumentation::init(Config::new("vote-indexer", Backend::Console))?;

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let hours = lookback_hours(env::var("NEW_ENTITY_SWEEP_LOOKBACK_HOURS").ok().as_deref())
        .map_err(IndexerError::Config)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| IndexerError::Config(format!("system clock before 1970: {e}")))?
        .as_secs();
    let since = cutoff(now, hours);

    let storage = Storage::connect(&database_url).await?;
    info!(lookback_hours = hours, since = %since, "new entity sweep: starting");

    let mut cursor = (since, Uuid::nil());
    let mut pages = 0u64;
    let mut scored = 0u64;

    loop {
        let page = storage
            .unscored_entities_page(&cursor, RANKING_REFRESH_BATCH_SIZE as i64)
            .await?;
        let Some(last) = page.last() else { break };
        cursor = last.clone();

        let ids: Vec<Uuid> = page.iter().map(|(_, id)| *id).collect();
        match storage.refresh_ranking_scores_for(&ids).await {
            Ok(n) => scored += n,
            Err(e) => {
                // Non-zero exit so the Job is marked failed. Everything scored so far is
                // committed, and the next run starts from whatever is still unscored.
                error!(error = %e, scored, "new entity sweep: refresh failed");
                std::process::exit(1);
            }
        }

        pages += 1;
        if pages % PROGRESS_EVERY_PAGES == 0 {
            info!(pages, scored, through = %cursor.0, "new entity sweep: progress");
        }
        if page.len() < RANKING_REFRESH_BATCH_SIZE {
            break;
        }
    }

    info!(pages, scored, "new entity sweep complete");
    Ok(())
}
