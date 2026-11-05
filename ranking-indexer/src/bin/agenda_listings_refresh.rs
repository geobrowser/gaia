//! Rebuilds what Debate agendas name and re-scores what that moves (GEO-3488, migration 0113).
//!
//! Best lifts the claims on each space's daily Debate agenda and the debates recorded on them, and,
//! more lightly, the claims and debates on the topics the agenda names. The relations table has no
//! timestamps and nothing re-scores an entity when a relation changes, and the boost decays by the
//! day, so this job is what brings both to the score: one call to
//! `trending.refresh_agenda_listings()`, run hourly by the `ranking-indexer-agenda-listings` CronJob.
//!
//! The call brings `trending.agenda_listings` in step with the graph and re-scores the claims and
//! topics whose rows changed, the debates on those claims, and the claims and debates tagged with
//! those topics. The first run of each New York day is a full pass that also re-scores every listed
//! claim, everything on a topic inside the horizon and anything still storing a term, which moves
//! the decaying boosts to the new day. It is one function call and so one transaction: readers keep
//! the previous rows and scores until it commits.
//!
//! Then, once the refresh has committed, the listings that changed go to the analytics warehouse
//! (GEO-3556, `ranking_indexer::agenda_listings_export`), so it reads the set the refresh just wrote.
//! A job without ClickHouse credentials refreshes and skips the send. A refused send fails the run
//! after the refresh has committed; the next run sends it again.
//!
//! Usage: `DATABASE_URL=... [CLICKHOUSE_URL=... CLICKHOUSE_USER=... CLICKHOUSE_PASSWORD=...] cargo run --bin agenda_listings_refresh`

use std::env;
use std::time::{Duration, Instant};

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::{agenda_listings_export, clickhouse};

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
    // The app role's statement_timeout is 30s and a sweep over busy topics can pass it, so it gets
    // its own limit, as topic_ranking_reconcile does. SET LOCAL inside one transaction rather than a
    // session SET: the connection goes through a transaction-mode pooler.
    // A failure returns Err, so the process exits non-zero and the Job is marked failed.
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '10min'")
        .execute(&mut *tx)
        .await?;
    let scored: i32 = sqlx::query_scalar("SELECT trending.refresh_agenda_listings()")
        .fetch_one(&mut *tx)
        .await?;
    // The two score counts read entity_ranking_scores_trending_idx, not the whole table.
    let (listings, claims, topics, boosted, topic_boosted): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM trending.agenda_listings), \
                (SELECT count(DISTINCT entity_id) FROM trending.agenda_listings \
                 WHERE source <> 'agenda_topics'), \
                (SELECT count(DISTINCT entity_id) FROM trending.agenda_listings \
                 WHERE source = 'agenda_topics'), \
                (SELECT count(*) FROM public.entity_ranking_scores WHERE trending_score <> 0), \
                (SELECT count(*) FROM public.entity_ranking_scores \
                 WHERE trending_score <> 0 AND trending_source = 'topic')",
        )
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    info!(
        scored,
        listings,
        claims,
        topics,
        boosted,
        topic_boosted,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "agenda listings refreshed"
    );

    if !clickhouse::configured() {
        warn!("no ClickHouse credentials: agenda listings not sent to the warehouse");
        return Ok(());
    }
    let started = Instant::now();
    let sent = agenda_listings_export::export(&pool, Duration::from_secs(60)).await?;
    info!(
        added = sent.added,
        removed = sent.removed,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "agenda listings sent to the warehouse"
    );
    Ok(())
}
