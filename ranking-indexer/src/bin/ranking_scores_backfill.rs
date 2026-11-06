//! One-shot backfill of geo-db derived collection 11 (`ranks.ranking_scores`) from Postgres.
//!
//! The ranking-indexer mirrors a block only when it recomputes it (`replace_ranking_scores`), so a
//! block that has not been recomputed since the mirror went on has no rows in geo-db. This reads
//! the whole table in key order and puts every row, in the same shape the live mirror writes
//! (`ranking_indexer::derived_mirror::plan` with no prior keys).
//!
//! Not scheduled and never run by the service. It needs `GEODB_DERIVED_MIRROR=1`, the key and
//! binding wiring and `GEODB_CLIENT_BIN`; set `GEODB_DERIVED_BATCH=1` for 256 rows a request.
//! Idempotent (last write wins), so re-run it freely. Logs counts only. Exits non-zero when a page
//! was not fully acknowledged after its retries, so a Job is marked failed rather than partial.
//!
//! Usage: `DATABASE_URL=... GEODB_DERIVED_MIRROR=1 ... ranking_scores_backfill`

use std::env;
use std::time::Duration;

use geodb_cursor::derived::DerivedMirror;
use ranking_indexer::derived_mirror;
use ranking_indexer::error::IndexerError;
use ranking_indexer::scoring::ScoreRow;
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Rows read and sent a page.
const PAGE: i64 = 2048;
const ATTEMPTS: u32 = 3;
/// Longer than the mirror's 60 s cooldown after a pipe error, so a retry is not refused outright.
const RETRY_WAIT: Duration = Duration::from_secs(65);

type Row = (Uuid, Uuid, Uuid, f64, i32);

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let Some(mirror) = DerivedMirror::from_env() else {
        return Err(IndexerError::Config(
            "geo-db derived mirror is off or unconfigured: set GEODB_DERIVED_MIRROR=1 and the key and binding paths".into(),
        ));
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    let (mut pages, mut sent_total) = (0u64, 0u64);
    let mut after: Option<(Uuid, Uuid, Uuid)> = None;
    loop {
        let rows: Vec<Row> = match after {
            None => sqlx::query_as(
                "SELECT block_id, entity_id, space_id, score, position FROM ranks.ranking_scores \
                     ORDER BY block_id, entity_id, space_id LIMIT $1",
            )
            .bind(PAGE)
            .fetch_all(&pool)
            .await?,
            Some((b, e, s)) => sqlx::query_as(
                "SELECT block_id, entity_id, space_id, score, position FROM ranks.ranking_scores \
                     WHERE (block_id, entity_id, space_id) > ($1, $2, $3) \
                     ORDER BY block_id, entity_id, space_id LIMIT $4",
            )
            .bind(b)
            .bind(e)
            .bind(s)
            .bind(PAGE)
            .fetch_all(&pool)
            .await?,
        };
        let Some(last) = rows.last() else { break };
        after = Some((last.0, last.1, last.2));
        // Rows are grouped by block, which `plan` takes one block at a time.
        let mut puts = Vec::with_capacity(rows.len());
        let mut i = 0;
        while i < rows.len() {
            let block = rows[i].0;
            let j = rows[i..]
                .iter()
                .position(|r| r.0 != block)
                .map_or(rows.len(), |n| i + n);
            let scores: Vec<ScoreRow> = rows[i..j]
                .iter()
                .map(|r| ScoreRow {
                    entity_id: r.1,
                    space_id: r.2,
                    score: r.3,
                    position: r.4,
                })
                .collect();
            puts.extend(derived_mirror::plan(block, &scores, &[]).0);
            i = j;
        }
        let mut ok = false;
        for attempt in 1..=ATTEMPTS {
            let acked = mirror.put_counted("ranking_scores", &puts).await;
            if acked == puts.len() {
                ok = true;
                break;
            }
            warn!(
                attempt,
                acked,
                expected = puts.len(),
                "ranking scores backfill: short send"
            );
            if attempt < ATTEMPTS {
                tokio::time::sleep(RETRY_WAIT).await;
            }
        }
        if !ok {
            error!(
                pages,
                sent_total, "ranking scores backfill: a page was not acknowledged; stopping"
            );
            std::process::exit(1);
        }
        pages += 1;
        sent_total += puts.len() as u64;
        if pages % 50 == 0 {
            info!(pages, sent_total, "ranking scores backfill: progress");
        }
        if (rows.len() as i64) < PAGE {
            break;
        }
    }
    info!(pages, sent_total, "ranking scores backfill: done");
    Ok(())
}
