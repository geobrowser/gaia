//! Behaviour signals from analytics into gaia (GEO-3235, migration 0109; analytics migration 107).
//!
//! Analytics publishes three ClickHouse views over the dashboards' bot-filtered event set
//! (`events_for_metrics`):
//!   * `feed_signal_cutoff`: the latest published input generation and its cutoff, the watermark;
//!   * `feed_signal_items(window_start, window_end)`: per Explore item and UTC hour, impressions
//!     (and by position band), opens, votes from Explore, debate plays and watch time;
//!   * `feed_signal_user_items(window_start, window_end)`: per signed-in user's wallet hash, item
//!     and UTC day, impressions on Explore and when last.
//!
//! A run asks gaia for the window to replace (`personalization.feed_signal_window`: at least a week
//! before the watermark, back to the last success after an outage, at most 30 days), reads both
//! views for exactly that window, and hands them to `personalization.replace_feed_signals`, which
//! replaces every bucket from the window start, applies retention and logs the run in one
//! transaction. Replace, never add: a rerun writes the same rows and a missed hour is in the next
//! window. When the watermark has not moved, the run records `unchanged` and reads nothing else.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::clickhouse;
use crate::error::IndexerError;

pub const CUTOFF_QUERY: &str =
    "SELECT generation, data_through FROM analytics.feed_signal_cutoff FORMAT JSONEachRow";

/// The item and user views scan a week of Explore events; generous, but bounded.
pub const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// The app role's statement_timeout is 30s; a first run writes 30 days. `SET LOCAL`, inside the
/// transaction, in case the connection goes through a pooler.
const APPLY_TIMEOUT: &str = "SET LOCAL statement_timeout = '5min'";

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Cutoff {
    pub generation: String,
    pub data_through: String,
}

impl Cutoff {
    pub fn data_through(&self) -> Result<DateTime<Utc>, IndexerError> {
        DateTime::parse_from_rfc3339(&self.data_through)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| {
                IndexerError::Config(format!(
                    "analytics cutoff {:?} is not a time: {e}",
                    self.data_through
                ))
            })
    }
}

/// One row of `analytics.feed_signal_items`, passed through to Postgres as is.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ItemRow {
    pub item_id: String,
    pub hour: String,
    pub impressions: u32,
    pub impressions_top3: u32,
    pub impressions_4_10: u32,
    pub impressions_11_30: u32,
    pub impressions_31_plus: u32,
    pub opens: u32,
    pub votes: u32,
    pub plays: u32,
    pub explore_plays: u32,
    pub watch_ms: u64,
    pub explore_watch_ms: u64,
    pub plays_with_duration: u32,
    pub watch_fraction_sum: f64,
    pub completed_plays: u32,
}

/// One row of `analytics.feed_signal_user_items`. Only a wallet hash identifies the user.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct UserRow {
    pub wallet_address_hash: String,
    pub item_id: String,
    pub day: String,
    pub impressions: u32,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Window {
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub last_data_through: Option<DateTime<Utc>>,
    pub unchanged: bool,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ApplyReport {
    pub run_id: i64,
    pub item_rows: i32,
    pub user_rows_in: i32,
    pub user_rows_placed: i32,
    pub user_rows: i32,
    pub item_rows_expired: i32,
    pub user_rows_expired: i32,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Freshness {
    pub last_run_at: Option<DateTime<Utc>>,
    pub data_through: Option<DateTime<Utc>>,
    pub stale: bool,
    pub reason: Option<String>,
}

fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The read of one parameterized view for [start, end). The times are formatted here, never taken
/// from input, so nothing reaches the SQL text but two RFC 3339 timestamps.
pub fn window_query(view: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    format!(
        "SELECT * FROM analytics.{view}(window_start='{}', window_end='{}') FORMAT JSONEachRow",
        iso(start),
        iso(end)
    )
}

/// The latest published generation. Exactly one row, or the view is broken.
pub fn parse_cutoff(body: &str) -> Result<Cutoff, IndexerError> {
    let mut rows: Vec<Cutoff> = clickhouse::parse_json_each_row(body, "cutoff")?;
    if rows.len() != 1 {
        return Err(IndexerError::Config(format!(
            "analytics.feed_signal_cutoff returned {} rows, expected 1 (has the dashboard refresh ever published?)",
            rows.len()
        )));
    }
    Ok(rows.remove(0))
}

pub async fn read_cutoff() -> Result<Cutoff, IndexerError> {
    parse_cutoff(&clickhouse::query(CUTOFF_QUERY, READ_TIMEOUT).await?)
}

pub async fn read_items(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<ItemRow>, IndexerError> {
    let body =
        clickhouse::query(&window_query("feed_signal_items", start, end), READ_TIMEOUT).await?;
    clickhouse::parse_json_each_row(&body, "feed item")
}

pub async fn read_users(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<UserRow>, IndexerError> {
    let body = clickhouse::query(
        &window_query("feed_signal_user_items", start, end),
        READ_TIMEOUT,
    )
    .await?;
    clickhouse::parse_json_each_row(&body, "feed user")
}

/// Fails unless gaia hashes a wallet address exactly as analytics does.
pub async fn check_wallet_hash(pool: &PgPool) -> Result<(), IndexerError> {
    let known: String = sqlx::query_scalar("SELECT personalization.wallet_address_hash($1)")
        .bind(clickhouse::KNOWN_ADDRESS)
        .fetch_one(pool)
        .await?;
    if known != clickhouse::KNOWN_HASH {
        return Err(IndexerError::Config(format!(
            "gaia's wallet hash ({known}) no longer matches analytics' ({})",
            clickhouse::KNOWN_HASH
        )));
    }
    Ok(())
}

pub async fn window(pool: &PgPool, data_through: DateTime<Utc>) -> Result<Window, IndexerError> {
    Ok(sqlx::query_as::<_, Window>(
        "SELECT window_start, window_end, last_data_through, unchanged \
         FROM personalization.feed_signal_window($1)",
    )
    .bind(data_through)
    .fetch_one(pool)
    .await?)
}

/// Replaces the window with `items` and `users` in one transaction. `dry_run` does all of it and
/// rolls back, so the report is exactly what a real run would write.
pub async fn apply(
    pool: &PgPool,
    generation: &str,
    window: &Window,
    items: &[ItemRow],
    users: &[UserRow],
    allow_empty: bool,
    dry_run: bool,
) -> Result<ApplyReport, IndexerError> {
    let items = serde_json::to_value(items).map_err(|e| IndexerError::Config(e.to_string()))?;
    let users = serde_json::to_value(users).map_err(|e| IndexerError::Config(e.to_string()))?;
    let mut tx = pool.begin().await?;
    sqlx::query(APPLY_TIMEOUT).execute(&mut *tx).await?;
    let report = sqlx::query_as::<_, ApplyReport>(
        "SELECT run_id, item_rows, user_rows_in, user_rows_placed, user_rows, item_rows_expired, user_rows_expired \
         FROM personalization.replace_feed_signals($1, $2, $3, $4::jsonb, $5::jsonb, $6)",
    )
    .bind(generation)
    .bind(window.window_start)
    .bind(window.window_end)
    .bind(items)
    .bind(users)
    .bind(allow_empty)
    .fetch_one(&mut *tx)
    .await?;
    if dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(report)
}

/// Logs a run that wrote no signals ('unchanged' or 'failed').
pub async fn record(
    pool: &PgPool,
    status: &str,
    cutoff: Option<&Cutoff>,
    error: Option<&str>,
) -> Result<(), IndexerError> {
    let data_through = cutoff.and_then(|c| c.data_through().ok());
    sqlx::query("SELECT personalization.record_feed_signal_run($1, $2, $3, $4)")
        .bind(status)
        .bind(cutoff.map(|c| c.generation.as_str()))
        .bind(data_through)
        .bind(error)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn freshness(pool: &PgPool) -> Result<Freshness, IndexerError> {
    Ok(sqlx::query_as::<_, Freshness>(
        "SELECT last_run_at, data_through, stale, reason FROM personalization.feed_signal_freshness()",
    )
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn window_queries_carry_only_formatted_times() {
        assert_eq!(
            window_query("feed_signal_items", at("2026-09-29T00:00:00Z"), at("2026-10-06T12:00:00.123Z")),
            "SELECT * FROM analytics.feed_signal_items(window_start='2026-09-29T00:00:00Z', window_end='2026-10-06T12:00:00Z') FORMAT JSONEachRow"
        );
    }

    #[test]
    fn the_cutoff_is_exactly_one_row_with_a_time() {
        let c = parse_cutoff("{\"generation\":\"g2\",\"data_through\":\"2026-10-06T12:00:00Z\",\"published_at\":\"2026-10-06T12:31:00Z\"}\n").unwrap();
        assert_eq!(c.generation, "g2");
        assert_eq!(c.data_through().unwrap(), at("2026-10-06T12:00:00Z"));
        assert!(parse_cutoff("").is_err(), "no generation published");
        assert!(parse_cutoff("{\"generation\":\"a\",\"data_through\":\"x\"}\n{\"generation\":\"b\",\"data_through\":\"x\"}").is_err());
        let bad = parse_cutoff("{\"generation\":\"g\",\"data_through\":\"2026-10-06 12:00:00\"}")
            .unwrap();
        assert!(
            bad.data_through().is_err(),
            "a time without a zone is refused, not guessed"
        );
    }

    #[test]
    fn item_and_user_rows_round_trip_as_analytics_prints_them() {
        let line = "{\"item_id\":\"00000001000040008000000000000000\",\"hour\":\"2026-10-06T10:00:00Z\",\"impressions\":5,\"impressions_top3\":1,\"impressions_4_10\":1,\"impressions_11_30\":1,\"impressions_31_plus\":1,\"opens\":1,\"votes\":1,\"plays\":2,\"explore_plays\":1,\"watch_ms\":180000,\"explore_watch_ms\":80000,\"plays_with_duration\":2,\"watch_fraction_sum\":1.6,\"completed_plays\":1}";
        let rows: Vec<ItemRow> = clickhouse::parse_json_each_row(line, "item").unwrap();
        assert_eq!(rows[0].watch_ms, 180000);
        let back = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(line).unwrap()
        );

        let user = "{\"wallet_address_hash\":\"sha256:ab\",\"item_id\":\"00000001000040008000000000000000\",\"day\":\"2026-10-06\",\"impressions\":5,\"last_seen_at\":\"2026-10-06T10:05:00Z\"}";
        let rows: Vec<UserRow> = clickhouse::parse_json_each_row(user, "user").unwrap();
        assert_eq!(rows[0].impressions, 5);
        // A quoted count (ClickHouse's 64-bit default) is refused rather than coerced.
        assert!(clickhouse::parse_json_each_row::<UserRow>(
            &user.replace("\"impressions\":5", "\"impressions\":\"5\""),
            "user"
        )
        .is_err());
    }
}
