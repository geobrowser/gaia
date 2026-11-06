//! One-shot backfill of geo-db derived collection 21 (`entity_ranking_scores`, the engaged-only
//! overlay) from Postgres. The live mirror puts only entities touched after it was switched on, so
//! the roughly 9.5k engaged rows already in `public.entity_ranking_scores` are missing until this
//! runs. See `vote_indexer::entity_scores_mirror` for the collection's shape.
//!
//! Needs `DATABASE_URL`, `GEODB_DERIVED_MIRROR=1` and the geodb-client wiring the vote-indexer pod
//! carries (`GEODB_CLIENT_BIN`, `GEODB_KEY_PATH`, `GEODB_BINDING_PATH`); set `GEODB_DERIVED_BATCH=1`.
//! Reads only the engaged rows (keyset pages by entity id), never the 48.9M-row table's quiet part.
//! Idempotent: re-run it freely. Logs counts only. Exits non-zero if any page could not be fully
//! acknowledged, after retrying it, so a Job is marked failed rather than silently partial.

use std::env;
use std::time::Duration;

use geodb_cursor::derived::DerivedMirror;
use hermes_instrumentation::{error, info, warn, Backend, Config};
use sqlx::PgPool;
use uuid::Uuid;

use vote_indexer::entity_scores_mirror::{engaged_page, engaged_puts, COLLECTION};
use vote_indexer::error::IndexerError;

/// Rows per page.
const PAGE_SIZE: i64 = 2048;
/// Tries per page before giving up.
const ATTEMPTS: u32 = 3;
/// Wait between tries. Longer than the mirror's 60 s cooldown after a pipe error.
const RETRY_WAIT: Duration = Duration::from_secs(65);

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
    if !mirror.allows("entity_ranking_scores") {
        return Err(IndexerError::Config(
            "GEODB_DERIVED_MIRROR_TABLES leaves entity_ranking_scores out".into(),
        ));
    }
    if !matches!(
        env::var("GEODB_DERIVED_BATCH").as_deref(),
        Ok("1") | Ok("true")
    ) {
        warn!("GEODB_DERIVED_BATCH is not 1: rows go one request each, far slower");
    }

    let pool = PgPool::connect(&database_url)
        .await
        .map_err(|e| IndexerError::Config(format!("database connect failed: {e}")))?;
    info!(
        page_size = PAGE_SIZE,
        collection = COLLECTION,
        "entity scores snapshot: starting"
    );

    let (mut pages, mut rows_sent) = (0u64, 0u64);
    let mut after = Uuid::nil();
    loop {
        let rows = match engaged_page(&pool, after, PAGE_SIZE).await {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, rows_sent, "entity scores snapshot: read failed");
                std::process::exit(1);
            }
        };
        let Some(last) = rows.last() else { break };
        after = last.entity_id;
        let puts = engaged_puts(&rows);
        let mut ok = puts.is_empty();
        for attempt in 1..=ATTEMPTS {
            if ok {
                break;
            }
            let sent = mirror.put_counted("entity_ranking_scores", &puts).await;
            if sent == puts.len() {
                ok = true;
                break;
            }
            warn!(
                attempt,
                sent,
                expected = puts.len(),
                "entity scores snapshot: short send"
            );
            if attempt < ATTEMPTS {
                tokio::time::sleep(RETRY_WAIT).await;
            }
        }
        if !ok {
            // Everything acknowledged so far is in geo-db; a re-run converges.
            error!(
                pages,
                rows_sent, "entity scores snapshot: incomplete, exiting non-zero"
            );
            std::process::exit(1);
        }
        pages += 1;
        rows_sent += puts.len() as u64;
        info!(pages, rows_sent, "entity scores snapshot: progress");
        if (rows.len() as i64) < PAGE_SIZE {
            break;
        }
    }
    info!(pages, rows_sent, "entity scores snapshot complete");
    Ok(())
}
