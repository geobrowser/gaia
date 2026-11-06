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
//! Usage: `DATABASE_URL=... cargo run --bin agenda_listings_refresh`

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::agenda_listings_compute as compute;
use ranking_indexer::agenda_listings_mirror::{self, Ident, Listing};
use ranking_indexer::entity_ranking_score_compute as scores;
use ranking_indexer::error::IndexerError;

/// `AGENDA_LISTINGS_RUST_COMPUTE` (default off): after the refresh, compute the listings table again
/// in code from the graph and log how many rows differ from what the SQL wrote. It covers the
/// listings half only (not the re-scoring), never changes the run, and logs a failure and carries on.
fn rust_compute_enabled() -> bool {
    matches!(
        env::var("AGENDA_LISTINGS_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

type DateRow = (
    uuid::Uuid,
    uuid::Uuid,
    Option<String>,
    Option<String>,
    Option<chrono::NaiveDate>,
);

async fn shadow(pool: &sqlx::PgPool) -> Result<(usize, usize), IndexerError> {
    use std::collections::BTreeSet;
    use uuid::Uuid;
    const TYPES: &str = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1";
    const BLOCKS: &str = "beaba5cb-a677-41a8-b353-77030613fc70";
    let rels: Vec<(Uuid, Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT from_entity_id, to_entity_id, type_id, space_id FROM public.relations r \
         WHERE (r.type_id = $1::uuid AND r.to_entity_id IN ( \
                  '79e3757b-2630-492c-9d3b-d73a78d45893', '96f859ef-a1ca-4b22-9372-c86ad58b694b', \
                  'fd51f935-2063-4617-be39-7b672b23364c')) \
            OR r.type_id IN ($2::uuid, 'e614cce1-c4ce-4586-8304-fd1237119eb2', \
                  'b96bf701-a399-430d-a072-f5e910cdeda9', '806d52bc-27e9-4c91-93c0-57978b093351') \
            OR (r.type_id = 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a' AND r.from_entity_id IN ( \
                  SELECT b.to_entity_id FROM public.relations b WHERE b.type_id = $2::uuid))",
    )
    .bind(TYPES)
    .bind(BLOCKS)
    .fetch_all(pool)
    .await?;
    let spaces: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, type::text FROM public.spaces")
        .fetch_all(pool)
        .await?;
    let dates: Vec<DateRow> = sqlx::query_as(
        "SELECT entity_id, space_id, datetime, date, \
                    (datetime_utc AT TIME ZONE trending.time_zone())::date \
             FROM public.\"values\" WHERE property_id = '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'",
    )
    .fetch_all(pool)
    .await?;
    let table: Vec<(Uuid, Uuid, Uuid, chrono::NaiveDate, String)> = sqlx::query_as(
        "SELECT agenda_id, space_id, entity_id, listed_on, source FROM trending.agenda_listings",
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
        &spaces
            .into_iter()
            .map(|(id, t)| compute::Space {
                id,
                dao: t == "DAO",
            })
            .collect::<Vec<_>>(),
        &dates
            .into_iter()
            .map(
                |(entity, space, datetime, date, ny_day)| compute::DateValue {
                    entity,
                    space,
                    datetime,
                    date,
                    ny_day,
                },
            )
            .collect::<Vec<_>>(),
    );
    // Compare as text so an unknown source in the table shows as a mismatch instead of a panic.
    let got: BTreeSet<_> = got
        .into_iter()
        .map(|l| {
            (
                l.agenda_id,
                l.space_id,
                l.entity_id,
                l.listed_on,
                l.source.to_string(),
            )
        })
        .collect();
    let want: BTreeSet<_> = table.into_iter().collect();
    Ok((got.symmetric_difference(&want).count(), want.len()))
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
    let prior: Vec<Ident> = if mirror.is_some() {
        sqlx::query_as(agenda_listings_mirror::PRIOR_SQL)
            .fetch_all(&pool)
            .await?
    } else {
        Vec::new()
    };

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
    if rust_compute_enabled() {
        match shadow(&pool).await {
            Ok((0, n)) => info!(rows = n, "agenda listings rust compute matches the SQL"),
            Ok((bad, n)) => warn!(
                mismatched = bad,
                rows = n,
                "agenda listings rust compute differs from the SQL"
            ),
            Err(e) => warn!(error = %e, "agenda listings rust compute skipped"),
        }
    }
    if let Some(mirror) = &mirror {
        // Read after the refresh committed; a read error skips the mirror, never the job.
        match sqlx::query_as::<_, Listing>(agenda_listings_mirror::SELECT_SQL)
            .fetch_all(&pool)
            .await
        {
            Ok(rows) => {
                agenda_listings_mirror::mirror(mirror, &rows, &prior).await;
                // `rows` is what was read from Postgres and offered to geo-db; the acknowledged
                // count is logged by `geodb_cursor::derived` (`geodb_derived.acknowledged`).
                info!(
                    rows_offered = rows.len(),
                    prior = prior.len(),
                    "agenda listings offered to geo-db"
                );
            }
            Err(e) => warn!(error = %e, "agenda listings mirror skipped: read failed"),
        }
    }
    // The `last_full_pass_on` watermark the refresh just moved (collection 7). Own flag, default off.
    if let Some(state) = ranking_indexer::state_mirror::from_env() {
        let acked = ranking_indexer::state_mirror::mirror_refresh(&state, &pool).await;
        info!(
            acknowledged = acked,
            "trending refresh state mirrored to geo-db"
        );
    }
    if scores::shadow_enabled() {
        // After the commit, so it never holds the refresh's transaction. A read or compute problem
        // is logged and skipped; the SQL scores stay the source.
        shadow_scores(&pool).await;
    }
    Ok(())
}

/// Recomputes the most recently written score rows from the inputs stored beside them and counts
/// the disagreements. Never fails the Job.
async fn shadow_scores(pool: &sqlx::PgPool) {
    const SAMPLE: i64 = 5000;
    let cfg =
        match sqlx::query_as::<_, (f64, f64, f64, f64, f64, f64, i32, i32, f64, f64, f64, f64)>(
            scores::CONFIG_SQL,
        )
        .fetch_one(pool)
        .await
        {
            Ok(c) => scores::Config {
                tau_seconds: c.0,
                wilson_z: c.1,
                prior_positive: c.2,
                prior_negative: c.3,
                quality_floor: c.4,
                intrinsic_cap: c.5,
                intrinsic_property_target: c.6,
                intrinsic_relation_target: c.7,
                participation_weight: c.8,
                participation_cap: c.9,
                comment_weight: c.10,
                comment_cap: c.11,
            },
            Err(e) => {
                warn!(error = %e, "entity ranking score shadow skipped: config read failed");
                return;
            }
        };
    match sqlx::query_as::<_, scores::Stored>(scores::STORED_SQL)
        .bind(SAMPLE)
        .fetch_all(pool)
        .await
    {
        Ok(rows) => {
            let bad = scores::mismatches(&cfg, &rows);
            if bad.is_empty() {
                info!(
                    checked = rows.len(),
                    "entity ranking score shadow: all rows match"
                );
            } else {
                warn!(
                    checked = rows.len(),
                    mismatched = bad.len(),
                    first = %bad[0],
                    "entity ranking score shadow: rows differ from the Rust compute"
                );
            }
        }
        Err(e) => warn!(error = %e, "entity ranking score shadow skipped: read failed"),
    }
}
