//! Runs the user-interest SQL (migrations 0100, 0107) and its Rust twin side by side on a small
//! synthetic fixture, in a real Postgres built from the api's migrations.
//!
//! The fixture is invented: random ids, no real data. It covers votes (agree, disagree, removal,
//! several spaces), follows, comment chains (deep, cyclic, a DAO author, an unparseable
//! timestamp), claim and topic debates, a debate that is not typed Debate, and external signals.
//!
//! Same database variable and CI behaviour as `e2e.rs`: unset locally skips, unset in CI panics.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, DurationRound, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use ranking_indexer::user_interest::{
    compute_signals, count_signal_mismatches, derive_events, dirty_users, Event, External, Graph,
    Rel, Signal, Vote, Weight, CLAIMS, DEBATE_TYPE, PARTICIPANTS, REPLY_TO, TOPICS, TOPIC_TYPE,
    TYPES,
};
use ranking_indexer::user_interest_shadow;

fn e2e_database_url() -> Option<String> {
    match std::env::var("RANKING_INDEXER_E2E_DATABASE_URL") {
        Ok(url) => Some(url),
        Err(_) if std::env::var("CI").is_ok() => panic!(
            "RANKING_INDEXER_E2E_DATABASE_URL is unset in CI. These tests would skip and \
             report as passed. Provision the database in the workflow."
        ),
        Err(_) => {
            eprintln!(
                "skipping user interest shadow e2e: RANKING_INDEXER_E2E_DATABASE_URL not set"
            );
            None
        }
    }
}

/// The tests rewrite `user_topic_signals`, so they must not interleave.
static SERIAL: Mutex<()> = Mutex::const_new(());

type VoteRow = (Uuid, Uuid, i16, i16, i16, DateTime<Utc>);
type ExternalRow = (Uuid, Uuid, String, DateTime<Utc>, DateTime<Utc>);
type EventRow = (Uuid, String, Uuid, Uuid, bool, Option<DateTime<Utc>>);
type SignalRow = (Uuid, Uuid, String, f64, i32, Option<DateTime<Utc>>, Uuid);

fn id() -> Uuid {
    Uuid::new_v4()
}

struct Fx {
    users: Vec<Uuid>,
    a: Uuid,
    entities: Vec<Uuid>,
    all_space_ids: Vec<Uuid>,
}

fn secs(t: DateTime<Utc>) -> String {
    t.timestamp().to_string()
}

async fn entity(pool: &PgPool, id: Uuid, created: &str, updated: &str) {
    sqlx::query(
        "INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) \
         VALUES ($1, $2, '0', $3, '0')",
    )
    .bind(id)
    .bind(created)
    .bind(updated)
    .execute(pool)
    .await
    .unwrap();
}

async fn rel(pool: &PgPool, ty: Uuid, from: Uuid, to: Uuid, space: Uuid) {
    sqlx::query(
        "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, \
         is_system) VALUES ($1, $2, $3, $4, $5, $6, false)",
    )
    .bind(id())
    .bind(id())
    .bind(ty)
    .bind(from)
    .bind(to)
    .bind(space)
    .execute(pool)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn vote(
    pool: &PgPool,
    user: Uuid,
    object: Uuid,
    space: Uuid,
    vote_type: i16,
    kind: i16,
    at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, \
         voted_at) VALUES ($1, $2, 0, $3, $4, $5, $6)",
    )
    .bind(user)
    .bind(object)
    .bind(space)
    .bind(vote_type)
    .bind(kind)
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
}

async fn external(pool: &PgPool, user: Uuid, object: Uuid, kind: &str, at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO personalization.external_interest_signals \
         (user_id, object_id, kind, occurred_at, recorded_at) VALUES ($1, $2, $3, $4, $4)",
    )
    .bind(user)
    .bind(object)
    .bind(kind)
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
}

async fn setup(pool: &PgPool, now: DateTime<Utc>) -> Fx {
    let (a, b, c, dao) = (id(), id(), id(), id());
    for (sid, ty) in [
        (a, "Personal"),
        (b, "Personal"),
        (c, "Personal"),
        (dao, "DAO"),
    ] {
        sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, $2::\"spaceTypes\", $3)")
            .bind(sid)
            .bind(ty)
            .bind(format!("e2e-shadow-{sid}"))
            .execute(pool)
            .await
            .unwrap();
    }
    let old = secs(now - Duration::days(40));
    let recent = secs(now - Duration::hours(3));
    let mut entities = Vec::new();
    let mut ent = |e: Uuid| {
        entities.push(e);
        e
    };

    let (t1, t2) = (ent(id()), ent(id()));
    let (c1, c2, c3, c4) = (ent(id()), ent(id()), ent(id()), ent(id()));
    for e in [t1, t2, c1, c2, c3, c4] {
        entity(pool, e, &old, &old).await;
    }
    rel(pool, TYPES, t1, TOPIC_TYPE, a).await;
    rel(pool, TOPICS, c1, t1, a).await;
    rel(pool, TOPICS, c2, t1, a).await;
    rel(pool, TOPICS, c2, t2, a).await;
    rel(pool, TOPICS, c3, t2, a).await;

    // Votes: agree, disagree, a removal (dropped), a second space for the same object, an
    // Interested on a Topic in two spaces (one follow), an Interested on a claim (a vote), a DAO.
    vote(pool, a, c1, a, 0, 1, now - Duration::days(2)).await;
    vote(pool, a, c2, a, 1, 2, now - Duration::days(5)).await;
    vote(pool, a, c2, b, 0, 1, now - Duration::days(1)).await;
    vote(pool, a, c3, a, 2, 1, now - Duration::days(1)).await;
    vote(pool, b, c1, b, 0, 1, now - Duration::days(9)).await;
    vote(pool, a, t1, a, 0, 3, now - Duration::days(3)).await;
    vote(pool, a, t1, b, 0, 3, now - Duration::days(1)).await;
    vote(pool, a, c4, a, 0, 3, now - Duration::days(4)).await;
    vote(pool, c, t2, c, 0, 3, now - Duration::days(30)).await;
    vote(pool, dao, c1, dao, 0, 1, now - Duration::days(1)).await;

    // Comments. m1 on c1; m2 (B) and m3 (A) reply to m1: thread root c1. m4 replies to c2.
    // m5/m6 reply to each other (a cycle). m7 is authored by the DAO. m8 has a timestamp that is
    // not epoch seconds. No comment has two equally deep roots: the SQL's DISTINCT ON orders by
    // depth only, so which root a tie picks is down to physical row order and cannot be matched.
    let ms: Vec<Uuid> = (0..8).map(|_| ent(id())).collect();
    for (i, m) in ms.iter().enumerate() {
        let created = if i == 7 {
            "not-a-number".to_string()
        } else {
            secs(now - Duration::hours(10 * (i as i64 + 1)))
        };
        entity(pool, *m, &created, &recent).await;
    }
    rel(pool, REPLY_TO, ms[0], c1, a).await;
    rel(pool, REPLY_TO, ms[1], ms[0], b).await;
    rel(pool, REPLY_TO, ms[2], ms[0], a).await;
    rel(pool, REPLY_TO, ms[3], c2, a).await;
    rel(pool, REPLY_TO, ms[4], ms[5], a).await;
    rel(pool, REPLY_TO, ms[5], ms[4], a).await;
    rel(pool, REPLY_TO, ms[6], c1, dao).await;
    rel(pool, REPLY_TO, ms[7], c4, a).await;

    // Debates. d1: claim debate on c3, A and B. d2: topic debate under t2, A. d3: not typed
    // Debate. d4: has a Claims relation AND a Topics one (the claim wins).
    let ds: Vec<Uuid> = (0..4).map(|_| ent(id())).collect();
    for (i, d) in ds.iter().enumerate() {
        entity(pool, *d, &secs(now - Duration::days(i as i64 + 1)), &recent).await;
    }
    for d in [ds[0], ds[1], ds[3]] {
        rel(pool, TYPES, d, DEBATE_TYPE, a).await;
    }
    rel(pool, CLAIMS, ds[0], c3, a).await;
    rel(pool, PARTICIPANTS, ds[0], a, a).await;
    rel(pool, PARTICIPANTS, ds[0], b, a).await;
    rel(pool, TOPICS, ds[1], t2, a).await;
    rel(pool, PARTICIPANTS, ds[1], a, a).await;
    rel(pool, PARTICIPANTS, ds[2], a, a).await;
    rel(pool, TOPICS, ds[2], t1, a).await;
    rel(pool, CLAIMS, ds[3], c1, a).await;
    rel(pool, TOPICS, ds[3], t2, a).await;
    rel(pool, PARTICIPANTS, ds[3], b, a).await;

    external(pool, a, c1, "not_interested", now - Duration::days(1)).await;
    external(pool, c, c2, "not_interested", now - Duration::days(2)).await;
    external(pool, dao, c2, "not_interested", now - Duration::days(2)).await;

    Fx {
        users: vec![a, b, c, dao],
        a,
        entities,
        all_space_ids: vec![a, b, c, dao],
    }
}

async fn teardown(pool: &PgPool, f: &Fx) {
    let q = [
        "DELETE FROM user_votes WHERE user_id = ANY($1)",
        "DELETE FROM personalization.external_interest_signals WHERE user_id = ANY($1)",
        "DELETE FROM personalization.user_topic_signals WHERE user_id = ANY($1)",
        "DELETE FROM relations WHERE from_entity_id = ANY($2) OR space_id = ANY($1)",
        "DELETE FROM entities WHERE id = ANY($2)",
        "DELETE FROM spaces WHERE id = ANY($1)",
    ];
    for s in q {
        let mut query = sqlx::query(s).bind(&f.all_space_ids);
        if s.contains("$2") {
            query = query.bind(&f.entities);
        }
        query.execute(pool).await.unwrap();
    }
}

fn us(t: DateTime<Utc>) -> i64 {
    t.timestamp_micros()
}

/// Every row of the input tables, as the Rust twin reads them. Fine for a fixture; the shipped
/// loader is bounded to a sample of users.
async fn whole_graph(pool: &PgPool) -> Graph {
    let mut g = Graph::default();
    let spaces: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM spaces WHERE type = 'Personal'")
        .fetch_all(pool)
        .await
        .unwrap();
    g.personal_spaces = spaces.into_iter().collect();
    let votes: Vec<VoteRow> = sqlx::query_as(
        "SELECT user_id, object_id, object_type, vote_kind, vote_type, voted_at FROM user_votes",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    g.votes = votes
        .into_iter()
        .map(
            |(user, object, object_type, vote_kind, vote_type, t)| Vote {
                user,
                object,
                object_type,
                vote_kind,
                vote_type,
                voted_at_us: us(t),
            },
        )
        .collect();
    let rels: Vec<(Uuid, Uuid, Uuid, Uuid)> =
        sqlx::query_as("SELECT type_id, from_entity_id, to_entity_id, space_id FROM relations")
            .fetch_all(pool)
            .await
            .unwrap();
    g.relations = rels
        .into_iter()
        .map(|(type_id, from, to, space)| Rel {
            type_id,
            from,
            to,
            space,
        })
        .collect();
    let ents: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, created_at, updated_at FROM entities")
            .fetch_all(pool)
            .await
            .unwrap();
    for (e, c, u) in ents {
        g.entity_created.insert(e, Some(c));
        g.entity_updated.insert(e, Some(u));
    }
    let ext: Vec<ExternalRow> = sqlx::query_as(
        "SELECT user_id, object_id, kind, occurred_at, recorded_at \
         FROM personalization.external_interest_signals",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    g.external = ext
        .into_iter()
        .map(|(user, object, kind, o, r)| External {
            user,
            object,
            kind,
            occurred_us: us(o),
            recorded_us: us(r),
        })
        .collect();
    g
}

async fn sql_events(pool: &PgPool, users: &[Uuid]) -> Vec<Event> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT user_id, kind, source_id, object_id, object_is_topic, occurred_at \
         FROM personalization.user_interest_events($1)",
    )
    .bind(users)
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out: Vec<Event> = rows
        .into_iter()
        .map(|(user, kind, source, object, object_is_topic, t)| Event {
            user,
            kind,
            source,
            object,
            object_is_topic,
            occurred_us: t.map(us),
        })
        .collect();
    out.sort();
    out
}

async fn sql_signals(pool: &PgPool, users: &[Uuid], as_of: DateTime<Utc>) -> Vec<Signal> {
    let rows: Vec<SignalRow> = sqlx::query_as(
        "SELECT user_id, topic_id, kind, decayed_count, event_count, last_event_at, top_object_id \
         FROM personalization.compute_user_topic_signals($1, $2)",
    )
    .bind(users)
    .bind(as_of)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(
            |(user, topic, kind, decayed_count, event_count, t, top_object)| Signal {
                user,
                topic,
                kind,
                decayed_count,
                event_count,
                last_event_us: t.map(us),
                top_object,
            },
        )
        .collect()
}

async fn weights(pool: &PgPool) -> (HashMap<String, Weight>, f64) {
    let rows: Vec<(String, f64, bool)> =
        sqlx::query_as("SELECT kind, weight, decays FROM personalization.interest_signal_weights")
            .fetch_all(pool)
            .await
            .unwrap();
    let hl: f64 = sqlx::query_scalar("SELECT half_life_days FROM personalization.interest_config")
        .fetch_one(pool)
        .await
        .unwrap();
    (
        rows.into_iter()
            .map(|(k, weight, decays)| (k, Weight { weight, decays }))
            .collect(),
        hl,
    )
}

fn now() -> DateTime<Utc> {
    Utc::now().duration_trunc(Duration::seconds(1)).unwrap()
}

async fn connect(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(3)
        .connect(url)
        .await
        .expect("connect to the e2e database")
}

#[tokio::test]
async fn the_rust_twin_matches_the_sql_on_events_signals_and_dirty_users() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let pool = connect(&url).await;
    let t0 = now();
    let f = setup(&pool, t0).await;

    let g = whole_graph(&pool).await;
    let sql = sql_events(&pool, &f.users).await;
    let rust = derive_events(&g, &f.users);
    assert!(sql.len() >= 12, "the fixture should be rich: {sql:#?}");
    assert_eq!(sql, rust, "events differ");
    let kinds: HashSet<&str> = sql.iter().map(|e| e.kind.as_str()).collect();
    for k in ["vote", "comment", "debate", "follow", "not_interested"] {
        assert!(kinds.contains(k), "fixture has no {k} event");
    }

    let (w, half_life) = weights(&pool).await;
    let topics_of = |o: Uuid| -> Vec<Uuid> {
        g.relations
            .iter()
            .filter(|r| r.type_id == TOPICS && r.from == o)
            .map(|r| r.to)
            .collect()
    };
    for as_of in [t0, t0 + Duration::days(20)] {
        let sql_sig = sql_signals(&pool, &f.users, as_of).await;
        let rust_sig = compute_signals(&rust, &w, topics_of, half_life, us(as_of));
        assert!(!sql_sig.is_empty());
        assert_eq!(
            count_signal_mismatches(&sql_sig, &rust_sig),
            0,
            "signals differ as of {as_of}:\nsql  {sql_sig:#?}\nrust {rust_sig:#?}"
        );
        assert_eq!(sql_sig.len(), rust_sig.len());
    }

    for since in [
        t0 - Duration::days(365),
        t0 - Duration::days(35),
        t0 - Duration::days(2),
        t0 - Duration::hours(4),
        t0 + Duration::hours(1),
    ] {
        let mut sql_dirty: Vec<Uuid> =
            sqlx::query_scalar("SELECT unnest(personalization.dirty_interest_users($1))")
                .bind(since)
                .fetch_all(&pool)
                .await
                .unwrap();
        sql_dirty.sort();
        assert_eq!(
            sql_dirty,
            dirty_users(&g, us(since)),
            "dirty users since {since}"
        );
    }
    assert!(f.users.contains(&f.a));
    teardown(&pool, &f).await;
}

#[tokio::test]
async fn the_shadow_run_finds_no_mismatch_after_the_sql_recompute_and_catches_a_tampered_row() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let pool = connect(&url).await;
    sqlx::query("DELETE FROM personalization.user_topic_signals")
        .execute(&pool)
        .await
        .unwrap();
    let t0 = now();
    let f = setup(&pool, t0).await;
    sqlx::query("SELECT personalization.recompute_user_topic_interest($1, $2)")
        .bind(&f.users)
        .bind(t0)
        .execute(&pool)
        .await
        .unwrap();

    let report = user_interest_shadow::run(&pool)
        .await
        .unwrap()
        .expect("the recompute stored rows");
    assert!(report.stored_rows > 0, "{report:?}");
    assert_eq!(report.stored_rows, report.rust_rows, "{report:?}");
    assert_eq!(report.mismatches, 0, "{report:?}");

    // The shadow never writes: the stored rows are what the SQL left.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM personalization.user_topic_signals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n as usize, report.stored_rows);

    // A row the SQL got "wrong" is counted, and a missing row too.
    sqlx::query(
        "UPDATE personalization.user_topic_signals SET decayed_count = decayed_count + 1 \
         WHERE ctid = (SELECT ctid FROM personalization.user_topic_signals LIMIT 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let r = user_interest_shadow::run(&pool).await.unwrap().unwrap();
    assert_eq!(r.mismatches, 1, "{r:?}");
    sqlx::query(
        "DELETE FROM personalization.user_topic_signals \
         WHERE ctid = (SELECT ctid FROM personalization.user_topic_signals LIMIT 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let r = user_interest_shadow::run(&pool).await.unwrap().unwrap();
    assert!(r.mismatches >= 1, "{r:?}");

    teardown(&pool, &f).await;
}

#[tokio::test]
async fn the_shadow_has_nothing_to_compare_when_no_signals_are_stored() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let pool = connect(&url).await;
    sqlx::query("DELETE FROM personalization.user_topic_signals")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(user_interest_shadow::run(&pool).await.unwrap(), None);
}
