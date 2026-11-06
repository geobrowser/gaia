//! One-shot backfill of geo-db derived collections 2 (`global_scores`), 3 (`local_scores`) and 4
//! (`space_scores`) from Postgres. See `vote_indexer::scores_snapshot` for why, and RUNBOOK
//! section 2f-scores for when to run it (Stage B: after the 12,582,912 cap is live).
//!
//! Needs `DATABASE_URL`, `GEODB_DERIVED_MIRROR=1` and the geodb-client wiring (`GEODB_CLIENT_BIN`,
//! `GEODB_KEY_PATH`, `GEODB_BINDING_PATH`); set `GEODB_DERIVED_BATCH=1`. Optional
//! `SCORES_SNAPSHOT_TABLES` (comma separated, default all three) picks tables, in the order
//! `space_scores`, `global_scores`, `local_scores` (small first). Idempotent. Logs counts only.
//! Exits non-zero if any page could not be fully acknowledged after retries, so a Job is marked
//! failed rather than silently partial.

use std::env;
use std::future::Future;
use std::time::Duration;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use hermes_instrumentation::{error, info, warn, Backend, Config};
use sqlx::PgPool;
use uuid::Uuid;

use vote_indexer::error::IndexerError;
use vote_indexer::scores_snapshot::{
    global_page, local_page, space_page, Page, GLOBAL_COLLECTION, LOCAL_COLLECTION, PAGE_SIZE,
    SPACE_COLLECTION,
};

/// Log progress every this many pages (about 25,600 rows).
const PROGRESS_EVERY_PAGES: u64 = 100;
/// Tries per page before giving up.
const ATTEMPTS: u32 = 3;
/// Longer than the mirror's cooldown after a pipe error, so the retry is not refused outright.
const RETRY_WAIT: Duration = Duration::from_secs(65);

/// Sends one page; true when every row was acknowledged.
async fn send_page(mirror: &DerivedMirror, table: &str, puts: &[DerivedRow]) -> bool {
    for attempt in 1..=ATTEMPTS {
        let sent = mirror.put_counted(table, puts).await;
        if sent == puts.len() {
            return true;
        }
        warn!(
            table,
            attempt,
            sent,
            expected = puts.len(),
            "scores snapshot: short send"
        );
        if attempt < ATTEMPTS {
            tokio::time::sleep(RETRY_WAIT).await;
        }
    }
    false
}

/// Copies one table page by page. Returns the rows acknowledged, or `Err(rows so far)` on a short
/// send. A read failure exits the process.
async fn copy_table<C: Copy, F, Fut>(
    mirror: &DerivedMirror,
    table: &str,
    collection: u32,
    next: F,
) -> Result<u64, u64>
where
    F: Fn(Option<C>) -> Fut,
    Fut: Future<Output = Result<Option<Page<C>>, sqlx::Error>>,
{
    let (mut pages, mut rows_sent) = (0u64, 0u64);
    let mut after = None;
    loop {
        let page = match next(after).await {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                error!(error = %e, table, collection, rows_sent, "scores snapshot: read failed");
                std::process::exit(1);
            }
        };
        after = Some(page.last);
        if !send_page(mirror, table, &page.rows).await {
            return Err(rows_sent);
        }
        pages += 1;
        rows_sent += page.rows.len() as u64;
        if pages.is_multiple_of(PROGRESS_EVERY_PAGES) {
            info!(
                table,
                collection, pages, rows_sent, "scores snapshot: progress"
            );
        }
        if page.rows.len() < PAGE_SIZE {
            break;
        }
    }
    info!(
        table,
        collection, pages, rows_sent, "scores snapshot: table done"
    );
    Ok(rows_sent)
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    let _telemetry = hermes_instrumentation::init(Config::new("vote-indexer", Backend::Console))?;

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let Some(mirror) = DerivedMirror::from_env() else {
        return Err(IndexerError::Config(
            "geo-db derived mirror is off or unconfigured: set GEODB_DERIVED_MIRROR=1 and the key and binding paths".into(),
        ));
    };
    if !matches!(
        env::var("GEODB_DERIVED_BATCH").as_deref(),
        Ok("1") | Ok("true")
    ) {
        warn!("GEODB_DERIVED_BATCH is not 1: rows go one request each, far slower");
    }
    let wanted = env::var("SCORES_SNAPSHOT_TABLES").unwrap_or_default();
    let wanted: Vec<&str> = wanted
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    for n in &wanted {
        if !["global_scores", "local_scores", "space_scores"].contains(n) {
            return Err(IndexerError::Config(format!(
                "SCORES_SNAPSHOT_TABLES names an unknown table: {n}"
            )));
        }
    }
    let on = |t: &str| wanted.is_empty() || wanted.contains(&t);

    let pool = PgPool::connect(&database_url)
        .await
        .map_err(|e| IndexerError::Config(format!("database connect failed: {e}")))?;
    info!(page_size = PAGE_SIZE, "scores snapshot: starting");

    let mut counts = (0u64, 0u64, 0u64);
    let mut failed = None;

    if on("space_scores") {
        match copy_table::<Uuid, _, _>(&mirror, "space_scores", SPACE_COLLECTION, |a| {
            space_page(&pool, a)
        })
        .await
        {
            Ok(n) => counts.2 = n,
            Err(n) => {
                counts.2 = n;
                failed = Some("space_scores");
            }
        }
    }
    if failed.is_none() && on("global_scores") {
        match copy_table::<Uuid, _, _>(&mirror, "global_scores", GLOBAL_COLLECTION, |a| {
            global_page(&pool, a)
        })
        .await
        {
            Ok(n) => counts.0 = n,
            Err(n) => {
                counts.0 = n;
                failed = Some("global_scores");
            }
        }
    }
    if failed.is_none() && on("local_scores") {
        match copy_table::<(Uuid, Uuid), _, _>(&mirror, "local_scores", LOCAL_COLLECTION, |a| {
            local_page(&pool, a)
        })
        .await
        {
            Ok(n) => counts.1 = n,
            Err(n) => {
                counts.1 = n;
                failed = Some("local_scores");
            }
        }
    }

    if let Some(table) = failed {
        // Everything acknowledged so far is in geo-db; a re-run converges.
        error!(
            table,
            global_rows = counts.0,
            local_rows = counts.1,
            space_rows = counts.2,
            "scores snapshot: incomplete, exiting non-zero"
        );
        std::process::exit(1);
    }
    info!(
        global_rows = counts.0,
        local_rows = counts.1,
        space_rows = counts.2,
        "scores snapshot complete"
    );
    Ok(())
}
