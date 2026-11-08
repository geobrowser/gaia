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
use ranking_indexer::agenda_rescore_compute as rescore;
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

type RelRow = (uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid);

async fn read_rels(pool: &sqlx::PgPool) -> Result<Vec<RelRow>, IndexerError> {
    const TYPES: &str = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1";
    const BLOCKS: &str = "beaba5cb-a677-41a8-b353-77030613fc70";
    let rels: Vec<RelRow> = sqlx::query_as(
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
    Ok(rels)
}

async fn shadow(pool: &sqlx::PgPool) -> Result<(usize, usize), IndexerError> {
    use std::collections::BTreeSet;
    use uuid::Uuid;
    let rels = read_rels(pool).await?;
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

/// `AGENDA_RESCORE_RUST_COMPUTE` (default off): read what the sweep sees before the refresh, then after
/// it commits work out in code which ids the SQL should have re-scored and what terms they should carry,
/// and log how that compares (the `scored` count, and the stored trending score and age credit). It
/// never changes the run and logs a failure and carries on.
fn rescore_compute_enabled() -> bool {
    matches!(
        env::var("AGENDA_RESCORE_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// What `trending.refresh_agenda_listings` reads that it then changes.
struct RescoreBefore {
    stored: std::collections::BTreeSet<compute::Listing>,
    terms: Vec<rescore::StoredTerm>,
    last_full_pass_on: Option<chrono::NaiveDate>,
    cfg: (f64, f64, f64, f64, i32, i32, f64),
}

fn listing_source(s: &str) -> Option<&'static str> {
    [
        "block_item",
        "agenda_claims",
        "agenda_debates",
        "agenda_topics",
    ]
    .into_iter()
    .find(|k| *k == s)
}

async fn read_listings(
    pool: &sqlx::PgPool,
) -> Result<std::collections::BTreeSet<compute::Listing>, IndexerError> {
    let rows: Vec<(
        uuid::Uuid,
        uuid::Uuid,
        uuid::Uuid,
        chrono::NaiveDate,
        String,
    )> = sqlx::query_as(
        "SELECT agenda_id, space_id, entity_id, listed_on, source FROM trending.agenda_listings",
    )
    .fetch_all(pool)
    .await?;
    // A source this build does not know is skipped; the SQL count then disagrees and the log shows it.
    Ok(rows
        .into_iter()
        .filter_map(|(agenda_id, space_id, entity_id, listed_on, source)| {
            Some(compute::Listing {
                agenda_id,
                space_id,
                entity_id,
                listed_on,
                source: listing_source(&source)?,
            })
        })
        .collect())
}

async fn rescore_before(pool: &sqlx::PgPool) -> Result<RescoreBefore, IndexerError> {
    let terms: Vec<(uuid::Uuid, f64, f64)> = sqlx::query_as(
        "SELECT entity_id, trending_score::float8, age_credit::float8 \
         FROM public.entity_ranking_scores WHERE trending_score <> 0 OR age_credit <> 0",
    )
    .fetch_all(pool)
    .await?;
    let last: Option<chrono::NaiveDate> =
        sqlx::query_scalar("SELECT last_full_pass_on FROM trending.refresh_state")
            .fetch_optional(pool)
            .await?
            .flatten();
    let cfg = sqlx::query_as(
        "SELECT trending_boost::float8, trending_topic_boost::float8, trending_day_factor::float8, \
                trending_debate_factor::float8, trending_debate_window_days, trending_horizon_days, \
                tau_seconds::float8 FROM public.entity_ranking_config WHERE id",
    )
    .fetch_one(pool)
    .await?;
    Ok(RescoreBefore {
        stored: read_listings(pool).await?,
        terms: terms
            .into_iter()
            .map(|(entity, trending_score, age_credit)| rescore::StoredTerm {
                entity,
                trending_score,
                age_credit,
            })
            .collect(),
        last_full_pass_on: last,
        cfg,
    })
}

/// Returns `(ids the code would re-score, the SQL's count, rows whose stored terms differ)`.
async fn rescore_shadow(
    pool: &sqlx::PgPool,
    before: &RescoreBefore,
    scored: i32,
) -> Result<(usize, i32, usize), IndexerError> {
    use std::collections::{HashMap, HashSet};
    let incoming = read_listings(pool).await?;
    let rels: Vec<compute::Relation> = read_rels(pool)
        .await?
        .into_iter()
        .map(|(from, to, type_id, space)| compute::Relation {
            from,
            to,
            type_id,
            space,
        })
        .collect();
    let spaces: Vec<compute::Space> =
        sqlx::query_as::<_, (uuid::Uuid, String)>("SELECT id, type::text FROM public.spaces")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id, t)| compute::Space {
                id,
                dao: t == "DAO",
            })
            .collect();
    let typed = rescore::Graph::new(&rels, &spaces, HashMap::new()).typed_ids();
    let created: HashMap<uuid::Uuid, Option<i64>> = sqlx::query_as::<_, (uuid::Uuid, String)>(
        "SELECT id, created_at FROM public.entities WHERE id = ANY($1)",
    )
    .bind(&typed)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, c)| (id, rescore::created_epoch(&c)))
    .collect();
    let graph = rescore::Graph::new(&rels, &spaces, created);
    let (boost, topic_boost, day_factor, debate_factor, window, horizon, tau) = before.cfg;
    let now = chrono::Utc::now();
    let today = ranking_indexer::sql_helpers::today(now);
    let sweep = rescore::Sweep {
        today,
        horizon_days: horizon,
        boost,
        full_pass: rescore::is_full_pass(before.last_full_pass_on, today),
    };
    let ids = rescore::rescore_ids(&graph, &before.stored, &incoming, &before.terms, &sweep);
    let want_ids: HashSet<uuid::Uuid> = ids.iter().copied().collect();
    let cfg = scores::TrendConfig {
        boost,
        topic_boost,
        day_factor,
        debate_factor,
        horizon_days: horizon,
        tau_seconds: tau,
    };
    let terms = graph.trending_terms(
        &incoming,
        &want_ids,
        &cfg,
        window,
        today,
        now.timestamp() as f64,
    );
    let stored: Vec<(uuid::Uuid, f64, f64)> = sqlx::query_as(
        "SELECT entity_id, trending_score::float8, age_credit::float8 \
         FROM public.entity_ranking_scores WHERE entity_id = ANY($1)",
    )
    .bind(ids.iter().copied().collect::<Vec<_>>())
    .fetch_all(pool)
    .await?;
    let stored: HashMap<_, _> = stored.into_iter().map(|(e, t, a)| (e, (t, a))).collect();
    let bad = ids
        .iter()
        .filter(|id| {
            let (t, a) = terms
                .get(id)
                .map(|r| (r.terms.trending_score, r.terms.age_credit))
                .unwrap_or((0.0, 0.0));
            let (st, sa) = stored.get(id).copied().unwrap_or((0.0, 0.0));
            !(scores::close(t, st) && scores::close(a, sa))
        })
        .count();
    Ok((ids.len(), scored, bad))
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

    // The ids already carrying a trending term, for the collection 21 mirror (needs the table in GEODB_DERIVED_MIRROR_TABLES).
    let scores_before = match &mirror {
        Some(m) => ranking_indexer::entity_scores_mirror::prior(m, &pool).await,
        None => None,
    };

    // Read before the refresh changes them, only with the flag on.
    let rescore_state = if rescore_compute_enabled() {
        match rescore_before(&pool).await {
            Ok(b) => Some(b),
            Err(e) => {
                warn!(error = %e, "agenda rescore rust compute skipped: before-state read failed");
                None
            }
        }
    } else {
        None
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
    if let Some(before) = &rescore_state {
        match rescore_shadow(&pool, before, scored).await {
            Ok((n, sql, 0)) if n as i64 == i64::from(sql) => {
                info!(ids = n, "agenda rescore rust compute matches the SQL")
            }
            Ok((n, sql, bad)) => warn!(
                rust_ids = n,
                sql_scored = sql,
                terms_differ = bad,
                "agenda rescore rust compute differs from the SQL"
            ),
            Err(e) => warn!(error = %e, "agenda rescore rust compute skipped"),
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
    // The re-scored rows into collection 21, after the commit; a read error skips the mirror, never the job.
    if let (Some(mirror), Some(before)) = (&mirror, &scores_before) {
        let (puts, deletes) =
            ranking_indexer::entity_scores_mirror::mirror(mirror, &pool, before).await;
        info!(puts, deletes, "entity scores offered to geo-db");
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
