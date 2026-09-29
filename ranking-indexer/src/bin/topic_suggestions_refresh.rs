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
use tracing::info;

use ranking_indexer::error::IndexerError;

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
    Ok(())
}
