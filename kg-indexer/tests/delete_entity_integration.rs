//! Integration tests for the `DeleteEntity` handler: an edit's `DeleteEntity` op removes the
//! entity's values and outgoing relations from the edit's space, archives what it removed, and is
//! reversible. Every test drives the real write path (`handle_edit_for` + `Storage::write_edit`,
//! the same functions the Kafka consumer and `replay_edit` call) against Postgres with every api
//! migration applied, using its own space and entity ids so tests do not see each other.
//!
//! Prerequisites: PostgreSQL with all api migrations applied (including 0129) and DATABASE_URL set.
//!
//! Run with: cargo test --package kg-indexer --test delete_entity_integration -- --ignored

use std::sync::atomic::{AtomicI64, Ordering};

use grc_20::model::builder::EditBuilder;
use grc_20::Edit;
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use kg_indexer::handlers::edits::handle_edit_for;
use kg_indexer::models::entities::EntityItem;
use kg_indexer::models::relations::SetRelationItem;
use kg_indexer::models::values::{ValueChangeType, ValueOp};
use kg_indexer::storage::{EditWriteOutcome, Storage};
use serde_json::Value as Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

static NEXT_BLOCK: AtomicI64 = AtomicI64::new(0);

fn next_block() -> i64 {
    // A random base keeps reruns against the same database clear of the
    // (block_number, sequence) unique constraint on edit_versions.
    let base = NEXT_BLOCK.load(Ordering::SeqCst);
    if base == 0 {
        let seed = (Uuid::new_v4().as_u128() % 1_000_000_000_000) as i64 + 1_000_000;
        let _ = NEXT_BLOCK.compare_exchange(0, seed, Ordering::SeqCst, Ordering::SeqCst);
    }
    NEXT_BLOCK.fetch_add(1, Ordering::SeqCst)
}

fn id() -> [u8; 16] {
    Uuid::new_v4().into_bytes()
}

fn u(b: [u8; 16]) -> Uuid {
    Uuid::from_bytes(b)
}

struct Ctx {
    storage: Storage,
    pool: PgPool,
    space: Uuid,
    prop: [u8; 16],
    prop2: [u8; 16],
    rel_type: [u8; 16],
}

async fn ctx(apply: bool) -> Ctx {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let storage = Storage::new(&url)
        .await
        .expect("storage")
        .with_apply_delete_entity(apply);
    let pool = PgPool::connect(&url).await.expect("pool");
    Ctx {
        storage,
        pool,
        space: Uuid::new_v4(),
        prop: id(),
        prop2: id(),
        rel_type: id(),
    }
}

fn hedit(space: Uuid, edit: &Edit<'_>, block: i64) -> HermesEdit {
    HermesEdit {
        id: edit.id.to_vec(),
        name: String::new(),
        payload: grc_20::encode_edit(edit).expect("encode"),
        authors: vec![],
        language: None,
        space_id: space.as_bytes().to_vec(),
        is_canonical: true,
        meta: Some(BlockchainMetadata {
            created_at: 1_700_000_000 + block as u64 % 1000,
            created_by: vec![0; 20],
            block_number: block as u64,
            cursor: String::new(),
            sequence: 0,
            is_last: true,
        }),
    }
}

/// Run one edit through the production write path in a transaction, at a fresh increasing block.
async fn apply(c: &Ctx, space: Uuid, edit: Edit<'_>) -> (EditWriteOutcome, i64) {
    let block = next_block();
    let h = hedit(space, &edit, block);
    let result = handle_edit_for(&h, c.storage.apply_delete_entity()).expect("handle_edit");
    let mut tx = c.storage.begin_with_timeout().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();
    let outcome = c
        .storage
        .write_edit(&h, result, None, None, &mut tx)
        .await
        .expect("write_edit");
    tx.commit().await.unwrap();
    (outcome, block)
}

fn builder() -> EditBuilder<'static> {
    EditBuilder::new(id())
}

async fn count(c: &Ctx, sql: &'static str, a: Uuid, b: Uuid) -> i64 {
    sqlx::query(sql)
        .bind(a)
        .bind(b)
        .fetch_one(&c.pool)
        .await
        .unwrap()
        .get::<i64, _>(0)
}

async fn values_of(c: &Ctx, entity: [u8; 16], space: Uuid) -> i64 {
    count(
        c,
        "SELECT COUNT(*) FROM values WHERE entity_id = $1 AND space_id = $2",
        u(entity),
        space,
    )
    .await
}

async fn rels_from(c: &Ctx, entity: [u8; 16], space: Uuid) -> i64 {
    count(
        c,
        "SELECT COUNT(*) FROM relations WHERE from_entity_id = $1 AND space_id = $2",
        u(entity),
        space,
    )
    .await
}

async fn rels_to(c: &Ctx, entity: [u8; 16], space: Uuid) -> i64 {
    count(
        c,
        "SELECT COUNT(*) FROM relations WHERE to_entity_id = $1 AND space_id = $2",
        u(entity),
        space,
    )
    .await
}

async fn open_value_versions(c: &Ctx, entity: [u8; 16], space: Uuid) -> i64 {
    count(
        c,
        "SELECT COUNT(*) FROM value_versions WHERE entity_id = $1 AND space_id = $2 AND valid_to_key IS NULL",
        u(entity),
        space,
    )
    .await
}

async fn text_of(c: &Ctx, entity: [u8; 16], prop: [u8; 16], space: Uuid) -> Option<String> {
    sqlx::query_scalar(
        "SELECT text FROM values WHERE entity_id = $1 AND property_id = $2 AND space_id = $3",
    )
    .bind(u(entity))
    .bind(u(prop))
    .bind(space)
    .fetch_optional(&c.pool)
    .await
    .unwrap()
    .flatten()
}

async fn archived(c: &Ctx, edit: [u8; 16], source: &'static str) -> i64 {
    sqlx::query(
        "SELECT COUNT(*) FROM kg_archive.deleted_entity_rows WHERE edit_id = $1 AND source = $2",
    )
    .bind(u(edit))
    .bind(source)
    .fetch_one(&c.pool)
    .await
    .unwrap()
    .get::<i64, _>(0)
}

/// Every row the entity owns in the space, as comparable JSON: live values and relations plus both
/// version tables.
async fn snapshot(c: &Ctx, entity: [u8; 16], space: Uuid) -> Json {
    let mut out = serde_json::Map::new();
    let queries: [(&str, &'static str); 4] = [
        (
            "values",
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.id), '[]'::jsonb) FROM values t WHERE t.entity_id = $1 AND t.space_id = $2",
        ),
        (
            "relations",
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.id), '[]'::jsonb) FROM relations t WHERE t.from_entity_id = $1 AND t.space_id = $2",
        ),
        (
            "value_versions",
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.id), '[]'::jsonb) FROM value_versions t WHERE t.entity_id = $1 AND t.space_id = $2",
        ),
        (
            "relation_versions",
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.id), '[]'::jsonb) FROM relation_versions t WHERE t.from_entity_id = $1 AND t.space_id = $2",
        ),
    ];
    for (name, sql) in queries {
        let v: Json = sqlx::query_scalar(sql)
            .bind(u(entity))
            .bind(space)
            .fetch_one(&c.pool)
            .await
            .unwrap();
        out.insert(name.to_string(), v);
    }
    Json::Object(out)
}

async fn restore(c: &Ctx, edit: [u8; 16]) -> Vec<(String, i64, i64)> {
    sqlx::query("SELECT source, restored, skipped FROM kg_archive.restore_deleted_entities($1)")
        .bind(u(edit))
        .fetch_all(&c.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect()
}

/// An entity with two values and one outgoing and one incoming relation, created in `space`.
fn seed_edit(c: &Ctx, e: [u8; 16], other: [u8; 16]) -> Edit<'static> {
    builder()
        .create_entity(e, |b| {
            b.text(c.prop, "alpha", None).text(c.prop2, "beta", None)
        })
        .create_entity(other, |b| b.text(c.prop, "other", None))
        .create_relation(|r| r.id(id()).relation_type(c.rel_type).from(e).to(other))
        .create_relation(|r| r.id(id()).relation_type(c.rel_type).from(other).to(e))
        .build()
}

// ---------------------------------------------------------------------------------------------
// Direct-write helpers for rows a public edit cannot produce (is_system relations, protected
// properties).
// ---------------------------------------------------------------------------------------------

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

fn relation_item(
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

async fn insert_direct(c: &Ctx, values: &[ValueOp], rels: &[SetRelationItem], ents: &[Uuid]) {
    let mut tx = c.storage.begin_with_timeout().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();
    let items: Vec<EntityItem> = ents
        .iter()
        .map(|&id| EntityItem {
            id,
            created_at: "1".into(),
            created_at_block: "1".into(),
            updated_at: "1".into(),
            updated_at_block: "1".into(),
        })
        .collect();
    c.storage.insert_entities(&items, &mut tx).await.unwrap();
    c.storage.insert_values(values, &mut tx).await.unwrap();
    c.storage.insert_relations(rels, &mut tx).await.unwrap();
    tx.commit().await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn delete_removes_values_and_outgoing_relations_and_closes_versions() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    assert_eq!(values_of(&c, e, c.space).await, 2);
    assert_eq!(rels_from(&c, e, c.space).await, 1);
    assert_eq!(open_value_versions(&c, e, c.space).await, 2);

    let del = builder().delete_entity(e).build();
    let del_id = del.id;
    let (out, _) = apply(&c, c.space, del).await;

    assert_eq!(out.entities_deleted, 1);
    assert!(out.deletes.applied);
    assert_eq!(out.deletes.values, 2);
    assert_eq!(out.deletes.relations, 1);
    assert_eq!(out.deletes.value_versions, 2);
    assert_eq!(out.deletes.relation_versions, 1);
    assert_eq!(values_of(&c, e, c.space).await, 0);
    assert_eq!(rels_from(&c, e, c.space).await, 0);
    assert_eq!(open_value_versions(&c, e, c.space).await, 0);
    assert_eq!(archived(&c, del_id, "values").await, 2);
    assert_eq!(archived(&c, del_id, "relations").await, 1);
    assert_eq!(archived(&c, del_id, "value_versions").await, 2);
    assert_eq!(archived(&c, del_id, "relation_versions").await, 1);
    // The other entity is untouched.
    assert_eq!(values_of(&c, other, c.space).await, 1);
    assert_eq!(open_value_versions(&c, other, c.space).await, 1);
    // The global entities row stays.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entities WHERE id = $1")
        .bind(u(e))
        .fetch_one(&c.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
#[ignore]
async fn archive_rows_record_edit_block_space_and_time() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    let del = builder().delete_entity(e).build();
    let del_id = del.id;
    let (_, block) = apply(&c, c.space, del).await;

    let rows = sqlx::query(
        "SELECT block_number, space_id, entity_id, edit_created_at, deleted_at IS NOT NULL AS has_time, restored_at IS NULL AS open \
         FROM kg_archive.deleted_entity_rows WHERE edit_id = $1",
    )
    .bind(u(del_id))
    .fetch_all(&c.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 6, "2 values + 1 relation + 2 + 1 versions");
    for r in rows {
        assert_eq!(r.get::<i64, _>("block_number"), block);
        assert_eq!(r.get::<Uuid, _>("space_id"), c.space);
        assert_eq!(r.get::<Uuid, _>("entity_id"), u(e));
        assert!(r.get::<i64, _>("edit_created_at") >= 1_700_000_000);
        assert!(r.get::<bool, _>("has_time"));
        assert!(r.get::<bool, _>("open"));
    }
}

#[tokio::test]
#[ignore]
async fn delete_then_restore_returns_identical_rows() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    // Every column type, so the jsonb round trip is proven for each.
    let seed = builder()
        .create_entity(e, |b| {
            b.text(c.prop, "txt", None)
                .integer(c.prop2, 42, None)
                .float(id(), 1.5, None)
                .boolean(id(), true)
                .date(id(), "2024-01-02")
                .bytes(id(), vec![1u8, 2, 3])
        })
        .create_entity(other, |b| b.text(c.prop, "o", None))
        .create_relation(|r| r.id(id()).relation_type(c.rel_type).from(e).to(other))
        .build();
    apply(&c, c.space, seed).await;
    // A second edit so the entity has closed and open versions, not only one open row each.
    apply(
        &c,
        c.space,
        builder()
            .update_entity(e, |b| {
                b.set(
                    c.prop,
                    grc_20::model::Value::Text {
                        value: "txt2".into(),
                        language: None,
                    },
                )
            })
            .build(),
    )
    .await;

    let before = snapshot(&c, e, c.space).await;
    assert_eq!(before["values"].as_array().unwrap().len(), 6);

    let del = builder().delete_entity(e).build();
    let del_id = del.id;
    apply(&c, c.space, del).await;
    let gone = snapshot(&c, e, c.space).await;
    assert_ne!(before, gone);
    assert_eq!(gone["values"].as_array().unwrap().len(), 0);

    let report = restore(&c, del_id).await;
    assert!(
        report.iter().all(|(_, _, skipped)| *skipped == 0),
        "{report:?}"
    );
    let after = snapshot(&c, e, c.space).await;
    assert_eq!(before, after, "restore must give back identical rows");

    // Restoring again is a no-op.
    let again = restore(&c, del_id).await;
    assert!(
        again.iter().all(|(_, restored, _)| *restored == 0),
        "{again:?}"
    );
    assert_eq!(snapshot(&c, e, c.space).await, before);
}

#[tokio::test]
#[ignore]
async fn restore_never_overwrites_newer_rows() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    let del = builder().delete_entity(e).build();
    let del_id = del.id;
    apply(&c, c.space, del).await;
    // The entity comes back with a new value for prop before anyone restores.
    apply(
        &c,
        c.space,
        builder()
            .create_entity(e, |b| b.text(c.prop, "newer", None))
            .build(),
    )
    .await;
    let report = restore(&c, del_id).await;
    let values = report.iter().find(|r| r.0 == "values").unwrap();
    assert_eq!(
        (values.1, values.2),
        (1, 1),
        "prop2 comes back, prop is skipped"
    );
    assert_eq!(
        text_of(&c, e, c.prop, c.space).await.as_deref(),
        Some("newer")
    );
    assert_eq!(
        text_of(&c, e, c.prop2, c.space).await.as_deref(),
        Some("beta")
    );
}

#[tokio::test]
#[ignore]
async fn kill_switch_off_writes_nothing_and_reports_what_would_go() {
    let on = ctx(true).await;
    let off = ctx(false).await;
    let (e, other) = (id(), id());
    apply(&off, off.space, seed_edit(&off, e, other)).await;
    let before = snapshot(&off, e, off.space).await;

    let del = builder().delete_entity(e).build();
    let del_id = del.id;
    let (out, _) = apply(&off, off.space, del).await;

    assert!(!out.deletes.applied);
    assert_eq!(out.entities_deleted, 1);
    assert_eq!(
        (
            out.deletes.values,
            out.deletes.relations,
            out.deletes.value_versions,
            out.deletes.relation_versions
        ),
        (2, 1, 2, 1),
        "would-delete counts equal what applying removes"
    );
    assert_eq!(
        snapshot(&off, e, off.space).await,
        before,
        "nothing written"
    );
    assert_eq!(
        archived(&off, del_id, "values").await,
        0,
        "nothing archived"
    );

    // Same state, flag on: the real delete removes exactly the counted rows.
    let e2 = id();
    apply(&on, on.space, seed_edit(&on, e2, id())).await;
    let (out_on, _) = apply(&on, on.space, builder().delete_entity(e2).build()).await;
    assert!(out_on.deletes.applied);
    assert_eq!(out_on.deletes.values, out.deletes.values);
    assert_eq!(out_on.deletes.relations, out.deletes.relations);
    assert_eq!(out_on.deletes.value_versions, out.deletes.value_versions);
    assert_eq!(
        out_on.deletes.relation_versions,
        out.deletes.relation_versions
    );
}

#[tokio::test]
#[ignore]
async fn kill_switch_off_keeps_writes_that_precede_the_delete_in_the_same_edit() {
    // With the flag off the handler must be exactly the old one: nothing pruned, nothing deleted.
    let c = ctx(false).await;
    let e = id();
    let edit = builder()
        .create_entity(e, |b| b.text(c.prop, "kept", None))
        .delete_entity(e)
        .build();
    apply(&c, c.space, edit).await;
    assert_eq!(
        text_of(&c, e, c.prop, c.space).await.as_deref(),
        Some("kept")
    );
}

#[tokio::test]
#[ignore]
async fn delete_then_recreate_in_the_same_edit_leaves_the_new_entity() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    let edit = builder()
        .delete_entity(e)
        .update_entity(e, |b| {
            b.set(
                c.prop,
                grc_20::model::Value::Text {
                    value: "reborn".into(),
                    language: None,
                },
            )
        })
        .build();
    apply(&c, c.space, edit).await;
    assert_eq!(
        values_of(&c, e, c.space).await,
        1,
        "old prop2 gone, only the new value"
    );
    assert_eq!(
        text_of(&c, e, c.prop, c.space).await.as_deref(),
        Some("reborn")
    );
    assert_eq!(text_of(&c, e, c.prop2, c.space).await, None);
    assert_eq!(rels_from(&c, e, c.space).await, 0);
    assert_eq!(open_value_versions(&c, e, c.space).await, 1);
}

#[tokio::test]
#[ignore]
async fn delete_then_recreate_in_a_later_edit_comes_back_cleanly() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    apply(&c, c.space, builder().delete_entity(e).build()).await;
    assert_eq!(values_of(&c, e, c.space).await, 0);
    apply(
        &c,
        c.space,
        builder()
            .create_entity(e, |b| b.text(c.prop, "back", None))
            .create_relation(|r| r.id(id()).relation_type(c.rel_type).from(e).to(other))
            .build(),
    )
    .await;
    assert_eq!(values_of(&c, e, c.space).await, 1);
    assert_eq!(
        text_of(&c, e, c.prop, c.space).await.as_deref(),
        Some("back")
    );
    assert_eq!(rels_from(&c, e, c.space).await, 1);
    assert_eq!(open_value_versions(&c, e, c.space).await, 1);
}

#[tokio::test]
#[ignore]
async fn delete_touches_only_the_deleting_space() {
    let c = ctx(true).await;
    let other_space = Uuid::new_v4();
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    apply(&c, other_space, seed_edit(&c, e, other)).await;
    let elsewhere = snapshot(&c, e, other_space).await;

    apply(&c, c.space, builder().delete_entity(e).build()).await;

    assert_eq!(values_of(&c, e, c.space).await, 0);
    assert_eq!(rels_from(&c, e, c.space).await, 0);
    assert_eq!(snapshot(&c, e, other_space).await, elsewhere);
    assert_eq!(values_of(&c, e, other_space).await, 2);
    assert_eq!(open_value_versions(&c, e, other_space).await, 2);
}

#[tokio::test]
#[ignore]
async fn protected_properties_are_kept() {
    let c = ctx(true).await;
    let e = u(id());
    let protected = Uuid::parse_str(sdk::core::ids::PROTECTED_PROPERTY_IDS[0]).unwrap();
    let plain = u(c.prop);
    insert_direct(
        &c,
        &[
            text_value(e, protected, c.space, "keep me"),
            text_value(e, plain, c.space, "drop me"),
        ],
        &[],
        &[e],
    )
    .await;
    let (out, _) = apply(&c, c.space, builder().delete_entity(e.into_bytes()).build()).await;
    assert_eq!(out.deletes.values, 1);
    let left: Vec<Uuid> =
        sqlx::query_scalar("SELECT property_id FROM values WHERE entity_id = $1 AND space_id = $2")
            .bind(e)
            .bind(c.space)
            .fetch_all(&c.pool)
            .await
            .unwrap();
    assert_eq!(left, vec![protected]);
}

#[tokio::test]
#[ignore]
async fn is_system_relations_are_kept() {
    let c = ctx(true).await;
    let e = u(id());
    let to = u(id());
    let (sys, user) = (Uuid::new_v4(), Uuid::new_v4());
    let t = u(c.rel_type);
    insert_direct(
        &c,
        &[],
        &[
            relation_item(sys, c.space, e, to, t, true),
            relation_item(user, c.space, e, to, t, false),
        ],
        &[e, to],
    )
    .await;
    let (out, _) = apply(&c, c.space, builder().delete_entity(e.into_bytes()).build()).await;
    assert_eq!(out.deletes.relations, 1);
    let left: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM relations WHERE from_entity_id = $1 AND space_id = $2")
            .bind(e)
            .bind(c.space)
            .fetch_all(&c.pool)
            .await
            .unwrap();
    assert_eq!(left, vec![sys]);
}

#[tokio::test]
#[ignore]
async fn incoming_relations_are_kept() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    assert_eq!(rels_to(&c, e, c.space).await, 1);
    apply(&c, c.space, builder().delete_entity(e).build()).await;
    assert_eq!(
        rels_to(&c, e, c.space).await,
        1,
        "other -> e is other's relation"
    );
    assert_eq!(rels_from(&c, other, c.space).await, 1);
    assert_eq!(rels_from(&c, e, c.space).await, 0);
}

#[tokio::test]
#[ignore]
async fn deleting_twice_is_a_no_op() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    apply(&c, c.space, builder().delete_entity(e).build()).await;
    let after_first = snapshot(&c, e, c.space).await;
    let second = builder().delete_entity(e).build();
    let second_id = second.id;
    let (out, _) = apply(&c, c.space, second).await;
    assert_eq!(out.deletes.values + out.deletes.relations, 0);
    assert_eq!(
        out.deletes.value_versions + out.deletes.relation_versions,
        0
    );
    assert_eq!(snapshot(&c, e, c.space).await, after_first);
    assert_eq!(archived(&c, second_id, "values").await, 0);
}

#[tokio::test]
#[ignore]
async fn deleting_an_entity_that_does_not_exist_is_a_no_op() {
    let c = ctx(true).await;
    let ghost = id();
    let del = builder().delete_entity(ghost).build();
    let del_id = del.id;
    let (out, _) = apply(&c, c.space, del).await;
    assert_eq!(out.entities_deleted, 1);
    assert_eq!(
        (
            out.deletes.values,
            out.deletes.relations,
            out.deletes.value_versions,
            out.deletes.relation_versions
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(archived(&c, del_id, "values").await, 0);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entities WHERE id = $1")
        .bind(u(ghost))
        .fetch_one(&c.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "a delete never creates an entity row");
}

#[tokio::test]
#[ignore]
async fn ops_within_an_edit_apply_in_order() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    // set, delete, set again, delete a different entity, create a relation from the first.
    let rel = id();
    let edit = builder()
        .update_entity(e, |b| {
            b.set(
                c.prop,
                grc_20::model::Value::Text {
                    value: "before-delete".into(),
                    language: None,
                },
            )
        })
        .delete_entity(e)
        .update_entity(e, |b| {
            b.set(
                c.prop,
                grc_20::model::Value::Text {
                    value: "after-delete".into(),
                    language: None,
                },
            )
        })
        .create_relation(|r| r.id(rel).relation_type(c.rel_type).from(e).to(other))
        .delete_entity(other)
        .build();
    apply(&c, c.space, edit).await;
    assert_eq!(
        text_of(&c, e, c.prop, c.space).await.as_deref(),
        Some("after-delete")
    );
    assert_eq!(
        text_of(&c, e, c.prop2, c.space).await,
        None,
        "pre-existing prop2 deleted"
    );
    assert_eq!(
        rels_from(&c, e, c.space).await,
        1,
        "relation created after the delete survives"
    );
    assert_eq!(values_of(&c, other, c.space).await, 0);
    assert_eq!(rels_from(&c, other, c.space).await, 0);
}

#[tokio::test]
#[ignore]
async fn versions_close_at_the_deleting_edits_version_key() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    let (_, seed_block) = apply(&c, c.space, seed_edit(&c, e, other)).await;
    let (_, del_block) = apply(&c, c.space, builder().delete_entity(e).build()).await;
    let want = del_block << 32;
    let rows = sqlx::query(
        "SELECT valid_from_key, valid_to_key FROM value_versions WHERE entity_id = $1 AND space_id = $2",
    )
    .bind(u(e))
    .bind(c.space)
    .fetch_all(&c.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for r in rows {
        assert_eq!(r.get::<i64, _>(0), seed_block << 32);
        assert_eq!(r.get::<Option<i64>, _>(1), Some(want));
    }
    let rv: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT valid_to_key FROM relation_versions WHERE from_entity_id = $1 AND space_id = $2",
    )
    .bind(u(e))
    .bind(c.space)
    .fetch_all(&c.pool)
    .await
    .unwrap();
    assert_eq!(rv, vec![Some(want)]);
    // The other entity's version, and its relation to e, stay open.
    assert_eq!(open_value_versions(&c, other, c.space).await, 1);
    let open_rv: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM relation_versions WHERE from_entity_id = $1 AND space_id = $2 AND valid_to_key IS NULL",
    )
    .bind(u(other))
    .bind(c.space)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(open_rv, 1);
}

#[tokio::test]
#[ignore]
async fn replaying_the_same_edit_is_idempotent() {
    let c = ctx(true).await;
    let (e, other) = (id(), id());
    apply(&c, c.space, seed_edit(&c, e, other)).await;
    let del = builder().delete_entity(e).build();
    let h = hedit(c.space, &del, next_block());
    for _ in 0..2 {
        let result = handle_edit_for(&h, true).unwrap();
        let mut tx = c.storage.begin_with_timeout().await.unwrap();
        c.storage
            .write_edit(&h, result, None, None, &mut tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(
        archived(&c, del.id, "values").await,
        2,
        "archived once, not twice"
    );
    assert_eq!(values_of(&c, e, c.space).await, 0);
}

/// `replay_edit` shares `Storage::write_edit` with the consumer; this runs the real binary and
/// checks it leaves the same rows and the same archive a live apply does.
#[tokio::test]
#[ignore]
async fn replay_edit_binary_behaves_like_the_live_path() {
    let live = ctx(true).await;
    let replay_space = Uuid::new_v4();
    // Same shape in both spaces: seed through the live path (so the seed edit is not under test).
    let (e, other) = (id(), id());
    apply(&live, live.space, seed_edit(&live, e, other)).await;
    apply(&live, replay_space, seed_edit(&live, e, other)).await;

    let del = builder().delete_entity(e).build();
    let live_del = del.id;
    apply(&live, live.space, del).await;

    // Replay: the same delete into the other space via the binary, from an ipfs_cache row.
    let del2 = builder().delete_entity(e).build();
    let replay_del = del2.id;
    let block = next_block();
    let payload = grc_20::encode_edit(&del2).unwrap();
    let uri = format!("ipfs://test-{}", Uuid::new_v4());
    sqlx::query("INSERT INTO ipfs_cache (uri, data, is_errored, block, space) VALUES ($1, $2, false, $3, $4)")
        .bind(&uri)
        .bind(&payload)
        .bind(block.to_string())
        .bind(replay_space)
        .execute(&live.pool)
        .await
        .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_replay_edit"))
        .env("DATABASE_URL", std::env::var("DATABASE_URL").unwrap())
        .env("KG_INDEXER_APPLY_DELETE_ENTITY", "1")
        .args([
            "--uri",
            &uri,
            "--space",
            &replay_space.to_string(),
            "--block",
            &block.to_string(),
        ])
        .output()
        .expect("run replay_edit");
    assert!(
        out.status.success(),
        "replay_edit failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Same removals ...
    for source in ["values", "relations", "value_versions", "relation_versions"] {
        assert_eq!(
            archived(&live, live_del, source).await,
            archived(&live, replay_del, source).await,
            "{source}"
        );
    }
    assert_eq!(values_of(&live, e, replay_space).await, 0);
    assert_eq!(rels_from(&live, e, replay_space).await, 0);
    assert_eq!(open_value_versions(&live, e, replay_space).await, 0);
    // ... and what is left is the same: other's rows and incoming relation survive in both.
    assert_eq!(
        values_of(&live, other, replay_space).await,
        values_of(&live, other, live.space).await
    );
    assert_eq!(
        rels_to(&live, e, replay_space).await,
        rels_to(&live, e, live.space).await
    );

    // With the flag off the binary writes nothing.
    let e3 = id();
    apply(&live, replay_space, seed_edit(&live, e3, id())).await;
    let del3 = builder().delete_entity(e3).build();
    let block3 = next_block();
    let uri3 = format!("ipfs://test-{}", Uuid::new_v4());
    sqlx::query("INSERT INTO ipfs_cache (uri, data, is_errored, block, space) VALUES ($1, $2, false, $3, $4)")
        .bind(&uri3)
        .bind(grc_20::encode_edit(&del3).unwrap())
        .bind(block3.to_string())
        .bind(replay_space)
        .execute(&live.pool)
        .await
        .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_replay_edit"))
        .env("DATABASE_URL", std::env::var("DATABASE_URL").unwrap())
        .env_remove("KG_INDEXER_APPLY_DELETE_ENTITY")
        .args([
            "--uri",
            &uri3,
            "--space",
            &replay_space.to_string(),
            "--block",
            &block3.to_string(),
        ])
        .output()
        .expect("run replay_edit");
    assert!(out.status.success());
    assert_eq!(
        values_of(&live, e3, replay_space).await,
        2,
        "flag off: nothing removed"
    );
}
