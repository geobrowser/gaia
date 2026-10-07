//! Hourly analytics -> gaia feed signals (GEO-3235, migration 0109). See
//! `ranking_indexer::feed_signals` for what is read and how the window is chosen.
//!
//! A run: read analytics' watermark; if it has not moved since the last success, log `unchanged`
//! and stop. Otherwise read the per-item and per-user views for the window gaia asks for and
//! replace it, with retention and the run log, in one transaction.
//!
//! Exits non-zero WITHOUT writing signals (and logs a `failed` run) when analytics is unreachable,
//! a row is malformed, the wallet hash no longer matches analytics', or the SQL refuses: an empty
//! item read over a window that holds rows, or user rows that place on no personal space while rows
//! exist (`--allow-empty` overrides both).
//!
//! `--dry-run` (or FEED_SIGNALS_DRY_RUN=1) does the whole replacement and rolls it back, so the
//! counts it prints are exactly what a real run would write. It logs nothing.
//!
//! `--check-freshness` reads only gaia and exits non-zero when the serving tables are stale (no
//! successful run for 3 hours, or the newest data read is over 6 hours old; both in
//! personalization.feed_signal_config). Its CronJob failing is the alert (KubeJobFailed).
//!
//! Usage:
//!   DATABASE_URL=... CLICKHOUSE_URL=... CLICKHOUSE_USER=... CLICKHOUSE_PASSWORD=... \
//!     feed_signals_sync [--dry-run] [--allow-empty]
//!   DATABASE_URL=... feed_signals_sync --check-freshness

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::{error, info};

use ranking_indexer::error::IndexerError;
use ranking_indexer::feed_signals::{self, Cutoff};

#[derive(Debug, Default, PartialEq)]
struct Options {
    dry_run: bool,
    allow_empty: bool,
    check_freshness: bool,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, IndexerError> {
    let mut options = Options {
        dry_run: matches!(
            env::var("FEED_SIGNALS_DRY_RUN").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        ),
        ..Options::default()
    };
    for arg in args {
        match arg.as_str() {
            "--dry-run" => options.dry_run = true,
            "--allow-empty" => options.allow_empty = true,
            "--check-freshness" => options.check_freshness = true,
            other => {
                return Err(IndexerError::Config(format!(
                    "unknown argument {other}; usage: feed_signals_sync [--dry-run] [--allow-empty] | --check-freshness"
                )))
            }
        }
    }
    Ok(options)
}

async fn check_freshness(pool: &PgPool) -> Result<(), IndexerError> {
    let f = feed_signals::freshness(pool).await?;
    info!(
        last_run_at = ?f.last_run_at,
        data_through = ?f.data_through,
        stale = f.stale,
        "feed signals freshness"
    );
    if f.stale {
        return Err(IndexerError::Config(format!(
            "feed signals are stale: {}",
            f.reason.unwrap_or_default()
        )));
    }
    Ok(())
}

async fn sync(pool: &PgPool, options: &Options, cutoff: &Cutoff) -> Result<(), IndexerError> {
    let started = Instant::now();
    let data_through = cutoff.data_through()?;
    let window = feed_signals::window(pool, data_through).await?;
    if window.unchanged {
        info!(
            generation = %cutoff.generation,
            data_through = %data_through,
            "analytics has published nothing new; nothing to do"
        );
        if !options.dry_run {
            feed_signals::record(pool, "unchanged", Some(cutoff), None).await?;
        }
        return Ok(());
    }
    let items = feed_signals::read_items(window.window_start, window.window_end).await?;
    let users = feed_signals::read_users(window.window_start, window.window_end).await?;
    let r = feed_signals::apply(
        pool,
        &cutoff.generation,
        &window,
        &items,
        &users,
        options.allow_empty,
        options.dry_run,
    )
    .await?;
    info!(
        generation = %cutoff.generation,
        window_start = %window.window_start,
        data_through = %window.window_end,
        previous_data_through = ?window.last_data_through,
        item_rows = r.item_rows,
        user_rows_in = r.user_rows_in,
        user_rows_placed = r.user_rows_placed,
        user_rows_unplaced = r.user_rows_in - r.user_rows_placed,
        user_rows = r.user_rows,
        item_rows_expired = r.item_rows_expired,
        user_rows_expired = r.user_rows_expired,
        dry_run = options.dry_run,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "feed signals replaced"
    );
    if options.dry_run {
        println!(
            "Dry run, rolled back: generation {} through {}, window from {}: {} item-hours, {} of {} per-user rows on a personal space ({} user-days written), {} item and {} per-user rows past retention",
            cutoff.generation,
            window.window_end,
            window.window_start,
            r.item_rows,
            r.user_rows_placed,
            r.user_rows_in,
            r.user_rows,
            r.item_rows_expired,
            r.user_rows_expired
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let options = parse_args(env::args().skip(1))?;

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    if options.check_freshness {
        return check_freshness(&pool).await;
    }

    feed_signals::check_wallet_hash(&pool).await?;
    let cutoff = match feed_signals::read_cutoff().await {
        Ok(c) => c,
        Err(e) => {
            if !options.dry_run {
                let _ = feed_signals::record(&pool, "failed", None, Some(&e.to_string())).await;
            }
            return Err(e);
        }
    };
    if let Err(e) = sync(&pool, &options, &cutoff).await {
        error!(error = %e, "feed signals run failed; nothing was written");
        if !options.dry_run {
            // Best effort: the run log is how a failure shows up next to the successes.
            let _ =
                feed_signals::record(&pool, "failed", Some(&cutoff), Some(&e.to_string())).await;
        }
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let o = parse_args(["--dry-run", "--allow-empty"].into_iter().map(String::from)).unwrap();
        assert!(o.dry_run && o.allow_empty && !o.check_freshness);
        assert!(
            parse_args(["--check-freshness".to_string()].into_iter())
                .unwrap()
                .check_freshness
        );
        assert!(parse_args(["--bogus".to_string()].into_iter()).is_err());
    }
}
