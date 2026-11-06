//! Rebuilds the onboarding topic suggestions (GEO-3079).
//!
//! The onboarding "Customize your feed" step offers topics ranked by how many claims and
//! debates in the chosen spaces link to each. Counting that per request is too slow for a
//! dialog that must open instantly, and the ranking barely moves day to day, so
//! `refresh_space_topic_suggestions()` (migration 0095) precomputes it and the dialog reads
//! `topic_suggestions_for_spaces`. This binary is the schedule: one call, run hourly by the
//! `ranking-indexer-topic-suggestions` CronJob.
//!
//! The refresh is a single function call, so it is one transaction: readers keep the
//! previous ranking until the new one commits. On the live DB it took 1.6s (2026-09-29).
//!
//! Usage: `DATABASE_URL=... cargo run --bin topic_suggestions_refresh`

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::topic_suggestions_mirror::{self, Suggestion};

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    // Best-effort copy into geo-db's derived class (GEODB_DERIVED_MIRROR, default off). With it off
    // `from_env` is None and nothing below reads the table a second time.
    let mirror = geodb_cursor::derived::DerivedMirror::from_env();
    let prior: Vec<(uuid::Uuid, uuid::Uuid)> = if mirror.is_some() {
        sqlx::query_as(topic_suggestions_mirror::PRIOR_SQL)
            .fetch_all(&pool)
            .await?
    } else {
        Vec::new()
    };

    let started = Instant::now();
    // A failure returns Err, so the process exits non-zero and the Job is marked failed.
    let rows: i32 = sqlx::query_scalar("SELECT public.refresh_space_topic_suggestions()")
        .fetch_one(&pool)
        .await?;
    info!(
        rows,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "topic suggestions refreshed"
    );
    if let Some(mirror) = &mirror {
        // Read after the refresh committed; a read error skips the mirror, never the job.
        match sqlx::query_as::<_, Suggestion>(topic_suggestions_mirror::SELECT_SQL)
            .fetch_all(&pool)
            .await
        {
            Ok(rows) => {
                topic_suggestions_mirror::mirror(mirror, &rows, &prior).await;
                info!(
                    rows = rows.len(),
                    prior = prior.len(),
                    "topic suggestions mirrored to geo-db"
                );
            }
            Err(e) => warn!(error = %e, "topic suggestions mirror skipped: read failed"),
        }
    }
    Ok(())
}
