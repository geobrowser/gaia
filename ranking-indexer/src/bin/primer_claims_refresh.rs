//! Refreshes the onboarding primer and anchor claims (GEO-3143, migration 0103).
//!
//! Three steps, run hourly by the `ranking-indexer-primer-claims` CronJob, a few minutes after the
//! account weights it reads (GEO-3141, at :53):
//!
//! 1. `refresh_primer_claim_stats()` rebuilds every claim's weighted split, information and
//!    engagement. The primer (`next_primer_claim`) reads these, so a new vote counts within the hour.
//! 2. `refresh_anchor_claims()` chooses a new anchor set on the first run of each calendar month
//!    (UTC) and otherwise does nothing. `ANCHOR_FORCE=1` chooses one now; use it once after the first
//!    deploy if the month's set should not wait, or after a retune.
//! 3. `record_claim_overlap_sample()` records, once a day, the share of user pairs that share at
//!    least 3 claims — the ticket's outcome measure against the 2 Oct baseline (26.5%).
//!
//! Each is one function call and so one transaction. Steps 1 and 2 run in the same transaction, so
//! an anchor set is never chosen from half-refreshed statistics.
//!
//! Usage: `DATABASE_URL=... cargo run --bin primer_claims_refresh`

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;

/// The ticket asks for 30-50 anchors; refresh_anchor_claims aims for 40 and keeps at least 30.
const ANCHOR_MIN: i32 = 30;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let force_anchors = matches!(
        env::var("ANCHOR_FORCE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    let started = Instant::now();
    // Any failure returns Err, so the process exits non-zero and the Job is marked failed.
    let mut tx = pool.begin().await?;
    let scored: i32 = sqlx::query_scalar("SELECT public.refresh_primer_claim_stats()")
        .fetch_one(&mut *tx)
        .await?;
    let new_set: Option<i32> =
        sqlx::query_scalar("SELECT public.refresh_anchor_claims(force => $1)")
            .bind(force_anchors)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;

    let (contested, eligible): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE contested), count(*) FILTER (WHERE weighted_voters >= 3) \
         FROM public.primer_claim_stats",
    )
    .fetch_one(&pool)
    .await?;
    info!(
        scored,
        eligible,
        contested,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "primer claim statistics refreshed"
    );

    if let Some(set_id) = new_set {
        let size: i32 =
            sqlx::query_scalar("SELECT claim_count FROM public.anchor_claim_sets WHERE id = $1")
                .bind(set_id)
                .fetch_one(&pool)
                .await?;
        if size < ANCHOR_MIN {
            warn!(
                set_id,
                size,
                "new anchor set is smaller than {ANCHOR_MIN}: too few claims have enough voters"
            );
        } else {
            info!(
                set_id,
                size,
                forced = force_anchors,
                "new anchor set chosen"
            );
        }
    }

    let sampled: bool = sqlx::query_scalar("SELECT public.record_claim_overlap_sample()")
        .fetch_one(&pool)
        .await?;
    if sampled {
        let (users, share): (i32, Option<f64>) = sqlx::query_as(
            "SELECT users, share FROM public.claim_overlap_samples ORDER BY sampled_at DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await?;
        info!(
            users,
            share, "claim overlap sampled (pairs sharing at least 3 claims)"
        );
    }
    Ok(())
}
