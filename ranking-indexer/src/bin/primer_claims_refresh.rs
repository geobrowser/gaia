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

use ranking_indexer::anchor_select;
use ranking_indexer::error::IndexerError;
use ranking_indexer::primer_claims_mirror::{
    self as family, AnchorMember, AnchorSet, ClaimStats, PriorClaim,
};
use ranking_indexer::primer_mirror::{self, OverlapSample};
use uuid::Uuid;

/// The ticket asks for 30-50 anchors; refresh_anchor_claims aims for 40 and keeps at least 30.
const ANCHOR_MIN: i32 = 30;

/// `ANCHOR_RUST_COMPUTE` (default off): after a new anchor set is chosen, choose it again in code
/// from the stats table and log whether the same claims came out in the same order.
fn anchor_rust_enabled() -> bool {
    matches!(
        env::var("ANCHOR_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

async fn anchor_shadow(pool: &sqlx::PgPool, set_id: i32) -> Result<bool, IndexerError> {
    let rows: Vec<(Uuid, Vec<Uuid>, f64, f64, f64, bool)> = sqlx::query_as(
        "SELECT claim_id, topic_ids, weighted_voters, agree_share, score, contested \
         FROM public.primer_claim_stats",
    )
    .fetch_all(pool)
    .await?;
    let stats: Vec<anchor_select::Candidate> = rows
        .into_iter()
        .map(
            |(claim_id, topic_ids, weighted_voters, agree_share, score, contested)| {
                anchor_select::Candidate {
                    claim_id,
                    topic_ids,
                    weighted_voters,
                    agree_share,
                    score,
                    contested,
                }
            },
        )
        .collect();
    let sql: Vec<(Uuid, i32, f64, f64, f64)> = sqlx::query_as(
        "SELECT claim_id, rank, score, weighted_voters, agree_share \
         FROM public.anchor_claim_members WHERE set_id = $1 ORDER BY rank",
    )
    .bind(set_id)
    .fetch_all(pool)
    .await?;
    let sql: Vec<anchor_select::Member> = sql
        .into_iter()
        .map(
            |(claim_id, rank, score, weighted_voters, agree_share)| anchor_select::Member {
                rank,
                claim_id,
                score,
                weighted_voters,
                agree_share,
            },
        )
        .collect();
    let rust = anchor_select::select(&stats, &anchor_select::Params::default())?;
    Ok(anchor_select::same_members(&rust, &sql))
}

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

    // Best-effort copy into geo-db's derived class (GEODB_DERIVED_MIRROR, default off). With it off
    // `from_env` is None and nothing below reads a table a second time. A failed read of what the
    // stats table held skips that mirror (a delete list built without it would be wrong), never the job.
    let mirror = geodb_cursor::derived::DerivedMirror::from_env();
    let prior: Option<Vec<PriorClaim>> = if mirror.is_some() {
        match sqlx::query_as::<_, PriorClaim>(family::PRIOR_SQL)
            .fetch_all(&pool)
            .await
        {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, "primer stats mirror skipped: prior read failed");
                None
            }
        }
    } else {
        None
    };

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

    if let (Some(set_id), true) = (new_set, anchor_rust_enabled()) {
        match anchor_shadow(&pool, set_id).await {
            Ok(same) => info!(
                set_id,
                same, "anchor set rust compute checked against the SQL"
            ),
            Err(e) => warn!(error = %e, "anchor set rust compute skipped"),
        }
    }
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

    // Read after every commit above; a read error skips that mirror, never the job.
    if let Some(mirror) = &mirror {
        // Collection 14.
        match sqlx::query_as::<_, OverlapSample>(primer_mirror::SELECT_SQL)
            .fetch_all(&pool)
            .await
        {
            Ok(rows) => {
                primer_mirror::mirror(mirror, &rows).await;
                info!(
                    rows = rows.len(),
                    "claim overlap samples mirrored to geo-db"
                );
            }
            Err(e) => warn!(error = %e, "claim overlap mirror skipped: read failed"),
        }
        // Collections 15 and 16.
        match (
            sqlx::query_as::<_, AnchorSet>(family::SETS_SQL)
                .fetch_all(&pool)
                .await,
            sqlx::query_as::<_, AnchorMember>(family::MEMBERS_SQL)
                .fetch_all(&pool)
                .await,
        ) {
            (Ok(sets), Ok(members)) => {
                family::mirror_anchors(mirror, &sets, &members).await;
                info!(
                    sets = sets.len(),
                    members = members.len(),
                    "anchor claim sets mirrored to geo-db"
                );
            }
            (a, b) => warn!(
                error = %a.err().map(|e| e.to_string()).or(b.err().map(|e| e.to_string())).unwrap_or_default(),
                "anchor claim mirror skipped: read failed"
            ),
        }
        // Collections 17, 18 and 19.
        if let Some(prior) = &prior {
            match sqlx::query_as::<_, ClaimStats>(family::STATS_SQL)
                .fetch_all(&pool)
                .await
            {
                Ok(rows) => {
                    family::mirror_stats(mirror, &rows, prior).await;
                    info!(
                        rows = rows.len(),
                        prior = prior.len(),
                        "primer claim stats mirrored to geo-db"
                    );
                }
                Err(e) => warn!(error = %e, "primer stats mirror skipped: read failed"),
            }
        }
    }
    Ok(())
}
