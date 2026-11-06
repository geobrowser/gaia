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
//! With `ACCOUNT_WEIGHTS_RUST_COMPUTE=1` the mirror sends rows scored in code
//! (`account_weights_compute`), after checking them against the table the SQL wrote.
//! With `GEODB_DERIVED_MIRROR=1` the table is then copied into geo-db collection 5 (RUNBOOK 2f-weights).
//!
//! Usage: `DATABASE_URL=... cargo run --bin account_weights_refresh`

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::info;

use ranking_indexer::account_weights_compute::{self, Params, VoterAgg};
use ranking_indexer::account_weights_mirror::{self, AccountWeight};
use ranking_indexer::error::IndexerError;
use uuid::Uuid;

fn rust_compute_enabled() -> bool {
    matches!(
        env::var("ACCOUNT_WEIGHTS_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Scores the voters in code as of now, from the same inputs the SQL read.
async fn rust_rows(pool: &sqlx::PgPool) -> Result<Vec<AccountWeight>, IndexerError> {
    let voters: Vec<VoterAgg> = sqlx::query_as(account_weights_compute::AGG_SQL)
        .fetch_all(pool)
        .await?;
    let excluded: std::collections::HashSet<Uuid> =
        sqlx::query_scalar(account_weights_compute::EXCLUSIONS_SQL)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    account_weights_compute::compute(
        &voters,
        &excluded,
        chrono::Utc::now().timestamp_micros(),
        &Params::default(),
    )
}

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
    // `from_env` is None and nothing below touches the table a second time.
    let mirror = geodb_cursor::derived::DerivedMirror::from_env();
    let prior: Vec<Uuid> = if mirror.is_some() {
        sqlx::query_scalar("SELECT user_id FROM public.account_weights")
            .fetch_all(&pool)
            .await?
    } else {
        Vec::new()
    };

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
    if let Some(mirror) = &mirror {
        // Read after the refresh committed; a read error skips the mirror, never the job.
        match sqlx::query_as::<_, AccountWeight>(account_weights_mirror::SELECT_SQL)
            .fetch_all(&pool)
            .await
        {
            Ok(table) => {
                // ACCOUNT_WEIGHTS_RUST_COMPUTE (default off): score in code from the votes and
                // exclusions, mirror those rows, and log how many differ from the table the SQL
                // just wrote. Any failure falls back to mirroring the table.
                let rows = if rust_compute_enabled() {
                    match rust_rows(&pool).await {
                        Ok(rust) => {
                            let mismatched =
                                account_weights_compute::count_mismatches(&rust, &table);
                            info!(
                                rust = rust.len(),
                                table = table.len(),
                                mismatched,
                                "account weights rust compute checked against the table"
                            );
                            if mismatched == 0 {
                                rust
                            } else {
                                table
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "account weights rust compute skipped");
                            table
                        }
                    }
                } else {
                    table
                };
                account_weights_mirror::mirror(mirror, &rows, &prior).await;
                info!(
                    rows = rows.len(),
                    prior = prior.len(),
                    "account weights mirrored to geo-db"
                );
            }
            Err(e) => tracing::warn!(error = %e, "account weights mirror skipped: read failed"),
        }
    }
    info!(
        scored,
        excluded,
        reduced,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "account weights refreshed"
    );
    Ok(())
}
