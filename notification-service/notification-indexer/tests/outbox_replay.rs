//! Integration test for the `outbox-replay` Postgres source (`PgSource`, `read_only_pool`).
//!
//! Runs only when `NOTIF_TEST_DATABASE_URL` points at a throwaway Postgres (it DROPs/CREATEs the two
//! tables it needs, so don't aim it at a real DB). The schema mirrors api/drizzle/0052 (the
//! `app_webhooks` foreign key is left out; the test needs no webhook rows). Verifies the window
//! bounds (`--from` inclusive, `--to` exclusive), the join to delivery rows, cursor paging, and that
//! the session is read-only.

use chrono::{DateTime, TimeZone, Utc};
use notification_indexer::outbox_replay::{
    read_only_pool, run, Options, PgSource, Row, RowSource, Sink,
};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

const SCHEMA: &str = r#"
DROP TABLE IF EXISTS notification_deliveries;
DROP TABLE IF EXISTS notification_outbox;
CREATE TABLE notification_outbox (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    idempotency_key text NOT NULL UNIQUE,
    event_type text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE notification_deliveries (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    outbox_id uuid NOT NULL REFERENCES notification_outbox(id),
    webhook_id uuid NOT NULL,
    status text NOT NULL DEFAULT 'pending',
    attempts smallint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(outbox_id, webhook_id)
);
"#;

fn t(sec: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + sec, 0).unwrap()
}

/// Seed one outbox row at `t(sec)` with the given webhook delivery rows; returns its id.
async fn seed(pool: &PgPool, n: u128, sec: i64, id: u128, hooks: &[u128]) -> Uuid {
    let id = Uuid::from_u128(id);
    sqlx::query(
        "INSERT INTO notification_outbox (id, idempotency_key, event_type, payload, created_at)
         VALUES ($1, $2, 'proposal_created', $3, $4)",
    )
    .bind(id)
    .bind(format!("{n:064x}"))
    .bind(serde_json::json!({"n": n}))
    .bind(t(sec))
    .execute(pool)
    .await
    .unwrap();
    for h in hooks {
        sqlx::query("INSERT INTO notification_deliveries (outbox_id, webhook_id) VALUES ($1, $2)")
            .bind(id)
            .bind(Uuid::from_u128(*h))
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

async fn all(src: &mut PgSource, batch: usize) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    loop {
        let after = out.last().map(|r| (r.created_at, r.id));
        let page = src.page(after, batch).await.unwrap();
        if page.is_empty() {
            return out;
        }
        out.extend(page);
    }
}

#[derive(Default)]
struct Count(u64);
impl Sink for Count {
    async fn enter(
        &mut self,
        _: &geodb_outbox::Unit,
    ) -> Result<geodb_outbox::Entered, geodb_outbox::MirrorError> {
        self.0 += 1;
        Ok(geodb_outbox::Entered::New)
    }
}

#[tokio::test]
async fn pg_source_window_join_paging_and_read_only() {
    let Ok(url) = std::env::var("NOTIF_TEST_DATABASE_URL") else {
        eprintln!("skipping pg_source_window_join_paging_and_read_only: set NOTIF_TEST_DATABASE_URL to run");
        return;
    };
    let admin = PgPool::connect(&url).await.unwrap();
    sqlx::raw_sql(SCHEMA).execute(&admin).await.unwrap();

    // Rows at t=0..5. Two share t=3 (tie broken by id). Row at t=0 is before the window, t=5 is
    // exactly `to`.
    seed(&admin, 1, 0, 1, &[]).await;
    seed(&admin, 2, 1, 2, &[30, 10, 20]).await; // == from, included; hooks come back sorted
    seed(&admin, 3, 2, 3, &[]).await; // no delivery rows -> empty webhooks
    seed(&admin, 4, 3, 5, &[40]).await;
    seed(&admin, 5, 3, 4, &[]).await;
    seed(&admin, 6, 5, 6, &[60]).await; // == to, excluded
    seed(&admin, 7, 6, 7, &[]).await;

    let mut src = PgSource {
        pool: read_only_pool(&url).await.unwrap(),
        from: t(1),
        to: t(5),
    };

    // Window: from inclusive, to exclusive, ordered by (created_at, id).
    let rows = all(&mut src, 100).await;
    let ids: Vec<u128> = rows.iter().map(|r| r.id.as_u128()).collect();
    assert_eq!(ids, vec![2, 3, 4, 5]);

    // Join: webhooks sorted; none when the row has no delivery rows.
    let hooks = |i: usize| -> Vec<u128> { rows[i].webhooks.iter().map(|w| w.as_u128()).collect() };
    assert_eq!(hooks(0), vec![10, 20, 30]);
    assert!(hooks(1).is_empty());
    assert!(hooks(2).is_empty());
    assert_eq!(hooks(3), vec![40]);
    assert_eq!(rows[0].idempotency_key, format!("{:064x}", 2));
    assert_eq!(rows[0].event_type, "proposal_created");
    assert_eq!(rows[0].payload, serde_json::json!({"n": 2}));

    // Paging by cursor with a tiny batch (spanning the t=3 tie) yields the same rows once each.
    let paged = all(&mut src, 1).await;
    assert_eq!(paged, rows);
    assert_eq!(src.page(None, 2).await.unwrap().len(), 2);

    // The full loop over the real source (dry run, then apply) sees exactly the window.
    let opts = |apply| Options {
        batch: 3,
        interval: Duration::ZERO,
        max_units: None,
        apply,
    };
    let dry = run(&mut src, &mut Count::default(), &opts(false))
        .await
        .unwrap();
    assert_eq!((dry.scanned, dry.would_enter), (4, 4));
    let mut sink = Count::default();
    let rep = run(&mut src, &mut sink, &opts(true)).await.unwrap();
    assert_eq!((rep.scanned, rep.entered, sink.0), (4, 4, 4));

    // An empty window reads nothing.
    let mut empty = PgSource {
        pool: read_only_pool(&url).await.unwrap(),
        from: t(10),
        to: t(20),
    };
    assert!(all(&mut empty, 10).await.is_empty());

    // The session is read-only: a write through the replay pool is refused; the admin pool can write.
    let ro = read_only_pool(&url).await.unwrap();
    let err = sqlx::query("DELETE FROM notification_outbox")
        .execute(&ro)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("read-only transaction"), "{err}");
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM notification_outbox")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(n, 7);
}
