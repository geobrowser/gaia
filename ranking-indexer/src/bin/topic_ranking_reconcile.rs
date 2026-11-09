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
//! `TOPIC_RANKING_RUST_COMPUTE` (default off, `1` or `true` to enable): inside the same transaction, work
//! out in code what the two SQL calls should have done ([`ranking_indexer::topic_ranking_compute`]) and log
//! how that compares. It only logs; a failure in it is rolled back to a savepoint and the job carries on.
//!
//! Usage: `DATABASE_URL=... cargo run --bin topic_ranking_reconcile`

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::time::Instant;

use futures::TryStreamExt;
use geodb_cursor::derived::DerivedMirror;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Acquire, PgConnection};
use tracing::{info, warn};
use uuid::Uuid;

use ranking_indexer::error::IndexerError;
use ranking_indexer::metrics::PgWriteSite;
use ranking_indexer::pg_bin_writes;
use ranking_indexer::topic_ranking_compute::{
    created_epoch, FeedKey, FeedRow, Graph, Rel, TopicConfig, Value, Vote,
};
use ranking_indexer::topic_ranking_mirror::{self, Diff};

const TYPES: &str = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1";
const TOPICS: &str = "806d52bc-27e9-4c91-93c0-57978b093351";
const SYSTEM_TYPE: &str = "88b3d6ad-288c-529c-a212-0e1c24819185";
const NAME: &str = "a126ca53-0c8e-48d5-b888-82c734c38935";
const CLAIMS: &str = "e614cce1-c4ce-4586-8304-fd1237119eb2";
const DEBATE_TYPE: &str = "fd51f935-2063-4617-be39-7b672b23364c";
const TAG_RELATION: &str = "25709034-1ba5-406f-94e4-d4af90042fba";

fn rust_compute_enabled() -> bool {
    matches!(
        env::var("TOPIC_RANKING_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

type RelRow = (Uuid, Uuid, Uuid);

fn rels(rows: Vec<RelRow>) -> Vec<Rel> {
    rows.into_iter()
        .map(|(from, to, type_id)| Rel { from, to, type_id })
        .collect()
}

async fn created_of(
    conn: &mut PgConnection,
    sql: &str,
    ids: Option<&[Uuid]>,
) -> Result<HashMap<Uuid, Option<i64>>, IndexerError> {
    let q = sqlx::query_as::<_, (Uuid, Option<String>)>(sql);
    let rows = match ids {
        Some(ids) => q.bind(ids).fetch_all(&mut *conn).await?,
        None => q.fetch_all(&mut *conn).await?,
    };
    Ok(rows
        .into_iter()
        .map(|(id, c)| (id, c.as_deref().and_then(created_epoch)))
        .collect())
}

/// After `refresh_topic_debate_scores()`: the ids it should have scored, and whether each stored
/// `topic_score` is what the code computes. Returns `(ids, rows whose stored topic score differs)`.
async fn debate_shadow(conn: &mut PgConnection) -> Result<(usize, usize), IndexerError> {
    let debates: Vec<RelRow> = sqlx::query_as(
        "SELECT from_entity_id, to_entity_id, type_id FROM public.relations \
         WHERE type_id = $1::uuid AND to_entity_id = $2::uuid",
    )
    .bind(TYPES)
    .bind(DEBATE_TYPE)
    .fetch_all(&mut *conn)
    .await?;
    let debate_ids: Vec<Uuid> = debates.iter().map(|r| r.0).collect();
    let mut all = debates;
    all.extend(
        sqlx::query_as::<_, RelRow>(
            "SELECT from_entity_id, to_entity_id, type_id FROM public.relations \
             WHERE type_id IN ($1::uuid, $2::uuid) AND from_entity_id = ANY($3)",
        )
        .bind(TOPICS)
        .bind(CLAIMS)
        .bind(&debate_ids)
        .fetch_all(&mut *conn)
        .await?,
    );
    let topic_ids: Vec<Uuid> = all
        .iter()
        .filter(|r| r.2.to_string() == TOPICS)
        .map(|r| r.1)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // Every entity tagged with one of those topics: the debates among them are what "held" counts.
    all.extend(
        sqlx::query_as::<_, RelRow>(
            "SELECT from_entity_id, to_entity_id, type_id FROM public.relations \
             WHERE type_id = $1::uuid AND to_entity_id = ANY($2)",
        )
        .bind(TOPICS)
        .bind(&topic_ids)
        .fetch_all(&mut *conn)
        .await?,
    );
    let created = created_of(
        conn,
        "SELECT id, created_at FROM public.entities WHERE id = ANY($1)",
        Some(&debate_ids),
    )
    .await?;
    let cfg: (f64, f64, f64) = sqlx::query_as(
        "SELECT topic_interested_weight::float8, topic_debate_weight::float8, topic_cap::float8 \
         FROM public.entity_ranking_config WHERE id",
    )
    .fetch_one(&mut *conn)
    .await?;
    let votes: Vec<Vote> = sqlx::query_as::<_, (Uuid, Uuid, i16, i16, i16)>(
        "SELECT user_id, object_id, object_type, vote_kind, vote_type FROM public.user_votes \
         WHERE object_id = ANY($1) AND object_type = 0 AND vote_kind = 3 AND vote_type = 0",
    )
    .bind(&topic_ids)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(user, object, object_type, vote_kind, vote_type)| Vote {
        user,
        object,
        object_type,
        vote_kind,
        vote_type,
    })
    .collect();
    let voters: Vec<Uuid> = votes
        .iter()
        .map(|v| v.user)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let weights: BTreeMap<String, f64> = sqlx::query_as::<_, (Uuid, f64)>(
        "SELECT user_id, weight FROM public.account_weights WHERE user_id = ANY($1)",
    )
    .bind(&voters)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(u, w)| (u.to_string(), w))
    .collect();

    let with_row: BTreeSet<Uuid> = created.keys().copied().collect();
    let graph = Graph::new(&rels(all), &[], &[], created);
    let ids = graph.topic_debate_ids();
    let want = graph.topic_debate_scores(
        &ids,
        &TopicConfig {
            interested_weight: cfg.0,
            debate_weight: cfg.1,
            cap: cfg.2,
        },
        &votes,
        &weights,
    );
    let id_list: Vec<Uuid> = ids.iter().copied().collect();
    let stored: HashMap<Uuid, f64> = sqlx::query_as::<_, (Uuid, f64)>(
        "SELECT entity_id, topic_score::float8 FROM public.entity_ranking_scores \
         WHERE entity_id = ANY($1)",
    )
    .bind(&id_list)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    // A debate with no `entities` row is skipped by the SQL and has no stored row either.
    let differ = ids
        .iter()
        .filter(|id| {
            let w = want.get(id).map_or(0.0, |r| r.topic_score);
            let s = stored.get(id).copied().unwrap_or(0.0);
            !ranking_indexer::entity_ranking_score_compute::close(w, s)
        })
        .count();
    // The SQL scores only debates that have an `entities` row, and returns that count.
    Ok((ids.intersection(&with_row).count(), differ))
}

const TAGGED: &str =
    "(SELECT DISTINCT from_entity_id FROM public.relations WHERE type_id = $1::uuid)";

type TableRow = (Uuid, Uuid, Uuid, f64, Option<i64>);

async fn read_table(conn: &mut PgConnection) -> Result<BTreeMap<FeedKey, FeedRow>, IndexerError> {
    let rows: Vec<TableRow> = sqlx::query_as(
        "SELECT topic_id, type_id, entity_id, ranking_score::float8, created_epoch \
         FROM public.entity_topic_ranking",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(t, k, e, s, c)| ((t, k, e), (s, c)))
        .collect())
}

/// What `reconcile_entity_topic_ranking()` reads, taken just before it runs.
struct ReconcileBefore {
    graph: Graph,
    scores: HashMap<Uuid, f64>,
    table: BTreeMap<FeedKey, FeedRow>,
}

async fn reconcile_before(conn: &mut PgConnection) -> Result<ReconcileBefore, IndexerError> {
    let rows: Vec<RelRow> = sqlx::query_as(&format!(
        "SELECT from_entity_id, to_entity_id, type_id FROM public.relations \
         WHERE from_entity_id IN {TAGGED} AND type_id IN ($1::uuid, $2::uuid, $3::uuid, $4::uuid)"
    ))
    .bind(TOPICS)
    .bind(TYPES)
    .bind(SYSTEM_TYPE)
    .bind(TAG_RELATION)
    .fetch_all(&mut *conn)
    .await?;
    let names: Vec<(Uuid, Uuid, Option<String>)> = sqlx::query_as(&format!(
        "SELECT entity_id, property_id, text FROM public.values \
         WHERE property_id = $2::uuid AND entity_id IN {TAGGED}"
    ))
    .bind(TOPICS)
    .bind(NAME)
    .fetch_all(&mut *conn)
    .await?;
    let excluded: Vec<Uuid> =
        sqlx::query_scalar("SELECT type_id FROM public.entity_type_exclusions")
            .fetch_all(&mut *conn)
            .await?;
    let created: HashMap<Uuid, Option<i64>> = sqlx::query_as::<_, (Uuid, Option<String>)>(
        &format!("SELECT id, created_at FROM public.entities WHERE id IN {TAGGED}"),
    )
    .bind(TOPICS)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(id, c)| (id, c.as_deref().and_then(created_epoch)))
    .collect();
    let scores: HashMap<Uuid, f64> = sqlx::query_as::<_, (Uuid, f64)>(&format!(
        "SELECT entity_id, ranking_score::float8 FROM public.entity_ranking_scores \
         WHERE entity_id IN {TAGGED}"
    ))
    .bind(TOPICS)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    let values: Vec<Value> = names
        .into_iter()
        .map(|(entity, property, text)| Value {
            entity,
            property,
            text,
        })
        .collect();
    Ok(ReconcileBefore {
        graph: Graph::new(&rels(rows), &values, &excluded, created),
        scores,
        table: read_table(conn).await?,
    })
}

/// After the reconcile: the count the code would have returned, and the rows of the table now that
/// differ from the table the code would have left. Returns `(rust count, rows that differ)`.
async fn reconcile_shadow(
    conn: &mut PgConnection,
    before: &ReconcileBefore,
) -> Result<(usize, usize), IndexerError> {
    let plan = before.graph.reconcile(&before.scores, &before.table);
    let now = read_table(conn).await?;
    let differ = plan
        .table
        .iter()
        .filter(|(k, v)| now.get(*k) != Some(*v))
        .count()
        + now.keys().filter(|k| !plan.table.contains_key(*k)).count();
    Ok((plan.changed(), differ))
}

/// Streams the table into `visit`, one row at a time, so the whole table is never held twice.
async fn stream_table(
    conn: &mut PgConnection,
    mut visit: impl FnMut(Uuid, Uuid, Uuid, f64),
) -> Result<(), IndexerError> {
    let mut rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, f64)>(topic_ranking_mirror::SELECT_SQL)
        .fetch(&mut *conn);
    while let Some((t, k, e, s)) = rows.try_next().await? {
        visit(t, k, e, s);
    }
    Ok(())
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

    let started = Instant::now();
    // The app role's statement_timeout is 30s and this pass takes 20-24s, so it gets its own
    // limit. SET LOCAL inside one transaction rather than a session SET: the connection goes
    // through a transaction-mode pooler, where a session setting may land on another backend.
    // A failure returns Err, so the process exits non-zero and the Job is marked failed.
    // Off: the debate-score refresh and the reconcile are skipped as one write (they share a
    // transaction with the shadows and the geo-db diff, which only describe what they changed),
    // so the whole pass is skipped and counted.
    pg_bin_writes::run(PgWriteSite::TopicRankingReconcile, async {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL statement_timeout = '10min'")
            .execute(&mut *tx)
            .await?;
        let shadow = rust_compute_enabled();
        // The two SQL calls below write `entity_topic_ranking` without telling geo-db. When the derived
        // mirror is on (GEODB_DERIVED_MIRROR, default off), note the table first and diff it after.
        let mirror = DerivedMirror::from_env().filter(|m| m.allows(topic_ranking_mirror::TABLE));
        let mut diff = None;
        if mirror.is_some() {
            let mut d = Diff::default();
            match stream_table(&mut tx, |t, k, e, s| d.before(t, k, e, s)).await {
                Ok(()) => diff = Some(d),
                Err(e) => {
                    warn!(error = %e, "topic ranking geo-db mirror skipped: before-state read failed")
                }
            }
        }
        let topic_debates: i32 = sqlx::query_scalar("SELECT public.refresh_topic_debate_scores()")
            .fetch_one(&mut *tx)
            .await?;
        if shadow {
            // A savepoint, so a failed shadow read cannot abort the transaction the job commits.
            let mut sp = tx.begin().await?;
            match debate_shadow(&mut sp).await {
                Ok((ids, 0)) if ids as i64 == i64::from(topic_debates) => {
                    info!(ids, "topic debate rust compute matches the SQL")
                }
                Ok((ids, differ)) => warn!(
                    rust_ids = ids,
                    sql_scored = topic_debates,
                    scores_differ = differ,
                    "topic debate rust compute differs from the SQL"
                ),
                Err(e) => warn!(error = %e, "topic debate rust compute skipped"),
            }
            let _ = sp.rollback().await;
        }
        // Read after the score refresh, because that is what the reconcile sees.
        let before = if shadow {
            let mut sp = tx.begin().await?;
            let read = reconcile_before(&mut sp).await;
            let _ = sp.rollback().await;
            match read {
                Ok(b) => Some(b),
                Err(e) => {
                    warn!(error = %e, "topic ranking rust compute skipped: before-state read failed");
                    None
                }
            }
        } else {
            None
        };
        let changed: i32 = sqlx::query_scalar("SELECT public.reconcile_entity_topic_ranking()")
            .fetch_one(&mut *tx)
            .await?;
        if let Some(before) = &before {
            let mut sp = tx.begin().await?;
            match reconcile_shadow(&mut sp, before).await {
                Ok((n, 0)) if n as i64 == i64::from(changed) => {
                    info!(changed = n, "topic ranking rust compute matches the SQL")
                }
                Ok((n, differ)) => warn!(
                    rust_changed = n,
                    sql_changed = changed,
                    rows_differ = differ,
                    "topic ranking rust compute differs from the SQL"
                ),
                Err(e) => warn!(error = %e, "topic ranking rust compute skipped"),
            }
            let _ = sp.rollback().await;
        }
        // The after state is read in the transaction, so a failed read here cannot be mistaken for a
        // delete of every row; it is sent only once the commit has gone through.
        let planned = match diff {
            Some(mut d) => match stream_table(&mut tx, |t, k, e, s| d.after(t, k, e, s)).await {
                Ok(()) => Some(d.finish()),
                Err(e) => {
                    warn!(error = %e, "topic ranking geo-db mirror skipped: after-state read failed");
                    None
                }
            },
            None => None,
        };
        tx.commit().await?;
        if let (Some(m), Some((puts, deletes))) = (&mirror, planned) {
            info!(
                puts = puts.len(),
                deletes = deletes.len(),
                "topic ranking geo-db mirror"
            );
            topic_ranking_mirror::mirror(m, &puts, &deletes).await;
        }
        info!(
            topic_debates,
            changed,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "topic ranking reconciled"
        );
        Ok::<_, IndexerError>(())
    })
    .await?;
    Ok(())
}
