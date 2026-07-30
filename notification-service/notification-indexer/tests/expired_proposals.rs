//! Integration test for `find_expired_proposals` and the rejection floor.
//!
//! Runs only when `NOTIF_TEST_DATABASE_URL` points at a throwaway Postgres
//! (it DROPs/CREATEs the tables it needs).
//!
//! Two regressions are covered here.
//!
//! 1. The query used to read `FROM proposals`, taking `end_time` off it. The
//!    governance-v2 split (`0067_governance_v2`) moved `end_time` to
//!    `proposal_versions` and left `proposals` as an identity table, so the
//!    query started failing with `column p.end_time does not exist` — every
//!    minute, for the life of the v2 cluster, with rejection notifications
//!    silently dead. The schema below mirrors v2: `end_time` exists ONLY on
//!    `proposal_versions`, so a regression to `FROM proposals` fails here
//!    rather than in production.
//!
//! 2. Simply fixing the query would have fired a rejection for every proposal
//!    the chain migration replayed (3,241 on testnet, back to 1970). The floor
//!    bounds that, and — because it is persisted rather than recomputed — an
//!    outage longer than the backfill window must NOT cause proposals to be
//!    skipped.

use notification_indexer::storage::{Storage, REJECTION_FLOOR_CURSOR};
use sqlx::PgPool;
use uuid::Uuid;

/// Mirrors the governance-v2 shape: `end_time` lives on `proposal_versions`,
/// `executed_at` stays on `proposals`, and `proposals_current` joins each
/// proposal to its current version.
const SCHEMA: &str = r#"
DROP VIEW IF EXISTS proposals_current;
DROP TABLE IF EXISTS proposal_versions;
DROP TABLE IF EXISTS proposals;
DROP TABLE IF EXISTS notification_outbox;
DROP TABLE IF EXISTS notification_poll_cursors;
CREATE TABLE proposals (
    id uuid PRIMARY KEY,
    space_id uuid NOT NULL,
    proposed_by uuid NOT NULL,
    executed_at bigint,
    current_version integer NOT NULL DEFAULT 1
);
CREATE TABLE proposal_versions (
    proposal_id uuid NOT NULL REFERENCES proposals(id),
    proposal_version integer NOT NULL,
    end_time bigint NOT NULL,
    name text,
    PRIMARY KEY (proposal_id, proposal_version)
);
CREATE VIEW proposals_current AS
    SELECT p.*, pv.proposal_version AS pv_version, pv.end_time, pv.name
    FROM proposals p
    JOIN proposal_versions pv
      ON pv.proposal_id = p.id AND pv.proposal_version = p.current_version;
CREATE TABLE notification_outbox (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    idempotency_key text NOT NULL UNIQUE,
    event_type text NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE notification_poll_cursors (
    name text PRIMARY KEY,
    cursor_updated_at timestamptz NOT NULL,
    cursor_id bigint NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
"#;

/// Insert a proposal whose current version ended `hours_ago` hours ago.
async fn seed_proposal(pool: &PgPool, name: &str, hours_ago: i64, executed: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO proposals (id, space_id, proposed_by, executed_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(if executed { Some(1_i64) } else { None })
    .execute(pool)
    .await
    .expect("insert proposal");

    sqlx::query(
        "INSERT INTO proposal_versions (proposal_id, proposal_version, end_time, name)
         VALUES ($1, 1, EXTRACT(EPOCH FROM now())::bigint - $2, $3)",
    )
    .bind(id)
    .bind(hours_ago * 3600)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert proposal_version");
    id
}

async fn connect() -> Option<PgPool> {
    let url = std::env::var("NOTIF_TEST_DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.expect("connect");
    for stmt in SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
        sqlx::query(stmt).execute(&pool).await.expect("schema");
    }
    Some(pool)
}

#[tokio::test]
async fn reads_end_time_from_the_current_version_not_the_proposals_table() {
    let Some(pool) = connect().await else {
        eprintln!("skipping: set NOTIF_TEST_DATABASE_URL to run");
        return;
    };
    let storage = Storage::new(pool.clone());

    let recent = seed_proposal(&pool, "recent", 2, false).await;
    seed_proposal(&pool, "executed", 2, true).await;

    // Floor of 10h: the 2h-old proposal is inside it.
    let found = storage
        .find_expired_proposals(100, floor_hours_ago(10))
        .await
        .expect("query must resolve against the v2 schema");

    let ids: Vec<Uuid> = found.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![recent], "only the unexecuted in-window proposal");
}

#[tokio::test]
async fn floor_excludes_the_replayed_migration_backlog() {
    let Some(pool) = connect().await else {
        eprintln!("skipping: set NOTIF_TEST_DATABASE_URL to run");
        return;
    };
    let storage = Storage::new(pool.clone());

    let inside = seed_proposal(&pool, "in window", 3, false).await;
    seed_proposal(&pool, "yesterday", 30, false).await;
    seed_proposal(&pool, "ancient (migration replay)", 24 * 365 * 40, false).await;

    let found = storage
        .find_expired_proposals(100, floor_hours_ago(10))
        .await
        .expect("query");
    assert_eq!(
        found.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![inside],
        "everything older than the floor stays silent"
    );

    // A floor of 0 means "no floor" — the historical backlog is exactly what we
    // are protecting against, so assert it WOULD come back without one.
    let unbounded = storage.find_expired_proposals(100, 0).await.expect("query");
    assert_eq!(
        unbounded.len(),
        3,
        "without a floor the whole backlog fires"
    );
}

#[tokio::test]
async fn already_notified_proposals_are_not_repeated() {
    let Some(pool) = connect().await else {
        eprintln!("skipping: set NOTIF_TEST_DATABASE_URL to run");
        return;
    };
    let storage = Storage::new(pool.clone());
    let id = seed_proposal(&pool, "already sent", 1, false).await;

    sqlx::query(
        "INSERT INTO notification_outbox (idempotency_key, event_type, payload)
         VALUES ('k1', 'proposal_rejected', jsonb_build_object('proposal_id', $1::text))",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("seed outbox");

    let found = storage
        .find_expired_proposals(100, floor_hours_ago(10))
        .await
        .expect("query");
    assert!(found.is_empty(), "the outbox anti-join dedupes");
}

#[tokio::test]
async fn floor_is_seeded_once_and_then_stable() {
    let Some(pool) = connect().await else {
        eprintln!("skipping: set NOTIF_TEST_DATABASE_URL to run");
        return;
    };
    let storage = Storage::new(pool.clone());

    let first = storage.rejection_floor(10).await.expect("seed");
    let persisted = storage
        .get_poll_cursor(REJECTION_FLOOR_CURSOR)
        .await
        .expect("read")
        .expect("floor row written on first call");
    assert_eq!(persisted.0.timestamp(), first);

    // The whole point of persisting: a later call with a DIFFERENT backfill
    // must not move the floor. If this recomputed `now - backfill` each time, a
    // restart after an outage longer than the window would silently skip every
    // proposal that expired during it.
    let second = storage.rejection_floor(999).await.expect("read back");
    assert_eq!(first, second, "an existing floor is never widened or moved");
}

fn floor_hours_ago(hours: i64) -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
        - hours * 3600
}
