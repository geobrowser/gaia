//! One-shot backfill of geo-db derived collections 0 (`entity_type_ranking`) and 1
//! (`entity_topic_ranking`) from Postgres. See `vote_indexer::ranking_snapshot` for why this is
//! its own binary, and RUNBOOK section 12 step 4 for when to run it.
//!
//! Needs `DATABASE_URL`, `GEODB_DERIVED_MIRROR=1` and the geodb-client wiring the vote-indexer pod
//! carries (`GEODB_CLIENT_BIN`, `GEODB_KEY_PATH`, `GEODB_BINDING_PATH`); set `GEODB_DERIVED_BATCH=1`.
//! Idempotent: re-run it freely. Logs counts only. Exits non-zero if any page could not be fully
//! acknowledged, after retrying it, so a Job is marked failed rather than silently partial.

use std::env;
use std::time::Duration;

use geodb_cursor::derived::DerivedMirror;
use hermes_instrumentation::{error, info, warn, Backend, Config};
use sqlx::PgPool;

use vote_indexer::derived_scores::{derived_row, TOPIC_COLLECTION, TYPE_COLLECTION};
use vote_indexer::error::IndexerError;
use vote_indexer::ranking_snapshot::{topic_page, type_cursor, type_page, PAGE_SIZE};

/// Log progress every this many pages (about 25,600 rows).
const PROGRESS_EVERY_PAGES: u64 = 100;
/// Tries per page before giving up.
const ATTEMPTS: u32 = 3;
/// Wait between tries. Longer than the mirror's 60 s cooldown after a pipe error, so the retry is
/// not refused outright.
const RETRY_WAIT: Duration = Duration::from_secs(65);

/// Sends one page; true when every row was acknowledged.
async fn send_page(
    mirror: &DerivedMirror,
    table: &str,
    rows: &[vote_indexer::derived_scores::RankRow],
) -> bool {
    let puts: Vec<_> = rows.iter().map(derived_row).collect();
    for attempt in 1..=ATTEMPTS {
        let sent = mirror.put_counted(table, &puts).await;
        if sent == puts.len() {
            return true;
        }
        warn!(
            table,
            attempt,
            sent,
            expected = puts.len(),
            "ranking snapshot: short send"
        );
        if attempt < ATTEMPTS {
            tokio::time::sleep(RETRY_WAIT).await;
        }
    }
    false
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    // Console: a one-shot Job wants its output in `kubectl logs`.
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

    let pool = PgPool::connect(&database_url)
        .await
        .map_err(|e| IndexerError::Config(format!("database connect failed: {e}")))?;
    info!(page_size = PAGE_SIZE, "ranking snapshot: starting");

    let mut failed = false;
    // GEODB_DERIVED_MIRROR_TABLES may leave a table out; then it is not even read.
    let do_type = mirror.allows("entity_type_ranking");
    let do_topic = mirror.allows("entity_topic_ranking");
    if !do_type || !do_topic {
        warn!(
            do_type,
            do_topic,
            "ranking snapshot: GEODB_DERIVED_MIRROR_TABLES leaves a table out; skipping it"
        );
    }

    // entity_type_ranking -> collection 0
    let (mut pages, mut rows_sent) = (0u64, 0u64);
    let mut after = None;
    loop {
        if !do_type {
            break;
        }
        let page = match type_page(&pool, after).await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, collection = TYPE_COLLECTION, rows_sent, "ranking snapshot: read failed");
                std::process::exit(1);
            }
        };
        let Some(last) = page.last() else { break };
        after = Some(type_cursor(last));
        if !send_page(&mirror, "entity_type_ranking", &page).await {
            failed = true;
            break;
        }
        pages += 1;
        rows_sent += page.len() as u64;
        if pages.is_multiple_of(PROGRESS_EVERY_PAGES) {
            info!(
                collection = TYPE_COLLECTION,
                pages, rows_sent, "ranking snapshot: progress"
            );
        }
        if page.len() < PAGE_SIZE {
            break;
        }
    }
    info!(
        collection = TYPE_COLLECTION,
        pages, rows_sent, "ranking snapshot: type table done"
    );
    let type_rows = rows_sent;

    // entity_topic_ranking -> collection 1
    let (mut pages, mut rows_sent) = (0u64, 0u64);
    let mut after = None;
    while do_topic && !failed {
        let page = match topic_page(&pool, after).await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, collection = TOPIC_COLLECTION, rows_sent, "ranking snapshot: read failed");
                std::process::exit(1);
            }
        };
        let Some((last, _)) = page.last() else { break };
        after = Some(*last);
        let rows: Vec<_> = page.into_iter().map(|(_, r)| r).collect();
        if !send_page(&mirror, "entity_topic_ranking", &rows).await {
            failed = true;
            break;
        }
        pages += 1;
        rows_sent += rows.len() as u64;
        if pages.is_multiple_of(PROGRESS_EVERY_PAGES) {
            info!(
                collection = TOPIC_COLLECTION,
                pages, rows_sent, "ranking snapshot: progress"
            );
        }
        if rows.len() < PAGE_SIZE {
            break;
        }
    }
    info!(
        collection = TOPIC_COLLECTION,
        pages, rows_sent, "ranking snapshot: topic table done"
    );

    if failed {
        // Everything acknowledged so far is in geo-db; a re-run converges, so just run it again.
        error!(
            type_rows,
            topic_rows = rows_sent,
            "ranking snapshot: incomplete, exiting non-zero"
        );
        std::process::exit(1);
    }
    info!(
        type_rows,
        topic_rows = rows_sent,
        "ranking snapshot complete"
    );
    Ok(())
}
