//! Integration tests for `Storage::delete_entities` / `close_entity_versions`: a `DeleteEntity`
//! op removes the entity's values and outgoing relations from the edit's space and nothing else.
//!
//! Prerequisites: PostgreSQL with the api migrations applied and DATABASE_URL set.
//!
//! Run with: cargo test --package kg-indexer --test delete_entity_integration -- --ignored

use kg_indexer::models::entities::EntityItem;
use kg_indexer::models::relations::SetRelationItem;
use kg_indexer::models::values::{ValueChangeType, ValueOp};
use kg_indexer::storage::Storage;
use sqlx::Row;
use uuid::Uuid;

fn text_value(entity: Uuid, property: Uuid, space: Uuid, text: &str) -> ValueOp {
    ValueOp {
        id: kg_indexer::handlers::edits::derive_value_id(&entity, &property, &space),
        change_type: ValueChangeType::Set,
        entity_id: entity,
        property_id: property,
        space_id: space,
        language: None,
        unit: None,
        text: Some(text.to_string()),
        decimal: None,
        boolean: None,
        time: None,
        point: None,
        rect: None,
        integer: None,
        float: None,
        bytes: None,
        date: None,
        datetime: None,
        schedule: None,
        embedding: None,
        time_utc: None,
        datetime_utc: None,
        context_root_id: None,
        context_edge_type_id: None,
        context_last_to_entity_id: None,
    }
}

fn relation(
    id: Uuid,
    space: Uuid,
    from: Uuid,
    to: Uuid,
    type_id: Uuid,
    system: bool,
) -> SetRelationItem {
    SetRelationItem {
        id,
        entity_id: Uuid::new_v4(),
        type_id,
        from_id: from,
        from_space_id: None,
        from_version_id: None,
        to_id: to,
        to_space_id: None,
        to_version_id: None,
        position: None,
        space_id: space,
        verified: None,
        is_system: system,
        context_root_id: None,
        context_edge_type_id: None,
        context_last_to_entity_id: None,
    }
}

async fn values_of(pool: &sqlx::PgPool, entity: Uuid, space: Option<Uuid>) -> i64 {
    let row = sqlx::query(
        "SELECT COUNT(*) FROM values WHERE entity_id = $1 AND ($2::uuid IS NULL OR space_id = $2)",
    )
    .bind(entity)
    .bind(space)
    .fetch_one(pool)
    .await
    .unwrap();
    row.get::<i64, _>(0)
}

async fn relations_named(pool: &sqlx::PgPool, id: Uuid) -> i64 {
    let row = sqlx::query("SELECT COUNT(*) FROM relations WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    row.get::<i64, _>(0)
}

#[tokio::test]
#[ignore]
async fn delete_entities_removes_values_and_outgoing_relations_in_that_space_only() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let storage = Storage::new(&url).await.expect("storage");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");

    let space = Uuid::new_v4();
    let other_space = Uuid::new_v4();
    let (dead, alive, prop, rel_type) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let (r_out, r_in, r_sys, r_other_space) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );

    let mut tx = storage.begin_with_timeout().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();
    let entities: Vec<EntityItem> = [dead, alive, prop, rel_type]
        .into_iter()
        .map(|id| EntityItem {
            id,
            created_at: "1".into(),
            created_at_block: "1".into(),
            updated_at: "1".into(),
            updated_at_block: "1".into(),
        })
        .collect();
    storage.insert_entities(&entities, &mut tx).await.unwrap();
    storage
        .insert_values(
            &[
                text_value(dead, prop, space, "dead here"),
                text_value(dead, prop, other_space, "dead elsewhere"),
                text_value(alive, prop, space, "alive"),
            ],
            &mut tx,
        )
        .await
        .unwrap();
    storage
        .insert_relations(
            &[
                relation(r_out, space, dead, alive, rel_type, false),
                relation(r_in, space, alive, dead, rel_type, false),
                relation(r_sys, space, dead, alive, rel_type, true),
                relation(r_other_space, other_space, dead, alive, rel_type, false),
            ],
            &mut tx,
        )
        .await
        .unwrap();
    let versioned = [
        text_value(dead, prop, space, "v1"),
        text_value(alive, prop, space, "v1"),
    ];
    storage
        .insert_value_versions(&versioned, 100, &mut tx)
        .await
        .unwrap();
    storage
        .delete_entities(&[dead], space, &mut tx)
        .await
        .unwrap();
    storage
        .close_entity_versions(&[dead], space, 101, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(values_of(&pool, dead, Some(space)).await, 0);
    assert_eq!(values_of(&pool, dead, Some(other_space)).await, 1);
    assert_eq!(values_of(&pool, alive, None).await, 1);

    assert_eq!(
        relations_named(&pool, r_out).await,
        0,
        "outgoing relation goes with the entity"
    );
    assert_eq!(
        relations_named(&pool, r_in).await,
        1,
        "a relation pointing at it is not its own"
    );
    assert_eq!(
        relations_named(&pool, r_sys).await,
        1,
        "system relations are protected"
    );
    assert_eq!(
        relations_named(&pool, r_other_space).await,
        1,
        "other spaces are untouched"
    );

    let open_versions = |entity: Uuid| {
        let pool = pool.clone();
        async move {
            let row = sqlx::query(
                "SELECT COUNT(*) FROM value_versions WHERE entity_id = $1 AND valid_to_key IS NULL",
            )
            .bind(entity)
            .fetch_one(&pool)
            .await
            .unwrap();
            row.get::<i64, _>(0)
        }
    };
    assert_eq!(
        open_versions(dead).await,
        0,
        "history of the deleted entity ends"
    );
    assert_eq!(open_versions(alive).await, 1);

    // Deleting again, or an unknown entity, is a no-op.
    let mut tx = storage.begin_with_timeout().await.unwrap();
    storage
        .delete_entities(&[dead, Uuid::new_v4()], space, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
