//! Integration tests for PostgreSQL cursor repository implementation.
//!
//! These tests require a real PostgreSQL database and use SQLx test macros
//! to ensure proper test isolation and cleanup.
//!
//! Run with: `cargo test --test postgres_cursor`

use actions_indexer_repository::{CursorRepository, PostgresCursorRepository};
use sqlx::Row;

/// Creates test cursor data for testing.
fn make_test_cursor_data() -> (&'static str, &'static str, i64) {
    ("test_indexer_1", "cursor_12345abcdef", 1000)
}

// ============================================================================
// Basic Cursor Operations Tests
// ============================================================================

#[sqlx::test(migrations = "src/postgres/migrations")]
async fn test_save_and_get_cursor(pool: sqlx::PgPool) {
    let repository = PostgresCursorRepository::new(pool.clone()).await.unwrap();
    let (id, cursor, block_number) = make_test_cursor_data();

    // Save cursor
    repository
        .save_cursor(id, cursor, &block_number)
        .await
        .unwrap();

    // Get cursor
    let retrieved_cursor = repository.get_cursor(id).await.unwrap();
    assert!(retrieved_cursor.is_some());
    assert_eq!(retrieved_cursor.unwrap(), cursor);

    // Verify in database
    let row = sqlx::query("SELECT id, cursor, block_number FROM meta WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<String, _>("id"), id);
    assert_eq!(row.get::<String, _>("cursor"), cursor);
    assert_eq!(row.get::<String, _>("block_number"), block_number.to_string());
}

#[sqlx::test(migrations = "src/postgres/migrations")]
async fn test_get_nonexistent_cursor(pool: sqlx::PgPool) {
    let repository = PostgresCursorRepository::new(pool.clone()).await.unwrap();

    let result = repository.get_cursor("nonexistent_id").await.unwrap();
    assert!(result.is_none());
}

#[sqlx::test(migrations = "src/postgres/migrations")]
async fn test_update_existing_cursor(pool: sqlx::PgPool) {
    let repository = PostgresCursorRepository::new(pool.clone()).await.unwrap();
    let (id, initial_cursor, initial_block) = make_test_cursor_data();

    // Save initial cursor
    repository
        .save_cursor(id, initial_cursor, &initial_block)
        .await
        .unwrap();

    // Update cursor
    let updated_cursor = "updated_cursor_67890xyz";
    let updated_block = 2000;
    repository
        .save_cursor(id, updated_cursor, &updated_block)
        .await
        .unwrap();

    // Get updated cursor
    let retrieved_cursor = repository.get_cursor(id).await.unwrap();
    assert!(retrieved_cursor.is_some());
    assert_eq!(retrieved_cursor.unwrap(), updated_cursor);

    // Verify only one record exists
    let count = sqlx::query("SELECT COUNT(*) as count FROM meta WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.get::<i64, _>("count"), 1);

    // Verify values in database
    let row = sqlx::query("SELECT cursor, block_number FROM meta WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("cursor"), updated_cursor);
    assert_eq!(row.get::<String, _>("block_number"), updated_block.to_string());
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[sqlx::test(migrations = "src/postgres/migrations")]
async fn test_empty_string_values(pool: sqlx::PgPool) {
    let repository = PostgresCursorRepository::new(pool.clone()).await.unwrap();

    // Test with empty cursor string
    repository
        .save_cursor("test_empty", "", &123)
        .await
        .unwrap();

    let result = repository.get_cursor("test_empty").await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), "");
}

#[sqlx::test(migrations = "src/postgres/migrations")]
async fn test_special_characters_in_id_and_cursor(pool: sqlx::PgPool) {
    let repository = PostgresCursorRepository::new(pool.clone()).await.unwrap();

    let special_id = "test-id_with.special@chars";
    let special_cursor = "cursor_with-special.chars@123/456";

    repository
        .save_cursor(special_id, special_cursor, &789)
        .await
        .unwrap();

    let result = repository.get_cursor(special_id).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), special_cursor);
}

// ============================================================================
// Repository Creation Tests
// ============================================================================

#[sqlx::test(migrations = "src/postgres/migrations")]
async fn test_repository_creation(pool: sqlx::PgPool) {
    // Test that repository can be created successfully
    let repository = PostgresCursorRepository::new(pool.clone()).await.unwrap();

    // Verify it's usable by doing a simple operation
    let result = repository.get_cursor("test_creation").await.unwrap();
    assert!(result.is_none());
}

#[sqlx::test(migrations = "src/postgres/migrations")]
async fn test_multiple_repository_instances(pool: sqlx::PgPool) {
    // Create multiple repository instances
    let repo1 = PostgresCursorRepository::new(pool.clone()).await.unwrap();
    let repo2 = PostgresCursorRepository::new(pool.clone()).await.unwrap();

    // Save with one instance
    repo1
        .save_cursor("multi_repo_test", "cursor_from_repo1", &123)
        .await
        .unwrap();

    // Read with another instance
    let result = repo2.get_cursor("multi_repo_test").await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), "cursor_from_repo1");
}
