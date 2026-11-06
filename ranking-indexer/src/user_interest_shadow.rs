//! Shadow run of the Rust twin of the user-interest compute (`user_interest`) against the SQL.
//!
//! After a sweep has committed, [`run`] takes a bounded random sample of the users the sweep just
//! wrote, reads exactly the inputs the SQL read for them, computes their signals with
//! `user_interest::{derive_events, compute_signals}` and counts the rows that differ from what the
//! SQL stored ([`user_interest::count_signal_mismatches`]).
//!
//! It never writes and never changes the job: every read is in one read-only `REPEATABLE READ`
//! transaction (so the stored rows and the inputs are one snapshot), the caller logs and carries on
//! when it fails, and it only runs when `USER_INTEREST_RUST_SHADOW` is `1` or `true` (default off).
//! A vote that lands between the sweep and the shadow's snapshot is excluded by `voted_at`, but a
//! comment or debate that lands in that gap can still show as a mismatch, so read a few stray
//! mismatches as noise and a steady count as a twin bug. One known source: a comment with two
//! equally deep Reply to roots, which the SQL resolves by physical row order.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use tracing::info;
use uuid::Uuid;

use crate::error::IndexerError;
use crate::user_interest::{
    compute_signals, count_signal_mismatches, derive_events, External, Graph, Rel, Signal, Vote,
    Weight, CLAIMS, MAX_THREAD_DEPTH, PARTICIPANTS, REPLY_TO, TOPICS, TYPES,
};

/// How many users one run compares. Bounded so the shadow stays small next to the sweep.
pub const SAMPLE_USERS: i64 = 200;

/// What one shadow run found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowReport {
    pub users: usize,
    /// Rows the SQL stored for the sampled users.
    pub stored_rows: usize,
    /// Rows the Rust twin computed for them.
    pub rust_rows: usize,
    /// Rows that differ (missing on either side, or a different value).
    pub mismatches: usize,
}

fn us(t: DateTime<Utc>) -> i64 {
    t.timestamp_micros()
}

/// Samples the users of the latest sweep (or refit) and compares them. `None` when nothing was
/// stored.
pub async fn run(pool: &PgPool) -> Result<Option<ShadowReport>, IndexerError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '60s'")
        .execute(&mut *tx)
        .await?;
    let report = compare(&mut tx).await?;
    tx.rollback().await?;
    Ok(report)
}

/// Runs [`run`], logs counts only, and records the metric. Never returns an error.
pub async fn run_and_log(pool: &PgPool) {
    match run(pool).await {
        Ok(Some(r)) => {
            crate::metrics::user_interest_shadow(r.stored_rows, r.mismatches);
            info!(
                users = r.users,
                stored_rows = r.stored_rows,
                rust_rows = r.rust_rows,
                mismatches = r.mismatches,
                "user interest shadow compared the Rust twin with the SQL"
            );
        }
        Ok(None) => info!("user interest shadow: no stored signals to compare"),
        Err(e) => tracing::warn!(error = %e, "user interest shadow skipped: read failed"),
    }
}

async fn compare(tx: &mut Transaction<'_, Postgres>) -> Result<Option<ShadowReport>, IndexerError> {
    let Some(as_of): Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT max(computed_at) FROM personalization.user_topic_signals")
            .fetch_one(&mut **tx)
            .await?
    else {
        return Ok(None);
    };
    let users: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM (SELECT DISTINCT user_id FROM personalization.user_topic_signals \
         WHERE computed_at = $1) u ORDER BY random() LIMIT $2",
    )
    .bind(as_of)
    .bind(SAMPLE_USERS)
    .fetch_all(&mut **tx)
    .await?;
    if users.is_empty() {
        return Ok(None);
    }

    let stored: Vec<Signal> = sqlx::query_as::<_, StoredRow>(
        "SELECT user_id, topic_id, kind, decayed_count, event_count, last_event_at, top_object_id \
         FROM personalization.user_topic_signals WHERE user_id = ANY($1)",
    )
    .bind(&users)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(Signal::from)
    .collect();

    let (weights, half_life_days) = read_config(tx).await?;
    let mut graph = load_graph(tx, &users, as_of).await?;
    let events = derive_events(&graph, &users);

    // The Topics of every object an event credits. A topic event is its own topic.
    let objects: Vec<Uuid> = events
        .iter()
        .filter(|e| !e.object_is_topic)
        .map(|e| e.object)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    graph
        .relations
        .extend(relations_from(tx, &objects, &[TOPICS]).await?);
    let mut topics: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for r in graph.relations.iter().filter(|r| r.type_id == TOPICS) {
        topics.entry(r.from).or_default().push(r.to);
    }
    let rust = compute_signals(
        &events,
        &weights,
        |o| topics.get(&o).cloned().unwrap_or_default(),
        half_life_days,
        us(as_of),
    );
    Ok(Some(ShadowReport {
        users: users.len(),
        stored_rows: stored.len(),
        rust_rows: rust.len(),
        mismatches: count_signal_mismatches(&stored, &rust),
    }))
}

#[derive(sqlx::FromRow)]
struct StoredRow {
    user_id: Uuid,
    topic_id: Uuid,
    kind: String,
    decayed_count: f64,
    event_count: i32,
    last_event_at: Option<DateTime<Utc>>,
    top_object_id: Uuid,
}

impl From<StoredRow> for Signal {
    fn from(r: StoredRow) -> Self {
        Signal {
            user: r.user_id,
            topic: r.topic_id,
            kind: r.kind,
            decayed_count: r.decayed_count,
            event_count: r.event_count,
            last_event_us: r.last_event_at.map(us),
            top_object: r.top_object_id,
        }
    }
}

async fn read_config(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(HashMap<String, Weight>, f64), IndexerError> {
    let rows: Vec<(String, f64, bool)> =
        sqlx::query_as("SELECT kind, weight, decays FROM personalization.interest_signal_weights")
            .fetch_all(&mut **tx)
            .await?;
    let half_life: f64 =
        sqlx::query_scalar("SELECT half_life_days FROM personalization.interest_config")
            .fetch_one(&mut **tx)
            .await?;
    Ok((
        rows.into_iter()
            .map(|(k, weight, decays)| (k, Weight { weight, decays }))
            .collect(),
        half_life,
    ))
}

type RelRow = (Uuid, Uuid, Uuid, Uuid);
type ExternalRow = (Uuid, Uuid, String, DateTime<Utc>, DateTime<Utc>);
type VoteRow = (Uuid, Uuid, i16, i16, i16, DateTime<Utc>);

fn rel(r: RelRow) -> Rel {
    Rel {
        type_id: r.0,
        from: r.1,
        to: r.2,
        space: r.3,
    }
}

/// Relations of `types` leaving `from`.
async fn relations_from(
    tx: &mut Transaction<'_, Postgres>,
    from: &[Uuid],
    types: &[Uuid],
) -> Result<Vec<Rel>, IndexerError> {
    if from.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<RelRow> = sqlx::query_as(
        "SELECT type_id, from_entity_id, to_entity_id, space_id FROM public.relations \
         WHERE from_entity_id = ANY($1) AND type_id = ANY($2)",
    )
    .bind(from)
    .bind(types)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(rel).collect())
}

/// The inputs `derive_events` reads for `users`, and no more: their votes and external signals, the
/// Reply to chains above their comments, the debates they joined with those debates' type, claims
/// and topics, the Topic type of the things they follow, and the `created_at` of the comments and
/// debates. (`entity_updated` is only for `dirty_users`, which the shadow does not run.)
async fn load_graph(
    tx: &mut Transaction<'_, Postgres>,
    users: &[Uuid],
    as_of: DateTime<Utc>,
) -> Result<Graph, IndexerError> {
    let mut g = Graph::default();
    let personal: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM public.spaces WHERE id = ANY($1) AND type = 'Personal'")
            .bind(users)
            .fetch_all(&mut **tx)
            .await?;
    g.personal_spaces = personal.into_iter().collect();

    // Votes up to the moment the SQL computed, so a vote cast since is not counted against it.
    let votes: Vec<VoteRow> = sqlx::query_as(
        "SELECT user_id, object_id, object_type, vote_kind, vote_type, voted_at \
         FROM public.user_votes \
         WHERE user_id = ANY($1) AND object_type = 0 AND vote_type IN (0, 1) AND voted_at <= $2",
    )
    .bind(users)
    .bind(as_of)
    .fetch_all(&mut **tx)
    .await?;
    g.votes = votes
        .into_iter()
        .map(
            |(user, object, object_type, vote_kind, vote_type, at)| Vote {
                user,
                object,
                object_type,
                vote_kind,
                vote_type,
                voted_at_us: us(at),
            },
        )
        .collect();

    let external: Vec<ExternalRow> = sqlx::query_as(
        "SELECT user_id, object_id, kind, occurred_at, recorded_at \
         FROM personalization.external_interest_signals WHERE user_id = ANY($1)",
    )
    .bind(users)
    .fetch_all(&mut **tx)
    .await?;
    g.external = external
        .into_iter()
        .map(|(user, object, kind, occurred, recorded)| External {
            user,
            object,
            kind,
            occurred_us: us(occurred),
            recorded_us: us(recorded),
        })
        .collect();

    // Comments: Reply to relations authored in the users' own spaces, then each target's own Reply
    // to chain, one level per round, as far as the SQL walks.
    let rows: Vec<RelRow> = sqlx::query_as(
        "SELECT type_id, from_entity_id, to_entity_id, space_id FROM public.relations \
         WHERE space_id = ANY($1) AND type_id = $2",
    )
    .bind(users)
    .bind(REPLY_TO)
    .fetch_all(&mut **tx)
    .await?;
    let mut comment_ids: HashSet<Uuid> = rows.iter().map(|r| r.1).collect();
    let mut seen_from: HashSet<Uuid> = comment_ids.clone();
    let mut frontier: Vec<Uuid> = rows
        .iter()
        .map(|r| r.2)
        .filter(|t| seen_from.insert(*t))
        .collect();
    g.relations.extend(rows.into_iter().map(rel));
    for _ in 0..MAX_THREAD_DEPTH {
        if frontier.is_empty() {
            break;
        }
        let up = relations_from(tx, &frontier, &[REPLY_TO]).await?;
        frontier = up
            .iter()
            .map(|r| r.to)
            .filter(|t| seen_from.insert(*t))
            .collect();
        g.relations.extend(up);
    }

    // Debates the users took part in, and what makes each a debate.
    let rows: Vec<RelRow> = sqlx::query_as(
        "SELECT type_id, from_entity_id, to_entity_id, space_id FROM public.relations \
         WHERE to_entity_id = ANY($1) AND type_id = $2",
    )
    .bind(users)
    .bind(PARTICIPANTS)
    .fetch_all(&mut **tx)
    .await?;
    let debates: Vec<Uuid> = rows
        .iter()
        .map(|r| r.1)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    g.relations.extend(rows.into_iter().map(rel));
    g.relations
        .extend(relations_from(tx, &debates, &[TYPES, CLAIMS, TOPICS]).await?);

    // Whether each voted-on object is a Topic (only an Interested vote looks).
    let interested: Vec<Uuid> = g
        .votes
        .iter()
        .filter(|v| v.vote_kind == 3)
        .map(|v| v.object)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    g.relations
        .extend(relations_from(tx, &interested, &[TYPES]).await?);

    comment_ids.extend(debates.iter().copied());
    let ids: Vec<Uuid> = comment_ids.into_iter().collect();
    let created: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, created_at, updated_at FROM public.entities WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    for (id, c, u) in created {
        g.entity_created.insert(id, Some(c));
        g.entity_updated.insert(id, Some(u));
    }
    Ok(g)
}
