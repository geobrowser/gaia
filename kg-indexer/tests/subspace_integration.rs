//! Integration tests for subspace storage operations.
//!
//! These tests verify that typed subspace insert/remove operations
//! work correctly against a real PostgreSQL database.
//!
//! Prerequisites:
//! - PostgreSQL running with migrations applied (including 0045)
//! - DATABASE_URL environment variable set
//!
//! Run with: cargo test --package kg-indexer --test subspace_integration -- --ignored

use sqlx::postgres::PgPoolOptions;
use std::env;
use uuid::Uuid;

async fn get_pool() -> sqlx::Pool<sqlx::Postgres> {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database")
}

/// Insert a verified subspace, then unverify it, verify the row is gone.
#[tokio::test]
#[ignore]
async fn test_insert_verified_then_unverify() {
    let pool = get_pool().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    // Insert
    sqlx::query(
        r#"INSERT INTO subspaces (parent_space_id, child_space_id, type)
           VALUES ($1, $2, 'verified'::"subspaceType")
           ON CONFLICT DO NOTHING"#,
    )
    .bind(parent)
    .bind(child)
    .execute(&pool)
    .await
    .expect("insert failed");

    // Verify it exists
    let count: (i64,) = sqlx::query_as(
        r#"SELECT COUNT(*) FROM subspaces
           WHERE parent_space_id = $1 AND child_space_id = $2 AND type = 'verified'::"subspaceType""#,
    )
    .bind(parent)
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1, "verified row should exist after insert");

    // Remove
    sqlx::query(
        r#"DELETE FROM subspaces
           WHERE parent_space_id = $1 AND child_space_id = $2 AND type = 'verified'::"subspaceType""#,
    )
    .bind(parent)
    .bind(child)
    .execute(&pool)
    .await
    .expect("delete failed");

    // Verify it's gone
    let count: (i64,) = sqlx::query_as(
        r#"SELECT COUNT(*) FROM subspaces
           WHERE parent_space_id = $1 AND child_space_id = $2 AND type = 'verified'::"subspaceType""#,
    )
    .bind(parent)
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 0, "verified row should be gone after delete");
}

/// Insert verified + related for the same pair, remove one, verify the other survives.
#[tokio::test]
#[ignore]
async fn test_dual_edges_remove_one_survives() {
    let pool = get_pool().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    // Insert both types
    sqlx::query(
        r#"INSERT INTO subspaces (parent_space_id, child_space_id, type)
           VALUES ($1, $2, 'verified'::"subspaceType"), ($1, $2, 'related'::"subspaceType")
           ON CONFLICT DO NOTHING"#,
    )
    .bind(parent)
    .bind(child)
    .execute(&pool)
    .await
    .expect("insert failed");

    // Verify both exist
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM subspaces WHERE parent_space_id = $1 AND child_space_id = $2",
    )
    .bind(parent)
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 2, "both edges should exist");

    // Remove only verified
    sqlx::query(
        r#"DELETE FROM subspaces
           WHERE parent_space_id = $1 AND child_space_id = $2 AND type = 'verified'::"subspaceType""#,
    )
    .bind(parent)
    .bind(child)
    .execute(&pool)
    .await
    .expect("delete failed");

    // Verify related survives
    let remaining: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM subspaces WHERE parent_space_id = $1 AND child_space_id = $2",
    )
    .bind(parent)
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining.0, 1, "related edge should survive");

    let type_val: (String,) = sqlx::query_as(
        "SELECT type::text FROM subspaces WHERE parent_space_id = $1 AND child_space_id = $2",
    )
    .bind(parent)
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(type_val.0, "related", "surviving edge should be related");

    // Cleanup
    sqlx::query("DELETE FROM subspaces WHERE parent_space_id = $1 AND child_space_id = $2")
        .bind(parent)
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();
}

/// Insert a topic edge, then remove it, verify it's gone from subspace_topics.
#[tokio::test]
#[ignore]
async fn test_insert_topic_then_remove() {
    let pool = get_pool().await;
    let space = Uuid::new_v4();
    let topic = Uuid::new_v4();

    // Insert
    sqlx::query(
        "INSERT INTO subspace_topics (space_id, topic_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(space)
    .bind(topic)
    .execute(&pool)
    .await
    .expect("insert failed");

    // Verify it exists
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM subspace_topics WHERE space_id = $1 AND topic_id = $2",
    )
    .bind(space)
    .bind(topic)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1, "topic edge should exist after insert");

    // Remove
    sqlx::query("DELETE FROM subspace_topics WHERE space_id = $1 AND topic_id = $2")
        .bind(space)
        .bind(topic)
        .execute(&pool)
        .await
        .expect("delete failed");

    // Verify it's gone
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM subspace_topics WHERE space_id = $1 AND topic_id = $2",
    )
    .bind(space)
    .bind(topic)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 0, "topic edge should be gone after delete");
}

/// Idempotency: inserting the same subspace twice should not error.
#[tokio::test]
#[ignore]
async fn test_insert_subspace_idempotent() {
    let pool = get_pool().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    for _ in 0..2 {
        sqlx::query(
            r#"INSERT INTO subspaces (parent_space_id, child_space_id, type)
               VALUES ($1, $2, 'verified'::"subspaceType")
               ON CONFLICT DO NOTHING"#,
        )
        .bind(parent)
        .bind(child)
        .execute(&pool)
        .await
        .expect("idempotent insert should not error");
    }

    let count: (i64,) = sqlx::query_as(
        r#"SELECT COUNT(*) FROM subspaces
           WHERE parent_space_id = $1 AND child_space_id = $2 AND type = 'verified'::"subspaceType""#,
    )
    .bind(parent)
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count.0, 1,
        "should have exactly 1 row after duplicate insert"
    );

    // Cleanup
    sqlx::query("DELETE FROM subspaces WHERE parent_space_id = $1 AND child_space_id = $2")
        .bind(parent)
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();
}

/// Removing a non-existent subspace should be a no-op (not error).
#[tokio::test]
#[ignore]
async fn test_remove_nonexistent_is_noop() {
    let pool = get_pool().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    // Delete something that doesn't exist — should succeed
    let result = sqlx::query(
        r#"DELETE FROM subspaces
           WHERE parent_space_id = $1 AND child_space_id = $2 AND type = 'verified'::"subspaceType""#,
    )
    .bind(parent)
    .bind(child)
    .execute(&pool)
    .await
    .expect("delete of nonexistent should not error");

    assert_eq!(result.rows_affected(), 0);
}
