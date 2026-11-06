//! One-shot backfill and read-back compare of geo-db derived collections 27
//! (`external_interest_signals`), 28 (`user_topic_signals`) and 29 (`feed_user_seen_daily`).
//! See `ranking_indexer::private_snapshot` for why, and RUNBOOK section 2f-private for when.
//!
//!   private_mirror_snapshot backfill   put every Postgres row (last-write-wins; safe to repeat)
//!   private_mirror_snapshot compare    read every user's group back and count differences
//!
//! Both need `DATABASE_URL`, `GEODB_DERIVED_MIRROR=1`, `GEODB_DERIVED_MIRROR_PRIVATE=1` and the
//! geodb-client wiring (`GEODB_CLIENT_BIN`, `GEODB_KEY_PATH`, `GEODB_BINDING_PATH`); backfill wants
//! `GEODB_DERIVED_BATCH=1`. `PRIVATE_SNAPSHOT_TABLES` (comma separated labels, default all three)
//! picks tables. Neither mode deletes anything. Logs counts only, never an id.
//!
//! `backfill` exits non-zero when a page was not fully acknowledged after retries. `compare` exits
//! non-zero when it could not read, and with code 2 when it read and found a difference.

use std::collections::BTreeSet;
use std::env;
use std::time::Duration;

use geodb_cursor::derived::{DerivedMirror, DerivedRow, BATCH_MAX};
use sqlx::PgPool;
use tracing::{error, info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::private_snapshot::{by_group, compare_group, ScanPipe, Tally};
use ranking_indexer::user_topic_signals_mirror as topic;
use ranking_indexer::{external_interest_mirror as ext, feed_user_seen_mirror as seen};

const TABLES: [&str; 3] = [
    "external_interest_signals",
    "user_topic_signals",
    "feed_user_seen_daily",
];
const KINDS: [&str; 6] = [
    "vote",
    "comment",
    "debate",
    "follow",
    "interested",
    "not_interested",
];
const ATTEMPTS: u32 = 3;
/// Longer than the mirror's cooldown after a pipe error, so the retry is not refused outright.
const RETRY_WAIT: Duration = Duration::from_secs(65);

/// The expected geo-db rows of one table, planned with the same functions the writers use.
async fn expected(pool: &PgPool, table: &str) -> Result<Vec<DerivedRow>, sqlx::Error> {
    Ok(match table {
        "external_interest_signals" => {
            let mut out = Vec::new();
            for kind in KINDS {
                let rows: Vec<ext::Signal> = sqlx::query_as(ext::SELECT_SQL)
                    .bind(kind)
                    .fetch_all(pool)
                    .await?;
                out.extend(ext::plan(&rows, &[]).0);
            }
            out
        }
        "user_topic_signals" => {
            let rows: Vec<topic::Signal> = sqlx::query_as(topic::SELECT_SQL)
                .bind(topic::refit_since())
                .fetch_all(pool)
                .await?;
            topic::plan(&rows, &[], &[]).0
        }
        _ => {
            let rows: Vec<seen::Seen> = sqlx::query_as(seen::SELECT_SQL).fetch_all(pool).await?;
            seen::plan(&rows, &[]).0
        }
    })
}

async fn send(mirror: &DerivedMirror, table: &str, rows: &[DerivedRow]) -> bool {
    for attempt in 1..=ATTEMPTS {
        let sent = mirror.put_counted(table, rows).await;
        if sent == rows.len() {
            return true;
        }
        warn!(
            table,
            attempt,
            sent,
            expected = rows.len(),
            "private snapshot: short send"
        );
        if attempt < ATTEMPTS {
            tokio::time::sleep(RETRY_WAIT).await;
        }
    }
    false
}

fn scan_pipe() -> Result<ScanPipe, String> {
    let cfg = geodb_cursor::Config::from_vars_for("GEODB_DERIVED_MIRROR", |k| env::var(k).ok())
        .ok_or("GEODB_DERIVED_MIRROR is off or the key and binding paths are unset")?;
    ScanPipe::spawn(&cfg.program, &cfg.args)
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();
    let mode = env::args().nth(1).unwrap_or_default();
    if mode != "backfill" && mode != "compare" {
        return Err(IndexerError::Config(
            "usage: private_mirror_snapshot backfill|compare".into(),
        ));
    }
    if !ext::private_enabled() {
        return Err(IndexerError::Config(
            "GEODB_DERIVED_MIRROR_PRIVATE is not 1".into(),
        ));
    }
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let mirror = DerivedMirror::from_env().ok_or_else(|| {
        IndexerError::Config("geo-db derived mirror is off or unconfigured".into())
    })?;
    let wanted = env::var("PRIVATE_SNAPSHOT_TABLES").unwrap_or_default();
    let wanted: BTreeSet<&str> = wanted
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(bad) = wanted.iter().find(|n| !TABLES.contains(n)) {
        return Err(IndexerError::Config(format!(
            "PRIVATE_SNAPSHOT_TABLES names an unknown table: {bad}"
        )));
    }
    let pool = PgPool::connect(&database_url)
        .await
        .map_err(|e| IndexerError::Config(format!("database connect failed: {e}")))?;
    let mut pipe = if mode == "compare" {
        Some(scan_pipe().map_err(IndexerError::Config)?)
    } else {
        None
    };
    info!(mode = %mode, "private snapshot: starting");

    let (mut failed, mut differs) = (false, false);
    for (table, collection) in
        TABLES
            .iter()
            .zip([ext::COLLECTION, topic::COLLECTION, seen::COLLECTION])
    {
        if !wanted.is_empty() && !wanted.contains(table) {
            continue;
        }
        let rows = match expected(&pool, table).await {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, table = *table, "private snapshot: read failed");
                std::process::exit(1);
            }
        };
        if let Some(pipe) = pipe.as_mut() {
            let groups = by_group(rows);
            let mut total = Tally::default();
            let mut errors = 0u64;
            for (group, want) in &groups {
                // A blocking pipe round trip; fine for a one-shot Job.
                match pipe.scan(collection, *group) {
                    Ok((held, more)) => total.add(compare_group(want, &held, more)),
                    Err(e) => {
                        errors += 1;
                        if errors == 1 {
                            error!(error = %e, table = *table, "private snapshot: scan failed");
                        }
                        if errors >= 5 {
                            break;
                        }
                    }
                }
            }
            info!(
                table = *table,
                users = groups.len(),
                matched = total.matched,
                mismatched = total.mismatched,
                missing = total.missing,
                extra = total.extra,
                partial_groups = total.partial_groups,
                scan_errors = errors,
                "private snapshot: compare"
            );
            failed |= errors > 0;
            differs |= !total.clean();
        } else {
            let mut sent = 0u64;
            for chunk in rows.chunks(BATCH_MAX) {
                if !send(&mirror, table, chunk).await {
                    error!(
                        table = *table,
                        rows_sent = sent,
                        "private snapshot: incomplete"
                    );
                    std::process::exit(1);
                }
                sent += chunk.len() as u64;
            }
            info!(
                table = *table,
                collection,
                rows_sent = sent,
                "private snapshot: table done"
            );
        }
    }
    if failed {
        std::process::exit(1);
    }
    if differs {
        std::process::exit(2);
    }
    Ok(())
}
