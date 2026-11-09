//! The `RANKING_INDEXER_PG_WRITES` switch for the periodic binaries.
//!
//! Each CronJob binary calls a plpgsql function that writes Postgres. With the flag off the call is
//! never started: [`run`] drops the future unpolled, so no statement (not even `BEGIN`) is sent, and
//! it counts the skip under the call's site label. A binary wraps the whole write in one `run`,
//! a transaction included, then carries on with whatever is not Postgres (geo-db mirrors, shadows
//! that do not depend on the write's result).

use crate::metrics::{pg_write_skipped, PgWriteSite};
use crate::storage::pg_writes_enabled_from;
use std::future::Future;

/// Whether `RANKING_INDEXER_PG_WRITES` is on in this process (default on).
pub fn pg_writes_on() -> bool {
    pg_writes_enabled_from(std::env::var("RANKING_INDEXER_PG_WRITES").ok().as_deref())
}

/// Runs `write` when `on`; otherwise drops it unpolled, counts the skip and returns `Ok(None)`.
pub async fn run_if<T, E>(
    on: bool,
    site: PgWriteSite,
    write: impl Future<Output = Result<T, E>>,
) -> Result<Option<T>, E> {
    if !on {
        pg_write_skipped(site);
        tracing::info!(
            site = site.as_str(),
            "RANKING_INDEXER_PG_WRITES is off: Postgres write skipped"
        );
        return Ok(None);
    }
    write.await.map(Some)
}

/// [`run_if`] with the switch read from the environment.
pub async fn run<T, E>(
    site: PgWriteSite,
    write: impl Future<Output = Result<T, E>>,
) -> Result<Option<T>, E> {
    run_if(pg_writes_on(), site, write).await
}
