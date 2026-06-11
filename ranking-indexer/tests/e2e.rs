//! End-to-end integration test for the full ranking-indexer pipeline.
//!
//! Runs real edits through `detect -> apply_detected_edit` (upsert -> dedup ->
//! eligibility -> scoring -> publish) against a live Postgres and asserts the
//! public `RANK_POSITION` projection. Requires a DB with the public schema + the
//! private `ranks` schema. Opt-in: set `RANKING_INDEXER_E2E_DATABASE_URL` to the
//! connection string. Skips (passes) if unset, so it never runs on a generic
//! CI `DATABASE_URL` that lacks the `ranks` schema.

use grc_20::model::builder::EditBuilder;
use sdk::core::ids::*;
use sqlx::Row;
use uuid::Uuid;

use ranking_indexer::detect::detect;
use ranking_indexer::recompute::apply_detected_edit;
use ranking_indexer::storage::Storage;

fn gid(n: u128) -> [u8; 16] {
    *Uuid::from_u128(n).as_bytes()
}
fn sid(s: &str) -> [u8; 16] {
    *Uuid::parse_str(s).unwrap().as_bytes()
}
fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

// Scenario UUIDs (high values, unlikely to collide with real data).
const BLOCK_SPACE: u128 = 0xE2E5_0000_0001;
const MEMBER1: u128 = 0xE2E5_0000_0011;
const MEMBER2: u128 = 0xE2E5_0000_0012;
const NONMEMBER: u128 = 0xE2E5_0000_0013;
const BLOCK: u128 = 0xE2E5_0000_0021;
const ENTITY_A: u128 = 0xE2E5_0000_0031;
const ENTITY_B: u128 = 0xE2E5_0000_0032;
const RANK1: u128 = 0xE2E5_0000_0041;
const RANK2: u128 = 0xE2E5_0000_0042;
const RANK3: u128 = 0xE2E5_0000_0043;

#[tokio::test]
async fn end_to_end_dao_block_filters_nonmembers_and_publishes() {
    let Ok(url) = std::env::var("RANKING_INDEXER_E2E_DATABASE_URL") else {
        eprintln!("skipping e2e: RANKING_INDEXER_E2E_DATABASE_URL not set");
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
    ] {
        sqlx::query(sql)
            .bind(u(BLOCK_SPACE))
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM ranks.ranking_items WHERE ranking_id = ANY($1)")
        .bind(&[u(RANK1), u(RANK2), u(RANK3)][..])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.rankings WHERE id = ANY($1)")
        .bind(&[u(RANK1), u(RANK2), u(RANK3)][..])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.ranking_scores WHERE block_id = $1")
        .bind(u(BLOCK))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.ranking_blocks WHERE id = $1")
        .bind(u(BLOCK))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM members WHERE space_id = $1")
        .bind(u(BLOCK_SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(u(BLOCK_SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed public prerequisites -----------------------------------------
    // The block lives in a DAO space; member1/member2 are members, nonmember isn't.
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e')")
        .bind(u(BLOCK_SPACE))
        .execute(pool)
        .await
        .unwrap();
    for m in [MEMBER1, MEMBER2] {
        sqlx::query("INSERT INTO members (member_space_id, space_id) VALUES ($1, $2)")
            .bind(u(m))
            .bind(u(BLOCK_SPACE))
            .execute(pool)
            .await
            .unwrap();
    }

    // --- edit 1: create the ranking block ----------------------------------
    let block_edit = EditBuilder::new(gid(1))
        .create_relation(|r| {
            r.id(gid(0x100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_relation(|r| {
            r.id(gid(0x101))
                .relation_type(sid(RANK_AGGREGATION_RESTRICTION_PROPERTY_ID))
                .from(gid(BLOCK))
                .to(sid(RANK_RESTRICTION_MEMBERS_AND_EDITORS_ID))
        })
        .create_entity(gid(BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Top Films", None)
        })
        .build();
    apply_detected_edit(
        &detect(&block_edit, u(BLOCK_SPACE), 1, 0),
        u(BLOCK_SPACE),
        &storage,
    )
    .await
    .unwrap();

    // --- ordinal submission helper (rank -> [first, second]) ----------------
    // Two members rank A above B; the non-member ranks B above A.
    let submit = |rank: u128, rel_base: u128, first: u128, second: u128| {
        EditBuilder::new(gid(rank ^ 0xED17))
            .create_relation(|r| {
                r.id(gid(rel_base))
                    .relation_type(sid(TYPE_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(sid(RANK_TYPE_ID))
            })
            .create_entity(gid(rank), |e| {
                e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
            })
            .create_relation(|r| {
                r.id(gid(rel_base + 1))
                    .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(gid(BLOCK))
            })
            .create_relation(|r| {
                r.id(gid(rel_base + 2))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(gid(first))
                    .to_space(gid(BLOCK_SPACE))
                    .position("a0")
            })
            .create_relation(|r| {
                r.id(gid(rel_base + 3))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(gid(second))
                    .to_space(gid(BLOCK_SPACE))
                    .position("a1")
            })
            .build()
    };

    apply_detected_edit(
        &detect(&submit(RANK1, 0x200, ENTITY_A, ENTITY_B), u(MEMBER1), 2, 0),
        u(MEMBER1),
        &storage,
    )
    .await
    .unwrap();
    apply_detected_edit(
        &detect(&submit(RANK2, 0x210, ENTITY_A, ENTITY_B), u(MEMBER2), 3, 0),
        u(MEMBER2),
        &storage,
    )
    .await
    .unwrap();
    // non-member ranks B above A — must be filtered out
    apply_detected_edit(
        &detect(
            &submit(RANK3, 0x220, ENTITY_B, ENTITY_A),
            u(NONMEMBER),
            4,
            0,
        ),
        u(NONMEMBER),
        &storage,
    )
    .await
    .unwrap();

    // --- assert the published RANK_POSITION projection ----------------------
    let rank_position = u(Uuid::parse_str(RANK_POSITION_RELATION_TYPE_ID)
        .unwrap()
        .as_u128());
    let value_prop = Uuid::parse_str(RANK_POSITION_VALUE_PROPERTY_ID).unwrap();

    let rows = sqlx::query(
        "SELECT r.to_entity_id AS entity, r.to_space_id AS space, v.integer AS value, r.position AS position \
         FROM relations r \
         JOIN values v ON v.entity_id = r.entity_id AND v.property_id = $3 \
         WHERE r.from_entity_id = $1 AND r.type_id = $2 \
         ORDER BY r.position",
    )
    .bind(u(BLOCK))
    .bind(rank_position)
    .bind(value_prop)
    .fetch_all(pool)
    .await
    .unwrap();

    // Only members counted (both ranked A>B), so A is #1 and B is #2.
    assert_eq!(rows.len(), 2, "expected two ranked entities");

    let e0: Uuid = rows[0].get("entity");
    let v0: i64 = rows[0].get("value");
    let s0: Uuid = rows[0].get("space");
    assert_eq!(e0, u(ENTITY_A), "top-ranked entity should be A");
    assert_eq!(v0, 100, "top entity scaled to 100");
    assert_eq!(
        s0,
        u(BLOCK_SPACE),
        "ranked perspective carried on to_space_id"
    );

    let e1: Uuid = rows[1].get("entity");
    let v1: i64 = rows[1].get("value");
    assert_eq!(e1, u(ENTITY_B), "second entity should be B");
    assert_eq!(v1, 50, "B is half of A (1.0 vs 2.0 summed)");

    // provenance: one Aggregated rankings relation per *eligible* submission (2).
    let aggregated = Uuid::parse_str(AGGREGATED_RANKINGS_RELATION_TYPE_ID).unwrap();
    let prov: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relations WHERE from_entity_id = $1 AND type_id = $2",
    )
    .bind(u(BLOCK))
    .bind(aggregated)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        prov, 2,
        "non-member submission must be excluded from provenance"
    );
}

/// Regression for #738: a block whose `TYPES -> Ranking Block` relation and its
/// config arrive in *separate* edits is never registered by `detect()` (which
/// resolves types from the current edit alone), so its rankings were silently
/// never scored. `recompute_block()` must recover the block from the indexed
/// graph and score it once a rank links to it.
#[tokio::test]
async fn cross_edit_block_is_recovered_from_kg_and_scored() {
    let Ok(url) = std::env::var("RANKING_INDEXER_E2E_DATABASE_URL") else {
        eprintln!("skipping e2e: RANKING_INDEXER_E2E_DATABASE_URL not set");
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    // Distinct scenario ids so this can share a DB with the other e2e test.
    const SPACE: u128 = 0xE2E5_0000_2001;
    const MEMBER: u128 = 0xE2E5_0000_2011;
    const BLOCK: u128 = 0xE2E5_0000_2021;
    const TYPE_REL: u128 = 0xE2E5_0000_2022;
    const NAME_VAL: u128 = 0xE2E5_0000_2023;
    const ENT_A: u128 = 0xE2E5_0000_2031;
    const ENT_B: u128 = 0xE2E5_0000_2032;
    const RANK: u128 = 0xE2E5_0000_2041;

    let su = |s: &str| Uuid::parse_str(s).unwrap();

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
    ] {
        sqlx::query(sql).bind(u(SPACE)).execute(pool).await.unwrap();
    }
    sqlx::query("DELETE FROM ranks.ranking_items WHERE ranking_id = $1")
        .bind(u(RANK))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.rankings WHERE id = $1")
        .bind(u(RANK))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.ranking_scores WHERE block_id = $1")
        .bind(u(BLOCK))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.ranking_blocks WHERE id = $1")
        .bind(u(BLOCK))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM members WHERE space_id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed public prerequisites -----------------------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e2')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO members (member_space_id, space_id) VALUES ($1, $2)")
        .bind(u(MEMBER))
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // Seed the KG as kg-indexer would: the block's `TYPES -> Ranking Block`
    // relation and its Name, but WITHOUT registering it in `ranks` — i.e. the
    // exact state where detect() never saw the type and config in one edit.
    sqlx::query(
        "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(u(TYPE_REL).to_string())
    .bind(u(TYPE_REL))
    .bind(su(TYPE_RELATION_TYPE_ID))
    .bind(u(BLOCK))
    .bind(su(RANKING_BLOCK_TYPE_ID))
    .bind(u(SPACE))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO values (id, entity_id, space_id, property_id, text) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(u(NAME_VAL).to_string())
    .bind(u(BLOCK))
    .bind(u(SPACE))
    .bind(su(NAME_PROPERTY_ID))
    .bind("Recovered Block")
    .execute(pool)
    .await
    .unwrap();

    // Precondition: the block is NOT registered in the ranks schema.
    assert!(
        storage.get_ranking_block(u(BLOCK)).await.unwrap().is_none(),
        "precondition: block must be unregistered before the rank arrives"
    );

    // --- a member submits a rank linked to the (unregistered) block --------
    let rank_edit = EditBuilder::new(gid(RANK ^ 0xED17))
        .create_relation(|r| {
            r.id(gid(0x300))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(sid(RANK_TYPE_ID))
        })
        .create_entity(gid(RANK), |e| {
            e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
        })
        .create_relation(|r| {
            r.id(gid(0x301))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(BLOCK))
        })
        .create_relation(|r| {
            r.id(gid(0x302))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(ENT_A))
                .to_space(gid(SPACE))
                .position("a0")
        })
        .create_relation(|r| {
            r.id(gid(0x303))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(ENT_B))
                .to_space(gid(SPACE))
                .position("a1")
        })
        .build();
    apply_detected_edit(&detect(&rank_edit, u(MEMBER), 2, 0), u(MEMBER), &storage)
        .await
        .unwrap();

    // --- the block is recovered from the KG, registered, and scored --------
    let registered: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ranks.ranking_blocks WHERE id = $1")
            .bind(u(BLOCK))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(registered, 1, "block must be recovered from the KG");

    let name: Option<String> =
        sqlx::query_scalar("SELECT name FROM ranks.ranking_blocks WHERE id = $1")
            .bind(u(BLOCK))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        name.as_deref(),
        Some("Recovered Block"),
        "recovered block carries its KG name/config"
    );

    let scores: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ranks.ranking_scores WHERE block_id = $1")
            .bind(u(BLOCK))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        scores, 2,
        "both ranked entities scored once the block is recovered"
    );
}
