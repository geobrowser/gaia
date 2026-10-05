//! End-to-end test for the topic interest CronJobs (GEO-3088, migration 0100), through the same
//! Rust entry points the `topic_interest` binary calls, against a real Postgres built from the api's
//! migrations.
//!
//! The acceptance criteria are asserted in detail by the SQL suite
//! `api/drizzle/tests/0100_user_topic_interest.sql`. This covers what that suite cannot: the
//! transaction and timeout wrapping here, the cursor surviving across separate commits, a new vote
//! landing on the next sweep, the nightly refit agreeing with the result, and the sweep yielding to
//! a refit that holds the lock in ANOTHER session (the SQL suite runs in one transaction, where the
//! advisory lock is re-entrant).
//!
//! Same database variable and CI behaviour as `e2e.rs`: unset locally skips, unset in CI panics.

use chrono::{DateTime, Duration, DurationRound, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use tokio::sync::Mutex;
use uuid::Uuid;

use ranking_indexer::topic_interest::{refit, sweep, SweepOutcome};

fn e2e_database_url() -> Option<String> {
    match std::env::var("RANKING_INDEXER_E2E_DATABASE_URL") {
        Ok(url) => Some(url),
        Err(_) if std::env::var("CI").is_ok() => panic!(
            "RANKING_INDEXER_E2E_DATABASE_URL is unset in CI. These tests would skip and \
             report as passed. Provision the database in the workflow."
        ),
        Err(_) => {
            eprintln!("skipping topic interest e2e: RANKING_INDEXER_E2E_DATABASE_URL not set");
            None
        }
    }
}

/// Both tests move the one sweep cursor and the refit rewrites every user, so they must not
/// interleave.
static SERIAL: Mutex<()> = Mutex::const_new(());

const TOPICS: &str = "806d52bc-27e9-4c91-93c0-57978b093351";
const LOCK_KEY: &str = "personalization.user_topic_interest";

struct Fixture {
    voter: Uuid,
    bystander: Uuid,
    topic: Uuid,
    claims: Vec<Uuid>,
}

async fn pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(3)
        .connect(url)
        .await
        .expect("connect to the e2e database")
}

async fn setup(pool: &PgPool, as_of: DateTime<Utc>) -> Fixture {
    let f = Fixture {
        voter: Uuid::new_v4(),
        bystander: Uuid::new_v4(),
        topic: Uuid::new_v4(),
        claims: (0..3).map(|_| Uuid::new_v4()).collect(),
    };
    for (id, name) in [(f.voter, "voter"), (f.bystander, "bystander")] {
        sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'Personal', $2)")
            .bind(id)
            .bind(format!("e2e-topic-interest-{name}-{id}"))
            .execute(pool)
            .await
            .unwrap();
    }
    let created = (as_of - Duration::days(60)).timestamp().to_string();
    for claim in &f.claims {
        sqlx::query(
            "INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) \
             VALUES ($1, $2, '0', $2, '0')",
        )
        .bind(claim)
        .bind(&created)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) \
             VALUES ($1, $2, $3::uuid, $4, $5, $6, false)",
        )
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(TOPICS)
        .bind(claim)
        .bind(f.topic)
        .bind(f.bystander)
        .execute(pool)
        .await
        .unwrap();
    }
    // Two votes, two days old: one agree, one disagree.
    for (claim, vote_type) in f.claims.iter().take(2).zip([0i16, 1]) {
        vote(pool, f.voter, *claim, vote_type, as_of - Duration::days(2)).await;
    }
    f
}

async fn vote(pool: &PgPool, user: Uuid, claim: Uuid, vote_type: i16, at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at) \
         VALUES ($1, $2, 0, $1, $3, 1, $4)",
    )
    .bind(user)
    .bind(claim)
    .bind(vote_type)
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
}

async fn weight(
    pool: &PgPool,
    user: Uuid,
    topic: Uuid,
    at: DateTime<Utc>,
) -> Option<(f64, String, i32)> {
    sqlx::query(
        "SELECT weight, top_kind, top_kind_count \
         FROM personalization.user_topic_weights($1, 100, $2) WHERE topic_id = $3",
    )
    .bind(user)
    .bind(at)
    .bind(topic)
    .fetch_optional(pool)
    .await
    .unwrap()
    .map(|r| (r.get(0), r.get(1), r.get(2)))
}

async fn teardown(pool: &PgPool, f: &Fixture) {
    let users = vec![f.voter, f.bystander];
    sqlx::query("DELETE FROM user_votes WHERE user_id = ANY($1)")
        .bind(&users)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM relations WHERE from_entity_id = ANY($1)")
        .bind(&f.claims)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM entities WHERE id = ANY($1)")
        .bind(&f.claims)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM personalization.user_topic_signals WHERE user_id = ANY($1)")
        .bind(&users)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&users)
        .execute(pool)
        .await
        .unwrap();
}

/// Second resolution, so a timestamp survives the round trip through Postgres unchanged.
fn now() -> DateTime<Utc> {
    Utc::now().duration_trunc(Duration::seconds(1)).unwrap()
}

#[tokio::test]
async fn a_new_vote_lands_on_the_next_sweep_and_the_refit_agrees() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let pool = pool(&url).await;
    let t0 = now();
    let f = setup(&pool, t0).await;

    // Start from a known cursor: one sweep an hour ago, so the votes two days old are older than
    // the lookback and only a backfill would see them. Then force that backfill.
    sqlx::query("DELETE FROM personalization.interest_sweep_state WHERE id = 'incremental'")
        .execute(&pool)
        .await
        .unwrap();
    let SweepOutcome::Ran(first) = sweep(&pool, Some(t0)).await.unwrap() else {
        panic!("the first sweep was skipped with no refit running");
    };
    assert!(
        first.dirty_users >= 1,
        "the backfill found nobody: {first:?}"
    );

    let (w, kind, count) = weight(&pool, f.voter, f.topic, t0)
        .await
        .expect("voter has a weight");
    let expected = 2.0 * 0.5f64.powf(2.0 / 30.0);
    assert!(
        (w - expected).abs() < 1e-9,
        "two votes two days old: {w} vs {expected}"
    );
    assert_eq!(
        (kind.as_str(), count),
        ("vote", 2),
        "the reason is the two votes"
    );
    assert!(
        weight(&pool, f.bystander, f.topic, t0).await.is_none(),
        "a user with no activity has no weight"
    );

    // A new vote a minute later reaches the next sweep, which sees only that user.
    vote(&pool, f.voter, f.claims[2], 0, t0 + Duration::minutes(1)).await;
    let t1 = t0 + Duration::minutes(2);
    let SweepOutcome::Ran(second) = sweep(&pool, Some(t1)).await.unwrap() else {
        panic!("the second sweep was skipped");
    };
    assert_eq!(
        second.since,
        t0 - Duration::hours(1),
        "looks back an hour from the last run"
    );
    let (w1, _, count1) = weight(&pool, f.voter, f.topic, t1).await.unwrap();
    assert_eq!(count1, 3, "the new vote is counted");
    assert!(
        w1 > w + 0.99,
        "the new vote raised the weight by about 1: {w} -> {w1}"
    );

    // The nightly refit, from scratch, agrees with what the sweeps left.
    let report = refit(&pool, Some(t1 + Duration::minutes(1))).await.unwrap();
    assert_eq!(
        report.disagreeing_rows, 0,
        "refit disagreed with the sweeps: {report:?}"
    );
    assert!(report.max_abs_diff < 1e-9, "{report:?}");
    let recorded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM personalization.interest_refit_runs WHERE ran_at = $1",
    )
    .bind(t1 + Duration::minutes(1))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recorded, 1, "the refit records its comparison");

    teardown(&pool, &f).await;
}

#[tokio::test]
async fn the_sweep_skips_while_the_refit_holds_the_lock() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let pool = pool(&url).await;
    let before: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_run_at FROM personalization.interest_sweep_state WHERE id = 'incremental'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();

    // Another session holds the refit's lock, as a running refit would.
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(LOCK_KEY)
        .execute(&mut *holder)
        .await
        .unwrap();

    let later = now() + Duration::days(365);
    assert_eq!(
        sweep(&pool, Some(later)).await.unwrap(),
        SweepOutcome::Skipped,
        "the sweep must not wait behind, or run alongside, a refit"
    );
    let after: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_run_at FROM personalization.interest_sweep_state WHERE id = 'incremental'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(before, after, "a skipped sweep leaves the cursor alone");

    holder.rollback().await.unwrap();
    assert!(
        matches!(
            sweep(&pool, Some(now())).await.unwrap(),
            SweepOutcome::Ran(_)
        ),
        "once the lock is released the sweep runs"
    );
}
