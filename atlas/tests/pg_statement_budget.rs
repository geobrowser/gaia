//! The checkpoint store's statement budget must never be left on a pooled connection.
//!
//! Atlas reaches Postgres through a PgBouncer pool in transaction mode that other services
//! share, so a session-level `SET statement_timeout` would stay on the server connection and
//! apply to the next client handed it. The budget is set with `SET LOCAL` per transaction.
//!
//! Needs a Postgres: set `ATLAS_PG_TEST_DATABASE_URL` (CI's Atlas E2E workflow does).
//! Skipped otherwise.

use atlas::persistence::{Checkpoint, PostgresCheckpointStore, PostgresPoolConfig};
use sqlx::{PgConnection, PgPool};

const BUDGET_MS: u64 = 4_321;

fn database_url() -> Option<String> {
    std::env::var("ATLAS_PG_TEST_DATABASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
}

fn pool_config() -> PostgresPoolConfig {
    PostgresPoolConfig {
        max_connections: 4,
        min_connections: 0,
        acquire_timeout_ms: 5_000,
        idle_timeout_ms: 60_000,
        max_lifetime_ms: 600_000,
        statement_timeout_ms: BUDGET_MS,
    }
}

async fn ensure_table(pool: &PgPool) {
    // Same shape as the api migrations that own this table.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS atlas_checkpoints (
            indexer_id text PRIMARY KEY NOT NULL,
            cursor text NOT NULL,
            block_number bigint NOT NULL,
            graph_state_version smallint NOT NULL,
            runtime_compatibility_marker text NOT NULL,
            root_space_id text NOT NULL,
            schema_version smallint NOT NULL,
            graph_state_blob jsonb NOT NULL,
            updated_at timestamp with time zone DEFAULT now() NOT NULL,
            emission_baseline_blob bytea
        )",
    )
    .execute(pool)
    .await
    .expect("create atlas_checkpoints");
}

/// `statement_timeout` as the connection reports it, and where that value came from.
async fn timeout_and_source(conn: &mut PgConnection) -> (String, String) {
    sqlx::query_as(
        "SELECT current_setting('statement_timeout'), source
           FROM pg_settings WHERE name = 'statement_timeout'",
    )
    .fetch_one(conn)
    .await
    .expect("read statement_timeout")
}

fn checkpoint(indexer_id: &str) -> Checkpoint {
    Checkpoint {
        schema_version: 1,
        indexer_id: indexer_id.to_string(),
        cursor: "cursor-1".to_string(),
        block_number: 1,
        graph_state_blob: serde_json::json!({}),
        graph_state_version: 1,
        runtime_compatibility_marker: "pg-statement-budget-test".to_string(),
        root_space_id: "root".to_string(),
        emission_baseline_blob: None,
    }
}

async fn assert_no_session_budget(pool: &PgPool, context: &str) {
    // Hold every pooled connection at once so each one is checked, not just the first.
    let mut conns = Vec::new();
    for _ in 0..pool_config().max_connections {
        conns.push(pool.acquire().await.expect("acquire"));
    }
    for conn in conns.iter_mut() {
        let (value, source) = timeout_and_source(conn).await;
        assert_ne!(
            source, "session",
            "{context}: a pooled connection carries a session-level statement_timeout ({value})"
        );
        assert_ne!(
            value, "4321ms",
            "{context}: the checkpoint budget leaked onto a pooled connection"
        );
    }
}

#[tokio::test]
async fn checkpoint_store_leaves_no_session_statement_timeout() {
    let Some(url) = database_url() else {
        eprintln!("Skipping pg_statement_budget (set ATLAS_PG_TEST_DATABASE_URL to enable)");
        return;
    };
    let store = PostgresCheckpointStore::new(&url, &pool_config()).expect("store");
    let pool = store.pg_pool().expect("pool").clone();
    ensure_table(&pool).await;

    assert_no_session_budget(&pool, "fresh pool").await;

    let id = format!("pg-budget-{}", std::process::id());
    store.save(&checkpoint(&id)).await.expect("save");
    let loaded = store.load(&id).await.expect("load").expect("row");
    assert_eq!(loaded.cursor, "cursor-1");
    assert!(store.indexer_ids().await.expect("ids").contains(&id));

    assert_no_session_budget(&pool, "after save, load and list").await;

    sqlx::query("DELETE FROM atlas_checkpoints WHERE indexer_id = $1")
        .bind(&id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn set_local_budget_applies_in_transaction_and_ends_with_it() {
    let Some(url) = database_url() else {
        eprintln!("Skipping pg_statement_budget (set ATLAS_PG_TEST_DATABASE_URL to enable)");
        return;
    };
    let store = PostgresCheckpointStore::new(&url, &pool_config()).expect("store");
    let pool = store.pg_pool().expect("pool").clone();

    // The statement the store runs at the start of each transaction sets the budget there.
    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(sqlx::AssertSqlSafe(
        atlas::persistence::set_local_statement_timeout_sql(BUDGET_MS as i32),
    ))
    .execute(&mut *tx)
    .await
    .expect("set local");
    let (inside, _) = timeout_and_source(&mut tx).await;
    assert_eq!(inside, "4321ms");
    tx.commit().await.expect("commit");

    assert_no_session_budget(&pool, "after a budgeted transaction").await;
}
