//! Per-user topic interest (GEO-3088, migration 0100): the schedule around the SQL.
//!
//! The computation lives in SQL, in the `personalization` schema, the same way the topic ranking
//! (0096) and topic suggestions (0095) do: it is a set-based read over `user_votes` and the graph,
//! and keeping it in one place is what lets the incremental sweep and the nightly refit share one
//! definition, so they can only disagree by what the sweep failed to notice. This module runs those
//! functions with the right timeout and transaction, and turns their results into a report.
//!
//! Nothing here touches the vote-indexer: its own CronJobs, its own tables, read-only on
//! `user_votes`. A failure here cannot stop or delay vote indexing.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::IndexerError;

/// Which pass a run performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every couple of minutes: recompute the users with new activity.
    Sweep,
    /// Nightly: rebuild topic co-occurrence, recompute everyone, and report disagreement with
    /// what the sweeps left.
    Refit,
}

impl FromStr for Mode {
    type Err = IndexerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sweep" => Ok(Mode::Sweep),
            "refit" => Ok(Mode::Refit),
            other => Err(IndexerError::Config(format!(
                "unknown topic_interest mode {other:?}: expected \"sweep\" or \"refit\""
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SweepReport {
    pub dirty_users: i32,
    pub signal_rows: i32,
    /// Activity at or after this time was looked for (the last run, less the lookback).
    pub since: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SweepOutcome {
    Ran(SweepReport),
    /// The nightly refit held the lock. The refit covers everyone, so nothing is lost, and the
    /// cursor did not move, so the next sweep looks back from the same point.
    Skipped,
}

impl SweepOutcome {
    /// The SQL returns NULL `dirty_users` when it could not take the lock.
    fn from_row(dirty_users: Option<i32>, signal_rows: i32, since: Option<DateTime<Utc>>) -> Self {
        match (dirty_users, since) {
            (Some(dirty_users), Some(since)) => SweepOutcome::Ran(SweepReport {
                dirty_users,
                signal_rows,
                since,
            }),
            _ => SweepOutcome::Skipped,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefitReport {
    pub cooccurrence_rows: i32,
    pub users: i32,
    pub signal_rows: i32,
    /// Rows where the incremental state differed from the fresh recompute. Zero is the normal
    /// case; a steady non-zero count means the sweep is missing a kind of change.
    pub disagreeing_rows: i32,
    pub max_abs_diff: f64,
}

/// The app role's `statement_timeout` is 30s. A first sweep is a full backfill and the refit
/// always is, so each run sets its own limit. `SET LOCAL` inside the run's transaction rather than
/// a session `SET`: the connection may go through a transaction-mode pooler.
const SWEEP_TIMEOUT: &str = "SET LOCAL statement_timeout = '5min'";
const REFIT_TIMEOUT: &str = "SET LOCAL statement_timeout = '15min'";

/// One incremental pass, in one transaction: the cursor moves only if the recompute commits.
/// `as_of` is for tests; the CronJob passes `None` (the database's `now()`).
pub async fn sweep(
    pool: &PgPool,
    as_of: Option<DateTime<Utc>>,
) -> Result<SweepOutcome, IndexerError> {
    let mut tx = pool.begin().await?;
    sqlx::query(SWEEP_TIMEOUT).execute(&mut *tx).await?;
    let (dirty_users, signal_rows, since): (Option<i32>, i32, Option<DateTime<Utc>>) =
        sqlx::query_as(
            "SELECT dirty_users, signal_rows, since \
         FROM personalization.sweep_user_topic_interest(coalesce($1, now()))",
        )
        .bind(as_of)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(SweepOutcome::from_row(dirty_users, signal_rows, since))
}

/// The nightly pass: co-occurrence first, so the refit's spreading reads tonight's neighbours,
/// then everyone from scratch. One transaction, so a failure leaves yesterday's state whole.
pub async fn refit(
    pool: &PgPool,
    as_of: Option<DateTime<Utc>>,
) -> Result<RefitReport, IndexerError> {
    let mut tx = pool.begin().await?;
    sqlx::query(REFIT_TIMEOUT).execute(&mut *tx).await?;
    let cooccurrence_rows: i32 =
        sqlx::query_scalar("SELECT personalization.refresh_topic_cooccurrence()")
            .fetch_one(&mut *tx)
            .await?;
    let (users, signal_rows, disagreeing_rows, max_abs_diff): (i32, i32, i32, f64) =
        sqlx::query_as(
            "SELECT users, signal_rows, disagreeing_rows, max_abs_diff \
         FROM personalization.refit_user_topic_interest(coalesce($1, now()), $2)",
        )
        .bind(as_of)
        .bind(cooccurrence_rows)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(RefitReport {
        cooccurrence_rows,
        users,
        signal_rows,
        disagreeing_rows,
        max_abs_diff,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_the_two_cronjob_arguments() {
        assert_eq!("sweep".parse::<Mode>().unwrap(), Mode::Sweep);
        assert_eq!("refit".parse::<Mode>().unwrap(), Mode::Refit);
    }

    #[test]
    fn an_unknown_mode_is_a_config_error_not_a_default() {
        // A typo in the CronJob must fail the Job, not silently run the other pass.
        assert!(matches!(
            "Sweep".parse::<Mode>(),
            Err(IndexerError::Config(_))
        ));
        assert!(matches!("".parse::<Mode>(), Err(IndexerError::Config(_))));
    }

    #[test]
    fn a_null_dirty_count_means_the_refit_held_the_lock() {
        assert_eq!(SweepOutcome::from_row(None, 0, None), SweepOutcome::Skipped);
        let since = Utc::now();
        assert_eq!(
            SweepOutcome::from_row(Some(3), 12, Some(since)),
            SweepOutcome::Ran(SweepReport {
                dirty_users: 3,
                signal_rows: 12,
                since
            })
        );
    }
}
