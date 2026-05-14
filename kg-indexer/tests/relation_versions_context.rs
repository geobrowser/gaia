//! Integration tests for context propagation through `insert_relation_versions`
//! on Update / Unset / Create relation ops (RFC 0003 — context-aware diffs).
//!
//! These tests verify the storage-layer rewrite that materializes a new
//! `relation_versions` row for Update and Unset ops by reading the
//! post-mutation state from the live `relations` table and attaching the
//! op's `context_root_id` / `context_edge_type_id`. Without this path, an
//! update would close the existing version row and write nothing new, so
//! contextual updates were invisible to `queryContextEntities`.
//!
//! Prerequisites:
//! - PostgreSQL running with migrations applied
//! - DATABASE_URL environment variable set
//!
//! Run with:
//!   cargo test --package kg-indexer \
//!     --test relation_versions_context -- --ignored

use kg_indexer::models::relations::{
    RelationOp, SetRelationItem, UnsetRelationItem, UpdateRelationItem,
};
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

async fn cleanup(pool: &sqlx::Pool<sqlx::Postgres>, rel_id: Uuid) {
    sqlx::query("DELETE FROM relation_versions WHERE relation_id = $1")
        .bind(rel_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM relations WHERE id = $1")
        .bind(rel_id)
        .execute(pool)
        .await
        .ok();
}

#[derive(sqlx::FromRow)]
struct VersionRow {
    valid_from_key: i64,
    valid_to_key: Option<i64>,
    context_root_id: Option<Uuid>,
    context_edge_type_id: Option<Uuid>,
    position: Option<String>,
}

async fn fetch_versions(
    pool: &sqlx::Pool<sqlx::Postgres>,
    relation_id: Uuid,
) -> Vec<VersionRow> {
    sqlx::query_as::<_, VersionRow>(
        "SELECT valid_from_key, valid_to_key, context_root_id, context_edge_type_id, position
         FROM relation_versions
         WHERE relation_id = $1
         ORDER BY valid_from_key",
    )
    .bind(relation_id)
    .fetch_all(pool)
    .await
    .expect("Failed to query relation_versions")
}

// --------------------------------------------------------------------------
// Test: a contextual Update materializes a new relation_versions row that
// carries the op's context columns and the post-mutation state.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn update_with_context_inserts_new_version_row() {
    let pool = get_pool().await;
    let storage = setup_storage().await;

    let rel_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();
    let root_id = Uuid::new_v4();
    let edge_type_id = Uuid::new_v4();

    for id in [entity_id, from_id, to_id, type_id, root_id, edge_type_id] {
        ensure_entity(&pool, id).await;
    }

    cleanup(&pool, rel_id).await;

    let v1_key: i64 = (1_000_i64) << 32;
    let v2_key: i64 = (1_001_i64) << 32;

    // Seed: create relation at v1 with no context.
    let create = SetRelationItem {
        id: rel_id,
        entity_id,
        type_id,
        from_id,
        from_space_id: None,
        from_version_id: None,
        to_id,
        to_space_id: None,
        to_version_id: None,
        position: Some("a".to_string()),
        space_id,
        verified: None,
        is_system: false,
        context_root_id: None,
        context_edge_type_id: None,
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_relations(std::slice::from_ref(&create), &mut tx)
        .await
        .unwrap();
    storage
        .insert_relation_versions(&[RelationOp::Create(create)], v1_key, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Apply: contextual Update at v2 — moves position to "b".
    let update = UpdateRelationItem {
        id: rel_id,
        space_id,
        from_space_id: None,
        from_version_id: None,
        to_space_id: None,
        to_version_id: None,
        position: Some("b".to_string()),
        verified: None,
        context_root_id: Some(root_id),
        context_edge_type_id: Some(edge_type_id),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .update_relations(std::slice::from_ref(&update), &mut tx)
        .await
        .unwrap();
    storage
        .insert_relation_versions(&[RelationOp::Update(update)], v2_key, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let rows = fetch_versions(&pool, rel_id).await;
    assert_eq!(rows.len(), 2, "expected one v1 and one v2 row");

    let v1 = &rows[0];
    let v2 = &rows[1];

    // v1 closed by v2.
    assert_eq!(v1.valid_from_key, v1_key);
    assert_eq!(v1.valid_to_key, Some(v2_key));
    assert_eq!(v1.context_root_id, None);
    assert_eq!(v1.context_edge_type_id, None);

    // v2 carries the op's context AND the post-mutation state.
    assert_eq!(v2.valid_from_key, v2_key);
    assert_eq!(v2.valid_to_key, None);
    assert_eq!(v2.context_root_id, Some(root_id));
    assert_eq!(v2.context_edge_type_id, Some(edge_type_id));
    assert_eq!(v2.position.as_deref(), Some("b"));

    cleanup(&pool, rel_id).await;
}

// --------------------------------------------------------------------------
// Test: a contextual Unset materializes a new relation_versions row with
// the unset applied (position null) and the op's context columns.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn unset_with_context_inserts_new_version_row() {
    let pool = get_pool().await;
    let storage = setup_storage().await;

    let rel_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();
    let root_id = Uuid::new_v4();
    let edge_type_id = Uuid::new_v4();

    for id in [entity_id, from_id, to_id, type_id, root_id, edge_type_id] {
        ensure_entity(&pool, id).await;
    }

    cleanup(&pool, rel_id).await;

    let v1_key: i64 = (2_000_i64) << 32;
    let v2_key: i64 = (2_001_i64) << 32;

    // Seed: create at v1 with a position to unset later.
    let create = SetRelationItem {
        id: rel_id,
        entity_id,
        type_id,
        from_id,
        from_space_id: None,
        from_version_id: None,
        to_id,
        to_space_id: None,
        to_version_id: None,
        position: Some("a".to_string()),
        space_id,
        verified: None,
        is_system: false,
        context_root_id: None,
        context_edge_type_id: None,
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_relations(std::slice::from_ref(&create), &mut tx)
        .await
        .unwrap();
    storage
        .insert_relation_versions(&[RelationOp::Create(create)], v1_key, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Apply: contextual Unset of position at v2.
    let unset = UnsetRelationItem {
        id: rel_id,
        space_id,
        from_space_id: None,
        from_version_id: None,
        to_space_id: None,
        to_version_id: None,
        position: Some(true), // unset position
        verified: None,
        context_root_id: Some(root_id),
        context_edge_type_id: Some(edge_type_id),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .unset_relation_fields(std::slice::from_ref(&unset), &mut tx)
        .await
        .unwrap();
    storage
        .insert_relation_versions(&[RelationOp::Unset(unset)], v2_key, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let rows = fetch_versions(&pool, rel_id).await;
    assert_eq!(rows.len(), 2);

    let v2 = &rows[1];
    assert_eq!(v2.valid_from_key, v2_key);
    assert_eq!(v2.valid_to_key, None);
    assert_eq!(v2.context_root_id, Some(root_id));
    assert_eq!(v2.context_edge_type_id, Some(edge_type_id));
    // Unset cleared the position; the new version row reflects the post-unset state.
    assert_eq!(v2.position, None);

    cleanup(&pool, rel_id).await;
}

// --------------------------------------------------------------------------
// Regression test: a contextual Create still writes the version row with
// context. This is the simplest case but covers the original RFC path.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn create_with_context_writes_version_row() {
    let pool = get_pool().await;
    let storage = setup_storage().await;

    let rel_id = Uuid::new_v4();
    let space_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let type_id = Uuid::new_v4();
    let root_id = Uuid::new_v4();
    let edge_type_id = Uuid::new_v4();

    for id in [entity_id, from_id, to_id, type_id, root_id, edge_type_id] {
        ensure_entity(&pool, id).await;
    }

    cleanup(&pool, rel_id).await;

    let v1_key: i64 = (3_000_i64) << 32;

    let create = SetRelationItem {
        id: rel_id,
        entity_id,
        type_id,
        from_id,
        from_space_id: None,
        from_version_id: None,
        to_id,
        to_space_id: None,
        to_version_id: None,
        position: Some("a".to_string()),
        space_id,
        verified: None,
        is_system: false,
        context_root_id: Some(root_id),
        context_edge_type_id: Some(edge_type_id),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_relations(std::slice::from_ref(&create), &mut tx)
        .await
        .unwrap();
    storage
        .insert_relation_versions(&[RelationOp::Create(create)], v1_key, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let rows = fetch_versions(&pool, rel_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].context_root_id, Some(root_id));
    assert_eq!(rows[0].context_edge_type_id, Some(edge_type_id));
    assert_eq!(rows[0].valid_from_key, v1_key);
    assert_eq!(rows[0].valid_to_key, None);

    cleanup(&pool, rel_id).await;
}
