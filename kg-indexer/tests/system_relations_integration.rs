//! Integration tests for system relations protection.
//!
//! These tests verify that relations marked with `is_system = true` cannot be
//! modified or deleted through the Storage methods.
//!
//! Prerequisites:
//! - PostgreSQL running with migrations applied (including is_system column)
//! - DATABASE_URL environment variable set
//!
//! Run with: cargo test --package kg-indexer --test system_relations_integration -- --ignored

use kg_indexer::models::relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem};
use kg_indexer::storage::Storage;
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

async fn setup_storage() -> Storage {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    Storage::new(&database_url)
        .await
        .expect("Failed to create storage")
}

/// Helper: insert a relation directly via SQL with is_system flag
#[allow(clippy::too_many_arguments)]
async fn insert_system_relation(
    pool: &sqlx::Pool<sqlx::Postgres>,
    id: Uuid,
    space_id: Uuid,
    entity_id: Uuid,
    from_entity_id: Uuid,
    to_entity_id: Uuid,
    type_id: Uuid,
    is_system: bool,
) {
    sqlx::query(
        r#"INSERT INTO relations (id, space_id, entity_id, from_entity_id, to_entity_id, type_id, position, verified, is_system)
           VALUES ($1, $2, $3, $4, $5, $6, 'original_position', true, $7)
           ON CONFLICT (id) DO UPDATE SET is_system = $7"#,
    )
    .bind(id)
    .bind(space_id)
    .bind(entity_id)
    .bind(from_entity_id)
    .bind(to_entity_id)
    .bind(type_id)
    .bind(is_system)
    .execute(pool)
    .await
    .expect("Failed to insert system relation");
}

/// Helper: ensure required entities exist (FK constraints)
async fn ensure_entity(pool: &sqlx::Pool<sqlx::Postgres>, id: Uuid) {
    sqlx::query(
        "INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
         VALUES ($1, '2024-01-01', '1', '2024-01-01', '1')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("Failed to insert entity");
}

/// Helper: query relation position and verified fields
async fn get_relation_fields(
    pool: &sqlx::Pool<sqlx::Postgres>,
    id: Uuid,
) -> Option<(Option<String>, Option<bool>)> {
    sqlx::query_as::<_, (Option<String>, Option<bool>)>(
        "SELECT position, verified FROM relations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .expect("Failed to query relation")
}

/// Helper: check if relation exists
async fn relation_exists(pool: &sqlx::Pool<sqlx::Postgres>, id: Uuid) -> bool {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM relations WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("Failed to count relations");
    count.0 > 0
}

/// Cleanup helper
async fn cleanup_relation(pool: &sqlx::Pool<sqlx::Postgres>, id: Uuid) {
    sqlx::query("DELETE FROM relations WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// Test: insert_relations (upsert) should NOT overwrite a system relation
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_insert_relations_does_not_overwrite_system_relation() {
    let pool = get_pool().await;
    let storage = setup_storage().await;

    let rel_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();

    // Ensure entities exist for FK constraints
    for id in [entity_id, from_id, to_id, type_id] {
        ensure_entity(&pool, id).await;
    }

    // Insert a system relation directly
    insert_system_relation(
        &pool, rel_id, space_id, entity_id, from_id, to_id, type_id, true,
    )
    .await;

    // Attempt to upsert via Storage (should be a no-op for system relations)
    let upsert = SetRelationItem {
        id: rel_id,
        entity_id,
        type_id,
        from_id,
        from_space_id: None,
        from_version_id: None,
        to_id,
        to_space_id: None,
        to_version_id: None,
        position: Some("overwritten_position".to_string()),
        space_id,
        verified: Some(false),
        is_system: false,
        context_root_id: None,
        context_edge_type_id: None,
    };

    let mut tx = pool.begin().await.unwrap();
    storage.insert_relations(&[upsert], &mut tx).await.unwrap();
    tx.commit().await.unwrap();

    // Assert: system relation should still have original values
    let (position, verified) = get_relation_fields(&pool, rel_id).await.unwrap();
    assert_eq!(
        position.as_deref(),
        Some("original_position"),
        "system relation position should not be overwritten"
    );
    assert_eq!(
        verified,
        Some(true),
        "system relation verified should not be overwritten"
    );

    cleanup_relation(&pool, rel_id).await;
}

// --------------------------------------------------------------------------
// Test: update_relations should NOT modify a system relation
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_update_relations_does_not_modify_system_relation() {
    let pool = get_pool().await;
    let storage = setup_storage().await;

    let rel_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();

    for id in [entity_id, from_id, to_id, type_id] {
        ensure_entity(&pool, id).await;
    }

    insert_system_relation(
        &pool, rel_id, space_id, entity_id, from_id, to_id, type_id, true,
    )
    .await;

    // Attempt to update via Storage
    let update = UpdateRelationItem {
        id: rel_id,
        from_space_id: None,
        from_version_id: None,
        to_space_id: None,
        to_version_id: None,
        position: Some("updated_position".to_string()),
        space_id,
        verified: Some(false),
        context_root_id: None,
        context_edge_type_id: None,
    };

    let mut tx = pool.begin().await.unwrap();
    storage.update_relations(&[update], &mut tx).await.unwrap();
    tx.commit().await.unwrap();

    // Assert: system relation should still have original values
    let (position, verified) = get_relation_fields(&pool, rel_id).await.unwrap();
    assert_eq!(
        position.as_deref(),
        Some("original_position"),
        "system relation position should not be updated"
    );
    assert_eq!(
        verified,
        Some(true),
        "system relation verified should not be updated"
    );

    cleanup_relation(&pool, rel_id).await;
}

// --------------------------------------------------------------------------
// Test: unset_relation_fields should NOT nullify fields on a system relation
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_unset_relation_fields_does_not_nullify_system_relation() {
    let pool = get_pool().await;
    let storage = setup_storage().await;

    let rel_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();

    for id in [entity_id, from_id, to_id, type_id] {
        ensure_entity(&pool, id).await;
    }

    insert_system_relation(
        &pool, rel_id, space_id, entity_id, from_id, to_id, type_id, true,
    )
    .await;

    // Attempt to unset position and verified via Storage
    let unset = UnsetRelationItem {
        id: rel_id,
        from_space_id: None,
        from_version_id: None,
        to_space_id: None,
        to_version_id: None,
        position: Some(true), // request to unset
        space_id,
        verified: Some(true), // request to unset
        context_root_id: None,
        context_edge_type_id: None,
    };

    let mut tx = pool.begin().await.unwrap();
    storage
        .unset_relation_fields(&[unset], &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Assert: system relation fields should NOT be nullified
    let (position, verified) = get_relation_fields(&pool, rel_id).await.unwrap();
    assert_eq!(
        position.as_deref(),
        Some("original_position"),
        "system relation position should not be unset"
    );
    assert_eq!(
        verified,
        Some(true),
        "system relation verified should not be unset"
    );

    cleanup_relation(&pool, rel_id).await;
}

// --------------------------------------------------------------------------
// Test: delete_relations should NOT delete a system relation
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_delete_relations_does_not_delete_system_relation() {
    let pool = get_pool().await;
    let storage = setup_storage().await;

    let rel_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();

    for id in [entity_id, from_id, to_id, type_id] {
        ensure_entity(&pool, id).await;
    }

    insert_system_relation(
        &pool, rel_id, space_id, entity_id, from_id, to_id, type_id, true,
    )
    .await;

    // Attempt to delete via Storage
    let mut tx = pool.begin().await.unwrap();
    storage
        .delete_relations(&[(rel_id, space_id)], &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Assert: system relation should still exist
    assert!(
        relation_exists(&pool, rel_id).await,
        "system relation should not be deleted"
    );

    cleanup_relation(&pool, rel_id).await;
}
