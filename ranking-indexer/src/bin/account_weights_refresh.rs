//! Recomputes every voter's account weight (GEO-3141).
//!
//! An account's weight in [0, 1] is how much its votes count towards OTHER people's
//! personalization: excluded test accounts weigh 0, new accounts ramp up over a week, and an
//! account that votes the same way on nearly every claim is weighted down. The rules and their
//! thresholds live in `refresh_account_weights()` (migration 0101); this binary is the schedule:
//! one call, run hourly by the `ranking-indexer-account-weights` CronJob.
//!
//! The refresh is a single function call, so it is one transaction: readers keep the previous
//! weights until the new ones commit.
//!
//! Usage: `DATABASE_URL=... cargo run --bin account_weights_refresh`

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
    let scored: i32 = sqlx::query_scalar("SELECT public.refresh_account_weights()")
        .fetch_one(&pool)
        .await?;
    let (excluded, reduced): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE weight = 0), count(*) FILTER (WHERE weight > 0 AND weight < 1) \
         FROM public.account_weights",
    )
    .fetch_one(&pool)
    .await?;
    info!(
        scored,
        excluded,
        reduced,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "account weights refreshed"
    );
    Ok(())
}
