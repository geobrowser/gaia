//! Learns each user's topic interests from what they already do (GEO-3088, migration 0100).
//!
//! `topic_interest sweep` runs every two minutes: it recomputes every user with activity since the
//! last run (votes, comments, debates, follows), so a new vote moves that user's weights within
//! minutes. `topic_interest refit` runs nightly: it rebuilds topic co-occurrence, recomputes every
//! user from scratch, and records how far the incremental state had drifted in
//! `personalization.interest_refit_runs`.
//!
//! Both are separate CronJobs from the vote-indexer, deliberately: since GEO-3101 the vote-indexer
//! halts on a failed write, and nothing about interest learning may be able to stop vote indexing.
//!
//! Usage: `DATABASE_URL=... cargo run --bin topic_interest -- sweep|refit`

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::topic_interest::{self, Mode, SweepOutcome};

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
        Mode::Sweep => match topic_interest::sweep(&pool, None).await? {
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
        },
        Mode::Refit => {
            let r = topic_interest::refit(&pool, None).await?;
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
