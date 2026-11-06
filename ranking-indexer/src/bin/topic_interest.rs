//! Learns each user's topic interests from what they already do (GEO-3088, migration 0100).
//!
//! `topic_interest sweep` runs every two minutes: it recomputes every user with activity since the
//! last run (votes, comments, debates, follows), so a new vote moves that user's weights within
//! minutes. `topic_interest refit` runs nightly: it rebuilds topic co-occurrence, recomputes every
//! user from scratch, and logs how far the incremental state had drifted (no longer recorded in a
//! table; migration 0117).
//!
//! Both are separate CronJobs from the vote-indexer, deliberately: since GEO-3101 the vote-indexer
//! halts on a failed write, and nothing about interest learning may be able to stop vote indexing.
//!
//! Usage: `DATABASE_URL=... cargo run --bin topic_interest -- sweep|refit`

use std::env;
use std::time::Instant;

use geodb_cursor::derived::DerivedMirror;
use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::topic_cooccurrence_compute as cooc;
use ranking_indexer::topic_cooccurrence_mirror::{self, Neighbour};
use ranking_indexer::topic_interest::{self, Mode, SweepOutcome};
use ranking_indexer::user_interest;
use ranking_indexer::user_interest_shadow;
use ranking_indexer::user_topic_signals_mirror::{self as signals_mirror, Ident, Signal};

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mode: Mode = env::args()
        .nth(1)
        .ok_or_else(|| IndexerError::Config("usage: topic_interest sweep|refit".into()))?
        .parse()?;
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    let started = Instant::now();
    // Any error returns Err, so the process exits non-zero and the Job is marked failed.
    match mode {
        Mode::Sweep => {
            // Best-effort copy of the rows the sweep touched into geo-db's derived class (both
            // GEODB_DERIVED_MIRROR and GEODB_DERIVED_MIRROR_PRIVATE, default off).
            let mirror = signals_mirror::from_env_private();
            let prior = read_idents(&pool, mirror.as_ref()).await;
            let outcome = topic_interest::sweep(&pool, None).await?;
            if matches!(outcome, SweepOutcome::Ran(_)) {
                if let (Some(m), Some(prior)) = (&mirror, prior) {
                    mirror_signals(
                        &pool,
                        m,
                        &prior,
                        signals_mirror::sweep_since(chrono::Utc::now()),
                    )
                    .await;
                }
            }
            // The sweep watermark the sweep just moved (collection 6). Own flag, default off.
            if matches!(outcome, SweepOutcome::Ran(_)) {
                if let Some(state) = ranking_indexer::state_mirror::from_env() {
                    let acked = ranking_indexer::state_mirror::mirror_sweep(&state, &pool).await;
                    info!(
                        acknowledged = acked,
                        "interest sweep state mirrored to geo-db"
                    );
                }
            }
            // Flag-off shadow of the Rust twin (USER_INTEREST_RUST_SHADOW): read-only, counts only,
            // never changes the outcome or the exit code.
            if matches!(outcome, SweepOutcome::Ran(_)) && user_interest::shadow_enabled() {
                user_interest_shadow::run_and_log(&pool).await;
            }
            match outcome {
                SweepOutcome::Ran(r) => info!(
                    dirty_users = r.dirty_users,
                    signal_rows = r.signal_rows,
                    since = %r.since,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "topic interest swept"
                ),
                SweepOutcome::Skipped => {
                    info!("topic interest sweep skipped: the nightly refit is running")
                }
            }
        }
        Mode::Refit => {
            // Best-effort copy of the co-occurrence table into geo-db's derived class
            // (GEODB_DERIVED_MIRROR, default off). With it off `from_env` is None and the table is
            // never read a second time. The sweep never writes the table, so only the refit mirrors.
            let mirror = geodb_cursor::derived::DerivedMirror::from_env();
            let prior: Vec<(uuid::Uuid, uuid::Uuid)> = if mirror.is_some() {
                sqlx::query_as(topic_cooccurrence_mirror::PRIOR_SQL)
                    .fetch_all(&pool)
                    .await?
            } else {
                Vec::new()
            };
            let signals = signals_mirror::from_env_private();
            let signals_prior = read_idents(&pool, signals.as_ref()).await;
            let r = topic_interest::refit(&pool, None).await?;
            if let (Some(m), Some(prior)) = (&signals, signals_prior) {
                mirror_signals(&pool, m, &prior, signals_mirror::refit_since()).await;
            }
            if cooc_rust_enabled() {
                match cooc_shadow(&pool).await {
                    Ok((0, n)) => info!(rows = n, "co-occurrence rust compute matches the SQL"),
                    Ok((bad, n)) => warn!(
                        mismatched = bad,
                        rows = n,
                        "co-occurrence rust compute differs from the SQL"
                    ),
                    Err(e) => warn!(error = %e, "co-occurrence rust compute skipped"),
                }
            }
            if let Some(mirror) = &mirror {
                // Read after the refit committed; a read error skips the mirror, never the job.
                match sqlx::query_as::<_, Neighbour>(topic_cooccurrence_mirror::SELECT_SQL)
                    .fetch_all(&pool)
                    .await
                {
                    Ok(rows) => {
                        topic_cooccurrence_mirror::mirror(mirror, &rows, &prior).await;
                        info!(
                            rows = rows.len(),
                            prior = prior.len(),
                            "topic co-occurrence mirrored to geo-db"
                        );
                    }
                    Err(e) => warn!(error = %e, "topic co-occurrence mirror skipped: read failed"),
                }
            }
            let elapsed_ms = started.elapsed().as_millis() as u64;
            if r.disagreeing_rows > 0 {
                // Expected occasionally (a claim re-tagged after people voted on it, a deleted
                // comment or follow, indexing lag past the lookback); the refit has already fixed
                // it. A count that stays high night after night means the sweep is missing a kind
                // of change.
                warn!(
                    cooccurrence_rows = r.cooccurrence_rows,
                    users = r.users,
                    signal_rows = r.signal_rows,
                    disagreeing_rows = r.disagreeing_rows,
                    max_abs_diff = r.max_abs_diff,
                    elapsed_ms,
                    "topic interest refit corrected incremental drift"
                );
            } else {
                info!(
                    cooccurrence_rows = r.cooccurrence_rows,
                    users = r.users,
                    signal_rows = r.signal_rows,
                    elapsed_ms,
                    "topic interest refit agreed with the incremental state"
                );
            }
        }
    }
    Ok(())
}

/// `TOPIC_COOCCURRENCE_RUST_COMPUTE` (default off): after the refit, compute the co-occurrence table
/// again in code from `entity_topic_ranking` and log how many rows differ from what the SQL wrote.
/// It never changes the run: a failure is logged and the job carries on.
fn cooc_rust_enabled() -> bool {
    matches!(
        env::var("TOPIC_COOCCURRENCE_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

async fn cooc_shadow(pool: &sqlx::PgPool) -> Result<(usize, usize), IndexerError> {
    let (min_support, max_topics, max_neighbours): (i32, i32, i32) = sqlx::query_as(
        "SELECT cooccurrence_min_support, cooccurrence_max_topics_per_entity, spread_max_neighbours \
         FROM personalization.interest_config",
    )
    .fetch_one(pool)
    .await?;
    let tags: Vec<(uuid::Uuid, uuid::Uuid)> =
        sqlx::query_as("SELECT DISTINCT topic_id, entity_id FROM public.entity_topic_ranking")
            .fetch_all(pool)
            .await?;
    let want: Vec<Neighbour> = sqlx::query_as(topic_cooccurrence_mirror::SELECT_SQL)
        .fetch_all(pool)
        .await?;
    let got = cooc::compute(
        &tags,
        cooc::Params {
            min_support: min_support.into(),
            max_topics_per_entity: max_topics.into(),
            max_neighbours: max_neighbours.into(),
        },
    );
    let want: Vec<cooc::Neighbour> = want
        .into_iter()
        .map(|n| cooc::Neighbour {
            topic_id: n.topic_id,
            neighbour_id: n.neighbour_id,
            similarity: n.similarity,
            support: n.support,
        })
        .collect();
    Ok((cooc::count_mismatches(&got, &want), want.len()))
}

/// The identities `user_topic_signals` holds now, or `None` when there is no mirror or the read
/// failed (the mirror is then skipped; it never stops the job).
async fn read_idents(pool: &sqlx::PgPool, mirror: Option<&DerivedMirror>) -> Option<Vec<Ident>> {
    mirror?;
    match sqlx::query_as::<_, Ident>(signals_mirror::IDENT_SQL)
        .fetch_all(pool)
        .await
    {
        Ok(rows) => Some(rows),
        Err(e) => {
            warn!(error = %e, "user topic signals mirror skipped: identity read failed");
            None
        }
    }
}

/// Reads the rows computed at or after `since` and the identities after the run, and sends the plan.
async fn mirror_signals(
    pool: &sqlx::PgPool,
    mirror: &DerivedMirror,
    prior: &[Ident],
    since: chrono::DateTime<chrono::Utc>,
) {
    let rows = sqlx::query_as::<_, Signal>(signals_mirror::SELECT_SQL)
        .bind(since)
        .fetch_all(pool)
        .await;
    let after = sqlx::query_as::<_, Ident>(signals_mirror::IDENT_SQL)
        .fetch_all(pool)
        .await;
    match (rows, after) {
        (Ok(rows), Ok(after)) => {
            signals_mirror::mirror(mirror, &rows, prior, &after).await;
            info!(
                rows = rows.len(),
                prior = prior.len(),
                "user topic signals mirrored to geo-db"
            );
        }
        (Err(e), _) | (_, Err(e)) => {
            warn!(error = %e, "user topic signals mirror skipped: read failed")
        }
    }
}
