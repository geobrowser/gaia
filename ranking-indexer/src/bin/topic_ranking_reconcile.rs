//! Keeps the For you topic ranking complete (GEO-3077).
//!
//! `entity_topic_ranking` (migration 0096) holds one row per (topic, type, entity) the For you
//! feed can show. `refresh_entity_ranking_scores` keeps it in step for every entity whose score
//! changes, but nothing reports a tag, name or type added to an entity whose score does not, or
//! a change to the editorial type exclusions. `reconcile_entity_topic_ranking()` repairs all of
//! that in one pass. This binary is its schedule: hourly, the same window new entities are
//! scored in. It is also the backfill: the table is created empty.
//!
//! Measured on the live DB 2026-09-29: 24s as the backfill (424,527 rows), 20s with nothing to
//! change.
//!
//! It first re-scores every topic debate (GEO-3150, migration 0107): a topic debate's Best score
//! counts the Interested votes and the debates already held on its topic, and neither changes the
//! debate itself, so nothing else would re-score it when they move. There are few topic debates,
//! so this is cheap, and running it first means the reconcile below sees the new scores.
//!
//! Usage: `DATABASE_URL=... cargo run --bin topic_ranking_reconcile`

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::info;

use ranking_indexer::error::IndexerError;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    let started = Instant::now();
    // The app role's statement_timeout is 30s and this pass takes 20-24s, so it gets its own
    // limit. SET LOCAL inside one transaction rather than a session SET: the connection goes
    // through a transaction-mode pooler, where a session setting may land on another backend.
    // A failure returns Err, so the process exits non-zero and the Job is marked failed.
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '10min'")
        .execute(&mut *tx)
        .await?;
    let topic_debates: i32 = sqlx::query_scalar("SELECT public.refresh_topic_debate_scores()")
        .fetch_one(&mut *tx)
        .await?;
    let changed: i32 = sqlx::query_scalar("SELECT public.reconcile_entity_topic_ranking()")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    info!(
        topic_debates,
        changed,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "topic ranking reconciled"
    );
    Ok(())
}
