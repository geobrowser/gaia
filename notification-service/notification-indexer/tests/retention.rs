//! Integration test for retention cleanup (`delete_expired_notifications`).
//!
//! Runs only when `NOTIF_TEST_DATABASE_URL` points at a throwaway Postgres
//! (it DROPs/CREATEs the two tables it needs). Verifies the retention window,
//! FK-safe delete order (deliveries before outbox), the "still-referenced
//! outbox is kept" guard, and that batching drains everything.

use notification_indexer::storage::Storage;
use sqlx::{PgPool, Row};

const SCHEMA: &str = r#"
DROP TABLE IF EXISTS notification_deliveries;
DROP TABLE IF EXISTS notification_outbox;
CREATE TABLE notification_outbox (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    idempotency_key text NOT NULL UNIQUE,
    event_type text NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE notification_deliveries (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    outbox_id uuid NOT NULL REFERENCES notification_outbox(id),
    webhook_id uuid NOT NULL DEFAULT gen_random_uuid(),
    status text NOT NULL DEFAULT 'pending',
    created_at timestamptz NOT NULL DEFAULT now()
);
"#;

/// Insert one outbox row (with an explicit age in days) and return its id.
async fn seed_outbox(pool: &PgPool, key: &str, age_days: i64) -> uuid::Uuid {
    seed_outbox_typed(pool, key, age_days, "proposal_created").await
}

/// Insert one outbox row of a given event_type and age; return its id.
async fn seed_outbox_typed(
    pool: &PgPool,
    key: &str,
    age_days: i64,
    event_type: &str,
) -> uuid::Uuid {
    let row = sqlx::query(
        "INSERT INTO notification_outbox (idempotency_key, event_type, created_at)
         VALUES ($1, $2, now() - make_interval(days => $3::int))
         RETURNING id",
    )
    .bind(key)
    .bind(event_type)
    .bind(age_days as i32)
    .fetch_one(pool)
    .await
    .expect("insert outbox");
    row.get("id")
}

async fn seed_delivery(pool: &PgPool, outbox_id: uuid::Uuid, age_days: i64) {
    sqlx::query(
        "INSERT INTO notification_deliveries (outbox_id, created_at)
         VALUES ($1, now() - make_interval(days => $2::int))",
    )
    .bind(outbox_id)
    .bind(age_days as i32)
    .execute(pool)
    .await
    .expect("insert delivery");
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    let row = sqlx::query(&format!("SELECT count(*) AS c FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count");
    row.get::<i64, _>("c")
}

#[tokio::test]
async fn retention_deletes_old_rows_fk_safely() {
    let Ok(url) = std::env::var("NOTIF_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping retention_deletes_old_rows_fk_safely: set NOTIF_TEST_DATABASE_URL to run"
        );
        return;
    };

    let storage = Storage::connect(&url).await.expect("connect");
    let pool = storage.pool().clone();

    for stmt in SCHEMA.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(stmt)
            .execute(&pool)
            .await
            .expect("apply schema");
    }

    // A,B: old outbox + old delivery → both removed.
    let a = seed_outbox(&pool, "a", 40).await;
    seed_delivery(&pool, a, 40).await;
    let b = seed_outbox(&pool, "b", 40).await;
    seed_delivery(&pool, b, 40).await;
    // C: recent outbox + recent delivery → both kept.
    let c = seed_outbox(&pool, "c", 0).await;
    seed_delivery(&pool, c, 0).await;
    // D: OLD outbox but a RECENT delivery → both kept (delivery survives the
    // window, so the outbox is still referenced and must not be deleted).
    let d = seed_outbox(&pool, "d", 40).await;
    seed_delivery(&pool, d, 0).await;
    // E: old outbox, no deliveries → removed.
    seed_outbox(&pool, "e", 40).await;
    // F,G: old poller-ledger rows with no deliveries → KEPT (deleting them would
    // let the rejection / vote-threshold pollers re-emit).
    seed_outbox_typed(&pool, "f", 40, "proposal_rejected").await;
    seed_outbox_typed(&pool, "g", 40, "entity_votes_threshold").await;

    // batch_size = 2 forces the drain loop to iterate more than once.
    let (deliveries_deleted, outbox_deleted) = storage
        .delete_expired_notifications(30, 2)
        .await
        .expect("retention cleanup");

    assert_eq!(
        deliveries_deleted, 2,
        "old deliveries A,B removed (C,D survive)"
    );
    assert_eq!(
        outbox_deleted, 3,
        "old unreferenced outbox A,B,E removed (C kept; D referenced; F,G are ledgers)"
    );
    assert_eq!(
        count(&pool, "notification_deliveries").await,
        2,
        "C,D deliveries remain"
    );
    assert_eq!(
        count(&pool, "notification_outbox").await,
        4,
        "C,D + ledger rows F,G remain"
    );
}
