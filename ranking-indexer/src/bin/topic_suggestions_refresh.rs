//! Rebuilds the onboarding topic suggestions (GEO-3079).
//!
//! The onboarding "Customize your feed" step offers topics ranked by how many claims and
//! debates in the chosen spaces link to each. Counting that per request is too slow for a
//! dialog that must open instantly, and the ranking barely moves day to day, so
//! `refresh_space_topic_suggestions()` (migration 0095) precomputes it and the dialog reads
//! `topic_suggestions_for_spaces`. This binary is the schedule: one call, run hourly by the
//! `ranking-indexer-topic-suggestions` CronJob.
//!
//! The refresh is a single function call, so it is one transaction: readers keep the
//! previous ranking until the new one commits. On the live DB it took 1.6s (2026-09-29).
//!
//! Usage: `DATABASE_URL=... cargo run --bin topic_suggestions_refresh`

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::metrics::PgWriteSite;
use ranking_indexer::pg_bin_writes;
use ranking_indexer::topic_suggestions_compute as compute;
use ranking_indexer::topic_suggestions_mirror::{self, Suggestion};

/// `TOPIC_SUGGESTIONS_RUST_COMPUTE` (default off): after the refresh, compute the table again in
/// code from the source tables and log how many rows differ from what the SQL wrote. It never
/// changes the run: a failure is logged and the job carries on.
fn rust_compute_enabled() -> bool {
    matches!(
        env::var("TOPIC_SUGGESTIONS_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

async fn shadow(pool: &sqlx::PgPool) -> Result<(usize, usize), IndexerError> {
    let rels: Vec<(uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid)> = sqlx::query_as(
        "SELECT from_entity_id, to_entity_id, type_id, space_id FROM public.relations \
         WHERE type_id = $1 OR (type_id = $2 AND to_entity_id = ANY($3))",
    )
    .bind(compute::TOPICS_PROP)
    .bind(compute::TYPES_PROP)
    .bind(vec![
        compute::CLAIM_TYPE,
        compute::DEBATE_TYPE,
        compute::TOPIC_TYPE,
    ])
    .fetch_all(pool)
    .await?;
    let names: Vec<(uuid::Uuid, uuid::Uuid, String)> = sqlx::query_as(
        "SELECT entity_id, space_id, text FROM public.\"values\" \
         WHERE property_id = $1 AND text IS NOT NULL",
    )
    .bind(compute::NAME_PROP)
    .fetch_all(pool)
    .await?;
    let spaces: Vec<(uuid::Uuid, Option<uuid::Uuid>)> =
        sqlx::query_as("SELECT id, topic_id FROM public.spaces")
            .fetch_all(pool)
            .await?;
    let want: Vec<(uuid::Uuid, uuid::Uuid, String, i32, i32)> = sqlx::query_as(
        "SELECT space_id, topic_id, name_key, links, rank FROM public.space_topic_suggestions",
    )
    .fetch_all(pool)
    .await?;
    let got = compute::compute(
        &rels
            .into_iter()
            .map(|(from, to, type_id, space)| compute::Relation {
                from,
                to,
                type_id,
                space,
            })
            .collect::<Vec<_>>(),
        &names
            .into_iter()
            .map(|(entity, space, text)| compute::Name {
                entity,
                space,
                text,
            })
            .collect::<Vec<_>>(),
        &spaces
            .into_iter()
            .map(|(id, topic_id)| compute::Space { id, topic_id })
            .collect::<Vec<_>>(),
    );
    let want: Vec<compute::Row> = want
        .into_iter()
        .map(|(space_id, topic_id, name_key, links, rank)| compute::Row {
            space_id,
            topic_id,
            name_key,
            links,
            rank,
        })
        .collect();
    Ok((compute::count_mismatches(&got, &want), want.len()))
}

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

    // Best-effort copy into geo-db's derived class (GEODB_DERIVED_MIRROR, default off). With it off
    // `from_env` is None and nothing below reads the table a second time.
    let mirror = geodb_cursor::derived::DerivedMirror::from_env();
    let prior: Vec<(uuid::Uuid, uuid::Uuid)> = if mirror.is_some() {
        sqlx::query_as(topic_suggestions_mirror::PRIOR_SQL)
            .fetch_all(&pool)
            .await?
    } else {
        Vec::new()
    };

    let started = Instant::now();
    // A failure returns Err, so the process exits non-zero and the Job is marked failed.
    let refreshed: Option<i32> = pg_bin_writes::run(PgWriteSite::TopicSuggestionsRefresh, async {
        sqlx::query_scalar("SELECT public.refresh_space_topic_suggestions()")
            .fetch_one(&pool)
            .await
    })
    .await?;
    // Off: nothing was refreshed, so the log line and the SQL comparison are skipped; the mirror
    // below still runs.
    if let Some(rows) = refreshed {
        info!(
            rows,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "topic suggestions refreshed"
        );
    }
    if refreshed.is_some() && rust_compute_enabled() {
        match shadow(&pool).await {
            Ok((0, n)) => info!(rows = n, "topic suggestions rust compute matches the SQL"),
            Ok((bad, n)) => warn!(
                mismatched = bad,
                rows = n,
                "topic suggestions rust compute differs from the SQL"
            ),
            Err(e) => warn!(error = %e, "topic suggestions rust compute skipped"),
        }
    }
    if let Some(mirror) = &mirror {
        // Read after the refresh committed; a read error skips the mirror, never the job.
        match sqlx::query_as::<_, Suggestion>(topic_suggestions_mirror::SELECT_SQL)
            .fetch_all(&pool)
            .await
        {
            Ok(rows) => {
                topic_suggestions_mirror::mirror(mirror, &rows, &prior).await;
                info!(
                    rows = rows.len(),
                    prior = prior.len(),
                    "topic suggestions mirrored to geo-db"
                );
            }
            Err(e) => warn!(error = %e, "topic suggestions mirror skipped: read failed"),
        }
    }
    Ok(())
}
