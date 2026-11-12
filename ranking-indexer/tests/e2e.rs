//! End-to-end integration test for the full ranking-indexer pipeline.
//!
//! Runs real edits through `detect -> apply_detected_edit` (upsert -> dedup ->
//! eligibility -> scoring -> publish) against a live Postgres and asserts the
//! public `RANK_POSITION` projection. Requires a DB with the public schema + the
//! private `ranks` schema. Set `RANKING_INDEXER_E2E_DATABASE_URL` to the
//! connection string — deliberately its own variable, so these never run against
//! a generic `DATABASE_URL` that lacks the `ranks` schema.
//!
//! Locally, an unset variable skips. **In CI it is a hard failure** (see
//! `e2e_database_url`). Skipping used to be silent everywhere, and the harness
//! reports a `return` as *passed*: this file printed "8 passed" having run
//! nothing, for as long as it has existed. One of these tests asserted
//! behaviour that #882 falsified in August and never failed once. GEO-2872.

use chrono::{DateTime, TimeZone, Utc};
use grc_20::model::builder::EditBuilder;
use hermes_schema::pb::membership::{HermesRoleGranted, HermesRoleRevoked, MembershipRole};
use sdk::core::ids::*;
use sqlx::Row;
use uuid::Uuid;

use ranking_indexer::detect::detect;
use ranking_indexer::membership::{apply_membership_event, MembershipEvent};
use ranking_indexer::models::BlockMeta;
use ranking_indexer::recompute::{apply_detected_edit, recompute_block};
use ranking_indexer::storage::Storage;

/// The e2e database URL, or `None` to skip.
///
/// Unset locally means skip — these need a Postgres with the `ranks` schema and
/// not every checkout has one. Unset **in CI** is a configuration error and
/// panics, because a skip here is indistinguishable from a pass and that is
/// exactly how this suite went a month reporting green while running nothing
/// (GEO-2872).
fn e2e_database_url() -> Option<String> {
    match std::env::var("RANKING_INDEXER_E2E_DATABASE_URL") {
        Ok(url) => Some(url),
        Err(_) if std::env::var("CI").is_ok() => panic!(
            "RANKING_INDEXER_E2E_DATABASE_URL is unset in CI. These tests would skip and \
             report as passed, which is worse than not running them at all. Provision the \
             database in the workflow, or delete the job — do not let it go quiet."
        ),
        Err(_) => {
            eprintln!(
                "skipping e2e: RANKING_INDEXER_E2E_DATABASE_URL not set (set it to a \
                 Postgres with the public + ranks schemas to run these)"
            );
            None
        }
    }
}

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
    let Some(url) = e2e_database_url() else {
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
    sqlx::query("DELETE FROM ranks.members WHERE space_id = $1")
        .bind(u(BLOCK_SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(u(BLOCK_SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed prerequisites --------------------------------------------------
    // The block lives in a DAO space; member1/member2 are members, nonmember
    // isn't. Membership lives in the indexer's own view (`ranks.members`).
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e')")
        .bind(u(BLOCK_SPACE))
        .execute(pool)
        .await
        .unwrap();
    for m in [MEMBER1, MEMBER2] {
        sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
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
        "SELECT r.to_entity_id AS entity, r.to_space_id AS space, r.from_space_id AS from_space, v.integer AS value, r.position AS position \
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
    let fs0: Uuid = rows[0].get("from_space");
    assert_eq!(e0, u(ENTITY_A), "top-ranked entity should be A");
    assert_eq!(v0, 100, "top entity scaled to 100");
    assert_eq!(
        s0,
        u(BLOCK_SPACE),
        "ranked perspective carried on to_space_id"
    );
    assert_eq!(
        fs0,
        u(BLOCK_SPACE),
        "block's home space carried on from_space_id"
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

    // entities: every reified entity the projection mints must be registered in
    // `entities` (the API's source of truth for entity existence). Otherwise
    // the reified entity carrying the rank value has no row and `entity(id)`
    // returns null, hiding the value from the graph.
    let orphans: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relations r \
         LEFT JOIN entities e ON e.id = r.entity_id \
         WHERE r.from_entity_id = $1 AND r.type_id = ANY($2) AND e.id IS NULL",
    )
    .bind(u(BLOCK))
    .bind(&[rank_position, aggregated][..])
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        orphans, 0,
        "projected reified entities must be registered in the entities table"
    );
}

/// Regression for #738: a block whose `TYPES -> Ranking Block` relation and its
/// config arrive in *separate* edits is never registered by `detect()` (which
/// resolves types from the current edit alone), so its rankings were silently
/// never scored. `recompute_block()` must recover the block from the indexed
/// graph and score it once a rank links to it.
#[tokio::test]
async fn cross_edit_block_is_recovered_from_kg_and_scored() {
    let Some(url) = e2e_database_url() else {
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
    sqlx::query("DELETE FROM ranks.members WHERE space_id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed prerequisites --------------------------------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e2')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
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
    .bind(u(TYPE_REL))
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

/// Regression (GEO-2253): `get_block_config_from_kg` must resolve the same
/// datetime-over-legacy-date precedence as `detect::window_bound`, or the
/// live path and the KG-recovery path disagree on an unregistered block's
/// window. Start authors both properties (datetime should win); end authors
/// only the legacy property (must still fall back correctly).
#[tokio::test]
async fn cross_edit_block_recovers_datetime_bounds_preferring_new_property_over_legacy() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    const SPACE: u128 = 0xE2E5_0000_3001;
    const MEMBER: u128 = 0xE2E5_0000_3011;
    const BLOCK: u128 = 0xE2E5_0000_3021;
    const TYPE_REL: u128 = 0xE2E5_0000_3022;
    const START_LEGACY_VAL: u128 = 0xE2E5_0000_3023;
    const START_DATETIME_VAL: u128 = 0xE2E5_0000_3024;
    const END_LEGACY_VAL: u128 = 0xE2E5_0000_3025;
    const ENT_A: u128 = 0xE2E5_0000_3031;
    const RANK: u128 = 0xE2E5_0000_3041;

    let su = |s: &str| Uuid::parse_str(s).unwrap();
    let legacy_start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let new_start = Utc.with_ymd_and_hms(2026, 2, 2, 9, 30, 0).unwrap();
    let legacy_end = Utc.with_ymd_and_hms(2026, 3, 3, 0, 0, 0).unwrap();

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
    sqlx::query("DELETE FROM ranks.members WHERE space_id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed prerequisites --------------------------------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e3')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
        .bind(u(MEMBER))
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // Seed the KG as kg-indexer would: the block's `TYPES -> Ranking Block`
    // relation, plus start authored on BOTH the legacy and datetime property
    // (datetime should win), and end authored on only the legacy property
    // (must fall back). Never registered in `ranks`.
    sqlx::query(
        "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(u(TYPE_REL))
    .bind(u(TYPE_REL))
    .bind(su(TYPE_RELATION_TYPE_ID))
    .bind(u(BLOCK))
    .bind(su(RANKING_BLOCK_TYPE_ID))
    .bind(u(SPACE))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO values (id, entity_id, space_id, property_id, datetime_utc) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(u(START_LEGACY_VAL).to_string())
    .bind(u(BLOCK))
    .bind(u(SPACE))
    .bind(su(RANK_START_DATE_PROPERTY_ID))
    .bind(legacy_start)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO values (id, entity_id, space_id, property_id, datetime_utc) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(u(START_DATETIME_VAL).to_string())
    .bind(u(BLOCK))
    .bind(u(SPACE))
    .bind(su(RANK_START_DATETIME_PROPERTY_ID))
    .bind(new_start)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO values (id, entity_id, space_id, property_id, datetime_utc) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(u(END_LEGACY_VAL).to_string())
    .bind(u(BLOCK))
    .bind(u(SPACE))
    .bind(su(RANK_END_DATE_PROPERTY_ID))
    .bind(legacy_end)
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
            r.id(gid(0x310))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(sid(RANK_TYPE_ID))
        })
        .create_entity(gid(RANK), |e| {
            e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
        })
        .create_relation(|r| {
            r.id(gid(0x311))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(BLOCK))
        })
        .create_relation(|r| {
            r.id(gid(0x312))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(ENT_A))
                .to_space(gid(SPACE))
                .position("a0")
        })
        .build();
    apply_detected_edit(&detect(&rank_edit, u(MEMBER), 2, 0), u(MEMBER), &storage)
        .await
        .unwrap();

    // --- the recovered block prefers the datetime bound over the legacy one,
    //     and falls back to legacy where the datetime bound is absent --------
    let (start_date, end_date): (Option<chrono::DateTime<Utc>>, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("SELECT start_date, end_date FROM ranks.ranking_blocks WHERE id = $1")
            .bind(u(BLOCK))
            .fetch_one(pool)
            .await
            .unwrap();

    assert_eq!(
        start_date,
        Some(new_start),
        "start: the datetime property must win over the legacy one authored on the same block"
    );
    assert_eq!(
        end_date,
        Some(legacy_end),
        "end: must fall back to the legacy property when no datetime property is authored"
    );
}

/// GEO-2328: a Rolling block's submission stays eligible only for
/// `submission_frequency` hours after `submitted_at` — unlike a static
/// block's fixed `[start, end]` window, it ages out purely from elapsed time,
/// with no new edit required. Recomputes the same block twice with different
/// injected `now` values (no real sleep) to simulate what the future sweep
/// binary will do periodically.
#[tokio::test]
async fn rolling_block_keeps_scoring_a_submission_long_after_its_frequency() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    const SPACE: u128 = 0xE2E5_0000_4001;
    const MEMBER: u128 = 0xE2E5_0000_4011;
    const BLOCK: u128 = 0xE2E5_0000_4021;
    const ENT_A: u128 = 0xE2E5_0000_4031;
    const RANK: u128 = 0xE2E5_0000_4041;

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
    sqlx::query("DELETE FROM ranks.members WHERE space_id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed prerequisites --------------------------------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e4')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
        .bind(u(MEMBER))
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- edit 1: a Rolling block with a 1-hour submission_frequency ---------
    // Rolling is a second `TYPES` relation (GEO-2328) alongside the standard
    // `Ranking Block` typing, not a dedicated property.
    let block_edit = EditBuilder::new(gid(1))
        .create_relation(|r| {
            r.id(gid(0x400))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_relation(|r| {
            r.id(gid(0x401))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANK_ROLLING_TYPE_ID))
        })
        .create_entity(gid(BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Rolling Block", None)
                .integer(sid(RANK_SUBMISSION_FREQUENCY_PROPERTY_ID), 1, None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(SPACE), 1, 0), u(SPACE), &storage)
        .await
        .unwrap();

    // --- a member submits a rank at submitted_at = epoch 0 -------------------
    let rank_edit = EditBuilder::new(gid(RANK ^ 0xED17))
        .create_relation(|r| {
            r.id(gid(0x410))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(sid(RANK_TYPE_ID))
        })
        .create_entity(gid(RANK), |e| {
            e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
        })
        .create_relation(|r| {
            r.id(gid(0x411))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(BLOCK))
        })
        .create_relation(|r| {
            r.id(gid(0x412))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(ENT_A))
                .to_space(gid(SPACE))
                .position("a0")
        })
        .build();
    apply_detected_edit(
        &detect(&rank_edit, u(MEMBER), 2, 0), // block_timestamp 0 -> submitted_at = epoch 0
        u(MEMBER),
        &storage,
    )
    .await
    .unwrap();

    let t = |n: i64| DateTime::<Utc>::from_timestamp(n, 0).unwrap();
    let scored_count = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM ranks.ranking_scores WHERE block_id = $1 AND entity_id = $2",
        )
        .bind(u(BLOCK))
        .bind(u(ENT_A))
        .fetch_one(pool)
        .await
        .unwrap()
    };

    // --- recompute at now = 30 min: still within the 1-hour frequency -------
    recompute_block(u(BLOCK), BlockMeta::default(), t(30 * 60), &storage)
        .await
        .unwrap();
    assert_eq!(
        scored_count().await,
        1,
        "a submission still within its submission_frequency must be scored"
    );

    // --- recompute at now = 2h: past one frequency, and at 40d: past every ---
    // Neither drops it. `submission_frequency` is a half-life for *weight*, not
    // a cliff for membership (GEO-2869): an aged ballot's entities sink in the
    // table, they never leave it. This assertion is the inverse of the one it
    // replaces, which asserted the pre-GEO-2515 expiry and had been wrong since
    // #882 changed it in August — it never failed because this whole file skips
    // without RANKING_INDEXER_E2E_DATABASE_URL and nothing in CI sets it.
    recompute_block(u(BLOCK), BlockMeta::default(), t(2 * 3600), &storage)
        .await
        .unwrap();
    assert_eq!(
        scored_count().await,
        1,
        "a submission past one submission_frequency is decayed, not dropped"
    );

    recompute_block(u(BLOCK), BlockMeta::default(), t(40 * 24 * 3600), &storage)
        .await
        .unwrap();
    assert_eq!(
        scored_count().await,
        1,
        "a submission 40 days past a 1-hour frequency still holds its row; \
         removing it would delete every entity only it ranked"
    );
}

/// Regression: a projection row written before this PR has `from_space_id IS
/// NULL` but keeps the deterministic v5 relation id (the PK). The step-2 DELETE
/// in `replace_rank_position_projection` must clear it on the next recompute;
/// if it were scoped by `from_space_id = block_space_id`, the NULL row would
/// survive and the ON-CONFLICT-free re-INSERT would collide on the PK →
/// duplicate-key error → aborted tx → post-#745 crash loop. This guards that a
/// pre-fix block self-heals (re-acquires `from_space_id = block_space`) on
/// recompute instead of crash-looping.
#[tokio::test]
async fn prefix_null_from_space_projection_self_heals_on_recompute() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    // Distinct scenario ids so this can share a DB with the other e2e tests.
    const SPACE: u128 = 0xE2E7_0000_0001;
    const MEMBER: u128 = 0xE2E7_0000_0011;
    const BLOCK: u128 = 0xE2E7_0000_0021;
    const ENT_A: u128 = 0xE2E7_0000_0031;
    const ENT_B: u128 = 0xE2E7_0000_0032;
    const RANK: u128 = 0xE2E7_0000_0041;

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
        "DELETE FROM ranks.members WHERE space_id = $1",
        "DELETE FROM spaces WHERE id = $1",
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

    // --- seed: DAO space with one member ------------------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e7')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
        .bind(u(MEMBER))
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- create the block ---------------------------------------------------
    let block_edit = EditBuilder::new(gid(0xE2E7_0001))
        .create_relation(|r| {
            r.id(gid(0xE2E7_0100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_relation(|r| {
            r.id(gid(0xE2E7_0101))
                .relation_type(sid(RANK_AGGREGATION_RESTRICTION_PROPERTY_ID))
                .from(gid(BLOCK))
                .to(sid(RANK_RESTRICTION_MEMBERS_AND_EDITORS_ID))
        })
        .create_entity(gid(BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Top Songs", None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(SPACE), 1, 0), u(SPACE), &storage)
        .await
        .unwrap();

    // --- a member submits a rank → a normal projection is written -----------
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
                    .to_space(gid(SPACE))
                    .position("a0")
            })
            .create_relation(|r| {
                r.id(gid(rel_base + 3))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(gid(second))
                    .to_space(gid(SPACE))
                    .position("a1")
            })
            .build()
    };

    apply_detected_edit(
        &detect(&submit(RANK, 0xE2E7_0200, ENT_A, ENT_B), u(MEMBER), 2, 0),
        u(MEMBER),
        &storage,
    )
    .await
    .unwrap();

    let rank_position = Uuid::parse_str(RANK_POSITION_RELATION_TYPE_ID).unwrap();
    let aggregated = Uuid::parse_str(AGGREGATED_RANKINGS_RELATION_TYPE_ID).unwrap();

    // The first recompute produced deterministic-id projection relations.
    let projected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relations WHERE from_entity_id = $1 AND type_id = ANY($2)",
    )
    .bind(u(BLOCK))
    .bind(&[rank_position, aggregated][..])
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(projected > 0, "first recompute must write a projection");

    // --- SIMULATE a pre-fix row: blank out from_space_id ---------------------
    // Rows written before this PR carried the deterministic PK id but NULL
    // from_space_id. Replicate exactly that state.
    sqlx::query(
        "UPDATE relations SET from_space_id = NULL \
         WHERE from_entity_id = $1 AND type_id = ANY($2)",
    )
    .bind(u(BLOCK))
    .bind(&[rank_position, aggregated][..])
    .execute(pool)
    .await
    .unwrap();

    let null_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relations \
         WHERE from_entity_id = $1 AND type_id = ANY($2) AND from_space_id IS NULL",
    )
    .bind(u(BLOCK))
    .bind(&[rank_position, aggregated][..])
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(null_rows, projected, "pre-fix state: all rows now NULL");

    // --- recompute the same block: must NOT crash with a duplicate-key error -
    // Re-applying the rank forces another `replace_rank_position_projection`.
    // The step-2 DELETE must clear the NULL-from_space rows (matching on space
    // alone) so the re-INSERT of the same deterministic ids succeeds.
    apply_detected_edit(
        &detect(&submit(RANK, 0xE2E7_0200, ENT_A, ENT_B), u(MEMBER), 3, 0),
        u(MEMBER),
        &storage,
    )
    .await
    .expect("recompute over a pre-fix (NULL from_space_id) projection must not collide");

    // --- the block self-healed: every projection row now carries from_space --
    let remaining_null: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relations \
         WHERE from_entity_id = $1 AND type_id = ANY($2) AND from_space_id IS NULL",
    )
    .bind(u(BLOCK))
    .bind(&[rank_position, aggregated][..])
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(remaining_null, 0, "recompute must clear pre-fix NULL rows");

    let healed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relations \
         WHERE from_entity_id = $1 AND type_id = ANY($2) AND from_space_id = $3",
    )
    .bind(u(BLOCK))
    .bind(&[rank_position, aggregated][..])
    .bind(u(SPACE))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        healed, projected,
        "every projection row must now carry from_space_id = block space"
    );
}

// Membership-lifecycle scenario UUIDs (distinct namespace so both e2e tests
// can run against the same database, even concurrently).
const M_SPACE: u128 = 0xE2E6_0000_0001;
const M_MEMBER: u128 = 0xE2E6_0000_0011;
const M_LATE: u128 = 0xE2E6_0000_0012;
const M_BLOCK: u128 = 0xE2E6_0000_0021;
const M_ENTITY_A: u128 = 0xE2E6_0000_0031;
const M_ENTITY_B: u128 = 0xE2E6_0000_0032;
const M_RANK1: u128 = 0xE2E6_0000_0041;
const M_RANK2: u128 = 0xE2E6_0000_0042;

/// A rank submitted by a non-member must be integrated when they become a
/// member (or editor), and dropped again when the role is revoked.
#[tokio::test]
async fn membership_events_integrate_and_drop_rankings() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
        "DELETE FROM ranks.members WHERE space_id = $1",
        "DELETE FROM ranks.editors WHERE space_id = $1",
        "DELETE FROM spaces WHERE id = $1",
    ] {
        sqlx::query(sql)
            .bind(u(M_SPACE))
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM ranks.ranking_items WHERE ranking_id = ANY($1)")
        .bind(&[u(M_RANK1), u(M_RANK2)][..])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.rankings WHERE id = ANY($1)")
        .bind(&[u(M_RANK1), u(M_RANK2)][..])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.ranking_scores WHERE block_id = $1")
        .bind(u(M_BLOCK))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ranks.ranking_blocks WHERE id = $1")
        .bind(u(M_BLOCK))
        .execute(pool)
        .await
        .unwrap();

    // --- seed: DAO space with one founding member ---------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e6')")
        .bind(u(M_SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
        .bind(u(M_MEMBER))
        .bind(u(M_SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- create the block + two submissions ---------------------------------
    let block_edit = EditBuilder::new(gid(0xE2E6_0001))
        .create_relation(|r| {
            r.id(gid(0xE2E6_0100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(M_BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_relation(|r| {
            r.id(gid(0xE2E6_0101))
                .relation_type(sid(RANK_AGGREGATION_RESTRICTION_PROPERTY_ID))
                .from(gid(M_BLOCK))
                .to(sid(RANK_RESTRICTION_MEMBERS_AND_EDITORS_ID))
        })
        .create_entity(gid(M_BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Top Tokens", None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(M_SPACE), 1, 0), u(M_SPACE), &storage)
        .await
        .unwrap();

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
                    .to(gid(M_BLOCK))
            })
            .create_relation(|r| {
                r.id(gid(rel_base + 2))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(gid(first))
                    .to_space(gid(M_SPACE))
                    .position("a0")
            })
            .create_relation(|r| {
                r.id(gid(rel_base + 3))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(gid(second))
                    .to_space(gid(M_SPACE))
                    .position("a1")
            })
            .build()
    };

    apply_detected_edit(
        &detect(
            &submit(M_RANK1, 0xE2E6_0200, M_ENTITY_A, M_ENTITY_B),
            u(M_MEMBER),
            2,
            0,
        ),
        u(M_MEMBER),
        &storage,
    )
    .await
    .unwrap();
    // The latecomer submits while NOT yet a member — must be excluded.
    apply_detected_edit(
        &detect(
            &submit(M_RANK2, 0xE2E6_0210, M_ENTITY_A, M_ENTITY_B),
            u(M_LATE),
            3,
            0,
        ),
        u(M_LATE),
        &storage,
    )
    .await
    .unwrap();

    let aggregated = Uuid::parse_str(AGGREGATED_RANKINGS_RELATION_TYPE_ID).unwrap();
    let contributing = |pool: &sqlx::PgPool| {
        let pool = pool.clone();
        async move {
            let rows: Vec<(Uuid,)> = sqlx::query_as(
                "SELECT to_entity_id FROM relations WHERE from_entity_id = $1 AND type_id = $2",
            )
            .bind(u(M_BLOCK))
            .bind(aggregated)
            .fetch_all(&pool)
            .await
            .unwrap();
            rows.into_iter().map(|(id,)| id).collect::<Vec<_>>()
        }
    };

    let prov = contributing(pool).await;
    assert_eq!(prov, vec![u(M_RANK1)], "latecomer must start excluded");

    // --- ROLE_GRANTED(MEMBER): the existing rank is integrated ---------------
    let granted = MembershipEvent::RoleGranted(HermesRoleGranted {
        space_id: u(M_SPACE).as_bytes().to_vec(),
        member_space_id: u(M_LATE).as_bytes().to_vec(),
        role: MembershipRole::Member as i32,
        meta: None,
    });
    apply_membership_event(&granted, &storage).await.unwrap();

    let mut prov = contributing(pool).await;
    prov.sort();
    assert_eq!(
        prov,
        vec![u(M_RANK1), u(M_RANK2)],
        "becoming a member must integrate the previously-excluded rank"
    );

    // --- ROLE_REVOKED(MEMBER): the rank drops out again ----------------------
    let revoked = MembershipEvent::RoleRevoked(HermesRoleRevoked {
        space_id: u(M_SPACE).as_bytes().to_vec(),
        member_space_id: u(M_LATE).as_bytes().to_vec(),
        role: MembershipRole::Member as i32,
        meta: None,
    });
    apply_membership_event(&revoked, &storage).await.unwrap();

    let prov = contributing(pool).await;
    assert_eq!(
        prov,
        vec![u(M_RANK1)],
        "revoking membership must drop the rank from the aggregate"
    );

    // --- ROLE_GRANTED(EDITOR): editors are eligible too ----------------------
    let granted_editor = MembershipEvent::RoleGranted(HermesRoleGranted {
        space_id: u(M_SPACE).as_bytes().to_vec(),
        member_space_id: u(M_LATE).as_bytes().to_vec(),
        role: MembershipRole::Editor as i32,
        meta: None,
    });
    apply_membership_event(&granted_editor, &storage)
        .await
        .unwrap();

    let mut prov = contributing(pool).await;
    prov.sort();
    assert_eq!(
        prov,
        vec![u(M_RANK1), u(M_RANK2)],
        "an editor's rank must be integrated"
    );

    // --- ROLE_REVOKED(EDITOR): the rank drops out again ----------------------
    let revoked_editor = MembershipEvent::RoleRevoked(HermesRoleRevoked {
        space_id: u(M_SPACE).as_bytes().to_vec(),
        member_space_id: u(M_LATE).as_bytes().to_vec(),
        role: MembershipRole::Editor as i32,
        meta: None,
    });
    apply_membership_event(&revoked_editor, &storage)
        .await
        .unwrap();

    let prov = contributing(pool).await;
    assert_eq!(
        prov,
        vec![u(M_RANK1)],
        "revoking the editor role must drop the rank from the aggregate"
    );
    let view_rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM ranks.members WHERE member_space_id = $1 AND space_id = $2)
              + (SELECT count(*) FROM ranks.editors WHERE member_space_id = $1 AND space_id = $2)",
    )
    .bind(u(M_LATE))
    .bind(u(M_SPACE))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        view_rows, 0,
        "both roles revoked — view tables must be empty"
    );
}

/// Regression for the `values_pkey` crash loop (prod incident 2026-06-22): a
/// recompute `INSERT INTO values` without `ON CONFLICT` collided whenever a
/// prior projection value row survived step-1's DELETE — e.g. its
/// `RANK_POSITION` relation was missing, so the DELETE's `entity_id IN
/// (SELECT … FROM relations …)` subquery never resolved it. The duplicate-key
/// was misclassified as transient, so the consumer crash-looped forever on the
/// same offset. The recompute INSERTs are now idempotent; re-running over an
/// orphaned value row must converge instead of erroring.
#[tokio::test]
async fn recompute_is_idempotent_when_a_prior_value_row_is_orphaned() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    // Distinct scenario ids so this can share a DB with the other e2e tests.
    // They were not distinct: this test, `rolling_block_keeps_scoring_...` and
    // `cross_edit_rank_is_recovered_...` all used 4001/4021/4041 and each opens by
    // DELETEing that space, so run in parallel they wiped each other. Nobody saw it
    // because the suite never executed (GEO-2872). Renumbered to 5xxx.
    const SPACE: u128 = 0xE2E5_0000_5001;
    const MEMBER: u128 = 0xE2E5_0000_5011;
    const BLK: u128 = 0xE2E5_0000_5021;
    const ENT_A: u128 = 0xE2E5_0000_5031;
    const ENT_B: u128 = 0xE2E5_0000_5032;
    const RNK: u128 = 0xE2E5_0000_5041;

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
    ] {
        sqlx::query(sql).bind(u(SPACE)).execute(pool).await.unwrap();
    }
    for sql in [
        "DELETE FROM ranks.ranking_items WHERE ranking_id = $1",
        "DELETE FROM ranks.rankings WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(RNK)).execute(pool).await.unwrap();
    }
    for sql in [
        "DELETE FROM ranks.ranking_scores WHERE block_id = $1",
        "DELETE FROM ranks.ranking_blocks WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(BLK)).execute(pool).await.unwrap();
    }
    sqlx::query("DELETE FROM ranks.members WHERE space_id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed prerequisites + build the projection -------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e3')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
        .bind(u(MEMBER))
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();

    let block_edit = EditBuilder::new(gid(0x3001))
        .create_relation(|r| {
            r.id(gid(0x3100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_relation(|r| {
            r.id(gid(0x3101))
                .relation_type(sid(RANK_AGGREGATION_RESTRICTION_PROPERTY_ID))
                .from(gid(BLK))
                .to(sid(RANK_RESTRICTION_MEMBERS_AND_EDITORS_ID))
        })
        .create_entity(gid(BLK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Top Films", None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(SPACE), 1, 0), u(SPACE), &storage)
        .await
        .unwrap();

    let submission = EditBuilder::new(gid(0x3001 ^ 0xED17))
        .create_relation(|r| {
            r.id(gid(0x3200))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(RNK))
                .to(sid(RANK_TYPE_ID))
        })
        .create_entity(gid(RNK), |e| {
            e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
        })
        .create_relation(|r| {
            r.id(gid(0x3201))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(RNK))
                .to(gid(BLK))
        })
        .create_relation(|r| {
            r.id(gid(0x3202))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RNK))
                .to(gid(ENT_A))
                .to_space(gid(SPACE))
                .position("a0")
        })
        .create_relation(|r| {
            r.id(gid(0x3203))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RNK))
                .to(gid(ENT_B))
                .to_space(gid(SPACE))
                .position("a1")
        })
        .build();
    let detected_submission = detect(&submission, u(MEMBER), 2, 0);
    apply_detected_edit(&detected_submission, u(MEMBER), &storage)
        .await
        .unwrap();

    let rank_position = u(Uuid::parse_str(RANK_POSITION_RELATION_TYPE_ID)
        .unwrap()
        .as_u128());
    let value_prop = Uuid::parse_str(RANK_POSITION_VALUE_PROPERTY_ID).unwrap();

    // --- recreate the prod failure state: orphan one projection value row ---
    // Delete a single RANK_POSITION relation but leave its value row behind.
    // The next recompute's value-DELETE resolves entity_ids through the
    // relations it lists, so it can no longer see this orphan — exactly the
    // condition that made the value INSERT collide on `values.id`.
    let victim: Uuid = sqlx::query_scalar(
        "SELECT entity_id FROM relations WHERE from_entity_id = $1 AND type_id = $2 LIMIT 1",
    )
    .bind(u(BLK))
    .bind(rank_position)
    .fetch_one(pool)
    .await
    .unwrap();
    let val_before: i64 = sqlx::query_scalar("SELECT count(*) FROM values WHERE entity_id = $1")
        .bind(victim)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        val_before, 1,
        "precondition: reified entity has its value row"
    );
    sqlx::query(
        "DELETE FROM relations WHERE entity_id = $1 AND type_id = $2 AND from_entity_id = $3",
    )
    .bind(victim)
    .bind(rank_position)
    .bind(u(BLK))
    .execute(pool)
    .await
    .unwrap();

    // --- recompute must converge, not crash on values_pkey ------------------
    let result = apply_detected_edit(&detected_submission, u(MEMBER), &storage).await;
    assert!(
        result.is_ok(),
        "recompute must be idempotent when a value row is orphaned, got: {result:?}"
    );

    // projection restored: A (#1, value 100) and B (#2, value 50).
    let rows = sqlx::query(
        "SELECT r.to_entity_id AS entity, v.integer AS value \
         FROM relations r JOIN values v ON v.entity_id = r.entity_id AND v.property_id = $3 \
         WHERE r.from_entity_id = $1 AND r.type_id = $2 ORDER BY r.position",
    )
    .bind(u(BLK))
    .bind(rank_position)
    .bind(value_prop)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "projection restored to two ranked entities");
    let e0: Uuid = rows[0].get("entity");
    let v0: i64 = rows[0].get("value");
    assert_eq!(e0, u(ENT_A), "top-ranked entity should be A");
    assert_eq!(
        v0, 100,
        "top entity scaled to 100 after idempotent recompute"
    );
    let v1: i64 = rows[1].get("value");
    assert_eq!(v1, 50, "second entity converges to 50");
}

/// Companion to `cross_edit_block_is_recovered_from_kg_and_scored`, for the
/// identical split-edit gap on Rank entities — except there's no reactive
/// live-path trigger for ranks (unlike blocks), so recovery is
/// periodic-sweep-only: `find_unregistered_ranks` + `get_rank_config_from_kg`
/// + a manual `upsert_ranking` + `recompute_block`, exactly what
/// `bin/backfill_blocks.rs` runs on a schedule.
#[tokio::test]
async fn cross_edit_rank_is_recovered_from_kg_and_scored() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    const BLOCK_SPACE: u128 = 0xE2E5_0000_6001;
    const MEMBER_SPACE: u128 = 0xE2E5_0000_6011; // the rank's own (personal) space
    const BLOCK: u128 = 0xE2E5_0000_6021;
    const RANK: u128 = 0xE2E5_0000_6041;
    const TYPE_REL: u128 = 0xE2E5_0000_6042;
    const BLOCK_REL: u128 = 0xE2E5_0000_6043;
    const RANK_TYPE_VAL: u128 = 0xE2E5_0000_6044;
    const ENT_A: u128 = 0xE2E5_0000_6051;
    const ENT_B: u128 = 0xE2E5_0000_6052;

    let su = |s: &str| Uuid::parse_str(s).unwrap();

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
        "DELETE FROM entities WHERE id = $2",
    ] {
        sqlx::query(sql)
            .bind(u(MEMBER_SPACE))
            .bind(u(RANK))
            .execute(pool)
            .await
            .unwrap();
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
    sqlx::query("DELETE FROM spaces WHERE id IN ($1, $2)")
        .bind(u(BLOCK_SPACE))
        .bind(u(MEMBER_SPACE))
        .execute(pool)
        .await
        .unwrap();

    // --- seed prerequisites: a Personal block (no membership check needed,
    // "All of Geo" admits any submitter) already registered in `ranks`. ----
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'Personal', '0xe2e3')")
        .bind(u(BLOCK_SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ranks.ranking_blocks (id, space_id, name, updated_at) \
         VALUES ($1, $2, 'Recovered Rank Target', now())",
    )
    .bind(u(BLOCK))
    .bind(u(BLOCK_SPACE))
    .execute(pool)
    .await
    .unwrap();

    // --- seed the KG as kg-indexer would: the rank's `TYPES -> Rank`
    // relation, its rank_type value, its `RANK_BLOCK` link, and its entity
    // row — WITHOUT registering it in `ranks.rankings` — i.e. the exact
    // state where detect() never saw the type and the entity in one edit. --
    sqlx::query(
        "INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) \
         VALUES ($1, '1700000000', '42', '1700000100', '43')",
    )
    .bind(u(RANK))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
         VALUES ($1, $1, $2, $3, $4, $5)",
    )
    .bind(u(TYPE_REL))
    .bind(su(TYPE_RELATION_TYPE_ID))
    .bind(u(RANK))
    .bind(su(RANK_TYPE_ID))
    .bind(u(MEMBER_SPACE))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO values (id, entity_id, space_id, property_id, text) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(u(RANK_TYPE_VAL).to_string())
    .bind(u(RANK))
    .bind(u(MEMBER_SPACE))
    .bind(su(RANK_TYPE_PROPERTY_ID))
    .bind("ORDINAL")
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
         VALUES ($1, $1, $2, $3, $4, $5)",
    )
    .bind(u(BLOCK_REL))
    .bind(su(RANK_BLOCK_RELATION_TYPE_ID))
    .bind(u(RANK))
    .bind(u(BLOCK))
    .bind(u(MEMBER_SPACE))
    .execute(pool)
    .await
    .unwrap();

    // Votes landed via the live path at the time (RANK_VOTES relations are
    // collected regardless of whether their `from` entity is classified),
    // so they're already sitting in `ranking_items` — orphaned, since
    // `ranks.rankings` has no row for RANK to attach them to yet.
    for (entity, position) in [(ENT_A, "a0"), (ENT_B, "a1")] {
        sqlx::query(
            "INSERT INTO ranks.ranking_items (ranking_id, entity_id, space_id, position) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(u(RANK))
        .bind(u(entity))
        .bind(u(MEMBER_SPACE))
        .bind(position)
        .execute(pool)
        .await
        .unwrap();
    }

    // --- precondition: the rank is NOT registered in the ranks schema ------
    let registered: i64 = sqlx::query_scalar("SELECT count(*) FROM ranks.rankings WHERE id = $1")
        .bind(u(RANK))
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        registered, 0,
        "precondition: rank must be unregistered before recovery"
    );

    // --- the sweep's recovery path: find it, reconstruct it, score it ------
    let candidates = storage.find_unregistered_ranks().await.unwrap();
    assert!(
        candidates.contains(&u(RANK)),
        "find_unregistered_ranks must surface the orphaned rank"
    );

    let recovered = storage
        .get_rank_config_from_kg(u(RANK))
        .await
        .unwrap()
        .expect("rank must be reconstructable from the KG");
    assert_eq!(recovered.rank_type.as_deref(), Some("ORDINAL"));
    assert_eq!(
        recovered.block_id,
        Some(u(BLOCK)),
        "block_id must be resolved directly from the RANK_BLOCK relation, \
         not from ranks.rankings (set_ranking_block silently no-ops against \
         an unregistered rank)"
    );

    storage.upsert_ranking(&recovered).await.unwrap();
    recompute_block(u(BLOCK), BlockMeta::default(), Utc::now(), &storage)
        .await
        .unwrap();

    let scores: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ranks.ranking_scores WHERE block_id = $1")
            .bind(u(BLOCK))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        scores, 2,
        "both previously-orphaned items scored once the rank is recovered"
    );
}

/// GEO-3676: a `ranks.ranking_block_config` row switches a block to average
/// mode, and adding or changing the row recomputes the block with no ballot edit
/// — through the same `find_pending_block_configs` + `recompute_block` pass
/// `bin/backfill_blocks.rs` runs hourly. Also checks the ballot count is stored
/// and that average mode keys rows by entity only.
#[tokio::test]
async fn block_config_switches_scoring_and_recomputes_without_a_ballot_edit() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    const SPACE: u128 = 0xE2E5_3676_0001;
    const OTHER_SPACE: u128 = 0xE2E5_3676_0002;
    const MEMBERS: [u128; 3] = [0xE2E5_3676_0011, 0xE2E5_3676_0012, 0xE2E5_3676_0013];
    const BLOCK: u128 = 0xE2E5_3676_0021;
    const RANKS: [u128; 3] = [0xE2E5_3676_0041, 0xE2E5_3676_0042, 0xE2E5_3676_0043];
    const A: u128 = 0xE2E5_3676_0031;
    const B: u128 = 0xE2E5_3676_0032;
    const FILL: [u128; 6] = [
        0xE2E5_3676_0051,
        0xE2E5_3676_0052,
        0xE2E5_3676_0053,
        0xE2E5_3676_0054,
        0xE2E5_3676_0055,
        0xE2E5_3676_0056,
    ];

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
        "DELETE FROM ranks.members WHERE space_id = $1",
        "DELETE FROM spaces WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(SPACE)).execute(pool).await.unwrap();
    }
    let rank_ids: Vec<Uuid> = RANKS.iter().map(|r| u(*r)).collect();
    for sql in [
        "DELETE FROM ranks.ranking_items WHERE ranking_id = ANY($1)",
        "DELETE FROM ranks.rankings WHERE id = ANY($1)",
    ] {
        sqlx::query(sql)
            .bind(&rank_ids[..])
            .execute(pool)
            .await
            .unwrap();
    }
    for sql in [
        "DELETE FROM ranks.ranking_scores WHERE block_id = $1",
        "DELETE FROM ranks.ranking_blocks WHERE id = $1",
        "DELETE FROM ranks.ranking_block_config WHERE block_id = $1",
    ] {
        sqlx::query(sql).bind(u(BLOCK)).execute(pool).await.unwrap();
    }

    // --- a DAO block with three members ------------------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e3676')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    for m in MEMBERS {
        sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
            .bind(u(m))
            .bind(u(SPACE))
            .execute(pool)
            .await
            .unwrap();
    }
    let block_edit = EditBuilder::new(gid(0x3676_0001))
        .create_relation(|r| {
            r.id(gid(0x3676_0100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_entity(gid(BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Best debates (e2e)", None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(SPACE), 1, 0), u(SPACE), &storage)
        .await
        .unwrap();

    // An ordinal ballot over `(entity, space)` in order.
    let submit = |rank: u128, rel_base: u128, votes: &[(u128, u128)]| {
        let mut edit = EditBuilder::new(gid(rank ^ 0xED17))
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
            });
        const POSITIONS: [&str; 3] = ["a0", "a1", "a2"];
        for (i, (entity, space)) in votes.iter().enumerate() {
            let position = POSITIONS[i];
            edit = edit.create_relation(|r| {
                r.id(gid(rel_base + 2 + i as u128))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(rank))
                    .to(gid(*entity))
                    .to_space(gid(*space))
                    .position(position)
            });
        }
        edit.build()
    };

    // Ballot 1 ranks A first of 3. Ballots 2 and 3 each rank B last of 3 —
    // the third under a different space id.
    let ballots = [
        (
            RANKS[0],
            0x3676_0200,
            vec![(A, SPACE), (FILL[0], SPACE), (FILL[1], SPACE)],
        ),
        (
            RANKS[1],
            0x3676_0210,
            vec![(FILL[2], SPACE), (FILL[3], SPACE), (B, SPACE)],
        ),
        (
            RANKS[2],
            0x3676_0220,
            vec![(FILL[4], SPACE), (FILL[5], SPACE), (B, OTHER_SPACE)],
        ),
    ];
    for (i, (rank, rel_base, votes)) in ballots.iter().enumerate() {
        let member = MEMBERS[i];
        apply_detected_edit(
            &detect(&submit(*rank, *rel_base, votes), u(member), 2 + i as i64, 0),
            u(member),
            &storage,
        )
        .await
        .unwrap();
    }

    type Row = (Uuid, Uuid, f64, i32, i32);
    let scores = || async {
        sqlx::query_as::<_, Row>(
            "SELECT entity_id, space_id, score, position, ballots FROM ranks.ranking_scores \
             WHERE block_id = $1 ORDER BY position",
        )
        .bind(u(BLOCK))
        .fetch_all(pool)
        .await
        .unwrap()
    };
    let published_b = || async {
        let rank_position = Uuid::parse_str(RANK_POSITION_RELATION_TYPE_ID).unwrap();
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM relations \
             WHERE from_entity_id = $1 AND type_id = $2 AND to_entity_id = $3",
        )
        .bind(u(BLOCK))
        .bind(rank_position)
        .bind(u(B))
        .fetch_one(pool)
        .await
        .unwrap()
    };

    // --- no config row: summed, (entity, space) rows, ballots recorded -------
    let sum_rows = scores().await;
    assert_eq!(sum_rows.len(), 9, "9 votes, B under two spaces -> two rows");
    assert!(
        sum_rows.iter().all(|r| r.4 == 1),
        "each row is on one ballot"
    );
    let b_rows: Vec<&Row> = sum_rows.iter().filter(|r| r.0 == u(B)).collect();
    assert_eq!(b_rows.len(), 2);
    assert_eq!(published_b().await, 2);
    assert!(!storage
        .find_pending_block_configs()
        .await
        .unwrap()
        .contains(&u(BLOCK)));

    // --- add a config row: pending, then applied by the backfill pass --------
    sqlx::query(
        "INSERT INTO ranks.ranking_block_config (block_id, max_rows, scoring_mode) \
         VALUES ($1, 2000, 'average')",
    )
    .bind(u(BLOCK))
    .execute(pool)
    .await
    .unwrap();
    assert!(storage
        .find_pending_block_configs()
        .await
        .unwrap()
        .contains(&u(BLOCK)));
    // Nothing changed yet: a config row alone is not a recompute.
    assert_eq!(scores().await, sum_rows);

    recompute_block(u(BLOCK), BlockMeta::default(), Utc::now(), &storage)
        .await
        .unwrap();
    assert!(!storage
        .find_pending_block_configs()
        .await
        .unwrap()
        .contains(&u(BLOCK)));

    let avg_rows = scores().await;
    assert_eq!(avg_rows.len(), 8, "B is one row in average mode");
    let a = avg_rows.iter().find(|r| r.0 == u(A)).unwrap();
    let b = avg_rows.iter().find(|r| r.0 == u(B)).unwrap();
    assert!((a.2 - 0.875).abs() < 1e-12, "A = (1.0 + 0.75) / 2");
    assert!(
        (b.2 - 1.75 / 3.0).abs() < 1e-9,
        "B = (0.5 + 0.5 + 0.75) / 3"
    );
    assert_eq!((a.4, b.4), (1, 2), "ballot counts");
    assert!(a.3 < b.3, "A is above B");
    // One ballot each under SPACE and OTHER_SPACE: the lower id is carried.
    assert_eq!(b.1, u(SPACE.min(OTHER_SPACE)));
    assert_eq!(published_b().await, 1, "one published row for B");

    // --- change it back: pending again, and the recompute restores sums -------
    sqlx::query(
        "UPDATE ranks.ranking_block_config SET max_rows = 200, scoring_mode = 'sum', \
         updated_at = now() WHERE block_id = $1",
    )
    .bind(u(BLOCK))
    .execute(pool)
    .await
    .unwrap();
    assert!(storage
        .find_pending_block_configs()
        .await
        .unwrap()
        .contains(&u(BLOCK)));
    recompute_block(u(BLOCK), BlockMeta::default(), Utc::now(), &storage)
        .await
        .unwrap();
    assert_eq!(scores().await, sum_rows);
    assert_eq!(published_b().await, 2);
    let applied: (Option<i32>, Option<String>) = sqlx::query_as(
        "SELECT applied_max_rows, applied_scoring_mode FROM ranks.ranking_block_config \
         WHERE block_id = $1",
    )
    .bind(u(BLOCK))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(applied, (Some(200), Some("sum".to_string())));
}

/// GEO-3676: a 2,000-row average-mode block end to end, and how long one
/// recompute of it takes (printed; run with `--nocapture` to read it). A
/// 2,100-item ballot is cut to the configured 2,000 rows, positions stay
/// contiguous, and the projection publishes exactly 2,000 relations.
#[tokio::test]
async fn a_2000_row_block_is_capped_and_published() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    const SPACE: u128 = 0xE2E5_3676_1001;
    const MEMBER: u128 = 0xE2E5_3676_1011;
    const BLOCK: u128 = 0xE2E5_3676_1021;
    const RANK: u128 = 0xE2E5_3676_1041;
    const FIRST_ENTITY: u128 = 0xE2E5_3676_2000_0000;
    const ITEMS: usize = 2_100;

    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
        "DELETE FROM ranks.members WHERE space_id = $1",
        "DELETE FROM spaces WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(SPACE)).execute(pool).await.unwrap();
    }
    for sql in [
        "DELETE FROM ranks.ranking_items WHERE ranking_id = $1",
        "DELETE FROM ranks.rankings WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(RANK)).execute(pool).await.unwrap();
    }
    for sql in [
        "DELETE FROM ranks.ranking_scores WHERE block_id = $1",
        "DELETE FROM ranks.ranking_blocks WHERE id = $1",
        "DELETE FROM ranks.ranking_block_config WHERE block_id = $1",
    ] {
        sqlx::query(sql).bind(u(BLOCK)).execute(pool).await.unwrap();
    }

    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e3676b')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
        .bind(u(MEMBER))
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ranks.ranking_block_config (block_id, max_rows, scoring_mode) \
         VALUES ($1, 2000, 'average')",
    )
    .bind(u(BLOCK))
    .execute(pool)
    .await
    .unwrap();

    let block_edit = EditBuilder::new(gid(0x3676_1001))
        .create_relation(|r| {
            r.id(gid(0x3676_1100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_entity(gid(BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Best debates (2,000-row e2e)", None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(SPACE), 1, 0), u(SPACE), &storage)
        .await
        .unwrap();

    // Fixed-width keys sort in ballot order. The builder borrows them for the
    // edit's lifetime, so they are leaked (once, in a test).
    let positions: Vec<&'static str> = (0..ITEMS)
        .map(|i| &*Box::leak(format!("a{i:05}").into_boxed_str()))
        .collect();
    let mut ballot = EditBuilder::new(gid(RANK ^ 0xED17))
        .create_relation(|r| {
            r.id(gid(0x3676_1200))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(sid(RANK_TYPE_ID))
        })
        .create_entity(gid(RANK), |e| {
            e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
        })
        .create_relation(|r| {
            r.id(gid(0x3676_1201))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(BLOCK))
        });
    for (i, position) in positions.iter().copied().enumerate() {
        ballot = ballot.create_relation(|r| {
            r.id(gid(0x3676_3000_0000 + i as u128))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(FIRST_ENTITY + i as u128))
                .to_space(gid(SPACE))
                .position(position)
        });
    }
    apply_detected_edit(
        &detect(&ballot.build(), u(MEMBER), 2, 0),
        u(MEMBER),
        &storage,
    )
    .await
    .unwrap();

    // Time recomputes of the full block (what every ballot edit costs).
    let mut times = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        recompute_block(u(BLOCK), BlockMeta::default(), Utc::now(), &storage)
            .await
            .unwrap();
        times.push(started.elapsed());
    }
    eprintln!("2,000-row recompute_block timings: {times:?}");

    let (rows, min_pos, max_pos): (i64, i32, i32) = sqlx::query_as(
        "SELECT count(*), min(position), max(position) FROM ranks.ranking_scores \
         WHERE block_id = $1",
    )
    .bind(u(BLOCK))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!((rows, min_pos, max_pos), (2_000, 1, 2_000));
    let first: (Uuid, i32) = sqlx::query_as(
        "SELECT entity_id, ballots FROM ranks.ranking_scores WHERE block_id = $1 AND position = 1",
    )
    .bind(u(BLOCK))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(first, (u(FIRST_ENTITY), 1));

    let rank_position = Uuid::parse_str(RANK_POSITION_RELATION_TYPE_ID).unwrap();
    let value_prop = Uuid::parse_str(RANK_POSITION_VALUE_PROPERTY_ID).unwrap();
    let published: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relations r \
         JOIN values v ON v.entity_id = r.entity_id AND v.property_id = $3 \
         WHERE r.from_entity_id = $1 AND r.type_id = $2",
    )
    .bind(u(BLOCK))
    .bind(rank_position)
    .bind(value_prop)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(published, 2_000);
}

/// GEO-3676: recomputes of one block are serialized by a per-block advisory
/// lock, so the live consumer and the hourly backfill cannot interleave (one
/// run's old-config scores landing after another run marked the new config
/// applied). A block id with nothing registered still takes the lock first,
/// which keeps the test free of fixtures.
#[tokio::test]
async fn recompute_waits_for_the_blocks_lock() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let block = Uuid::new_v4();
    let other_block = Uuid::new_v4();

    let held = storage.lock_block_for_recompute(block).await.unwrap();

    // Another block is not held up.
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recompute_block(other_block, BlockMeta::default(), Utc::now(), &storage),
    )
    .await
    .expect("a different block's recompute does not wait")
    .unwrap();

    // The same block waits while the lock is held.
    let waited = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        recompute_block(block, BlockMeta::default(), Utc::now(), &storage),
    )
    .await;
    assert!(
        waited.is_err(),
        "recompute ran while another held the block's lock"
    );

    // Released, it runs.
    held.commit().await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recompute_block(block, BlockMeta::default(), Utc::now(), &storage),
    )
    .await
    .expect("recompute runs once the lock is released")
    .unwrap();
}

/// GEO-3591: an edit that deletes a Rank's vote relations removes those items,
/// so a ranker can clear their ballot (`ranks.update` with no votes is an edit
/// of deletes only), drop one item, or re-submit with deletes and creates in
/// either order. Also a deleted RANK_BLOCK link, and items written before
/// migration 0125 (no relation id) once the backfill fills it in.
///
/// Average mode, so the test also shows a cleared ranker is not a ballot.
#[tokio::test]
async fn deleted_rank_votes_leave_the_ballot_and_a_cleared_ranker_is_no_ballot() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    const SPACE: u128 = 0xE2E5_3591_0001;
    const MEMBERS: [u128; 3] = [0xE2E5_3591_0011, 0xE2E5_3591_0012, 0xE2E5_3591_0013];
    const BLOCK: u128 = 0xE2E5_3591_0021;
    const RANKS: [u128; 3] = [0xE2E5_3591_0041, 0xE2E5_3591_0042, 0xE2E5_3591_0043];
    const A: u128 = 0xE2E5_3591_0031;
    const B: u128 = 0xE2E5_3591_0032;
    const C: u128 = 0xE2E5_3591_0033;
    // Relation ids: 0x3591_0r0v for rank r's vote v; 0x3591_0r50 its block link.
    let vote_rel = |r: usize, v: u128| 0x3591_0000 + ((r as u128 + 1) << 8) + v;
    let link_rel = |r: usize| 0x3591_0000 + ((r as u128 + 1) << 8) + 0x50;

    // --- clean prior runs (idempotent) -------------------------------------
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM values WHERE space_id = $1",
        "DELETE FROM ranks.members WHERE space_id = $1",
        "DELETE FROM spaces WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(SPACE)).execute(pool).await.unwrap();
    }
    let rank_ids: Vec<Uuid> = RANKS.iter().map(|r| u(*r)).collect();
    for sql in [
        "DELETE FROM relations WHERE from_entity_id = ANY($1)",
        "DELETE FROM ranks.ranking_items WHERE ranking_id = ANY($1)",
        "DELETE FROM ranks.rankings WHERE id = ANY($1)",
    ] {
        sqlx::query(sql)
            .bind(&rank_ids[..])
            .execute(pool)
            .await
            .unwrap();
    }
    for sql in [
        "DELETE FROM ranks.ranking_scores WHERE block_id = $1",
        "DELETE FROM ranks.ranking_blocks WHERE id = $1",
        "DELETE FROM ranks.ranking_block_config WHERE block_id = $1",
    ] {
        sqlx::query(sql).bind(u(BLOCK)).execute(pool).await.unwrap();
    }

    // --- a DAO average-mode block with three members -------------------------
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e3591')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    for m in MEMBERS {
        sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
            .bind(u(m))
            .bind(u(SPACE))
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO ranks.ranking_block_config (block_id, max_rows, scoring_mode) \
         VALUES ($1, 2000, 'average')",
    )
    .bind(u(BLOCK))
    .execute(pool)
    .await
    .unwrap();
    let block_edit = EditBuilder::new(gid(0x3591_0001))
        .create_relation(|r| {
            r.id(gid(0x3591_0100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_entity(gid(BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "Clearable (e2e)", None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(SPACE), 1, 0), u(SPACE), &storage)
        .await
        .unwrap();

    const POSITIONS: [&str; 3] = ["a0", "a1", "a2"];
    // Ops for rank `r`'s votes `(relation id, entity)`, in order.
    let votes = |mut edit: EditBuilder<'static>, r: usize, votes: &[(u128, u128)]| {
        for (i, (rel, entity)) in votes.iter().enumerate() {
            let position = POSITIONS[i];
            edit = edit.create_relation(|b| {
                b.id(gid(*rel))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(RANKS[r]))
                    .to(gid(*entity))
                    .to_space(gid(SPACE))
                    .position(position)
            });
        }
        edit
    };
    let deletes = |mut edit: EditBuilder<'static>, rels: &[u128]| {
        for rel in rels {
            edit = edit.delete_relation(gid(*rel));
        }
        edit
    };
    let apply = |r: usize, edit: grc_20::Edit<'static>, block_number: i64| {
        let storage = &storage;
        async move {
            let member = u(MEMBERS[r]);
            apply_detected_edit(&detect(&edit, member, block_number, 0), member, storage)
                .await
                .unwrap();
        }
    };

    // --- three ballots ---------------------------------------------------------
    let ballots: [&[u128]; 3] = [&[A, B, C], &[A, B], &[C, A]];
    for (r, entities) in ballots.iter().enumerate() {
        let edit = EditBuilder::new(gid(RANKS[r] ^ 0xED17))
            .create_relation(|b| {
                b.id(gid(0x3591_0000 + ((r as u128 + 1) << 8) + 0x40))
                    .relation_type(sid(TYPE_RELATION_TYPE_ID))
                    .from(gid(RANKS[r]))
                    .to(sid(RANK_TYPE_ID))
            })
            .create_entity(gid(RANKS[r]), |e| {
                e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
            })
            .create_relation(|b| {
                b.id(gid(link_rel(r)))
                    .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                    .from(gid(RANKS[r]))
                    .to(gid(BLOCK))
            });
        let vs: Vec<(u128, u128)> = entities
            .iter()
            .enumerate()
            .map(|(v, e)| (vote_rel(r, v as u128), *e))
            .collect();
        apply(r, votes(edit, r, &vs).build(), 10 + r as i64).await;
    }

    // entity -> ballots, and how many rankings the projection credits.
    let state = || async {
        let rows: Vec<(Uuid, i32)> = sqlx::query_as(
            "SELECT entity_id, ballots FROM ranks.ranking_scores WHERE block_id = $1 \
             ORDER BY entity_id",
        )
        .bind(u(BLOCK))
        .fetch_all(pool)
        .await
        .unwrap();
        let aggregated = Uuid::parse_str(AGGREGATED_RANKINGS_RELATION_TYPE_ID).unwrap();
        let prov: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM relations WHERE from_entity_id = $1 AND type_id = $2",
        )
        .bind(u(BLOCK))
        .bind(aggregated)
        .fetch_one(pool)
        .await
        .unwrap();
        (rows, prov)
    };
    let items_of = |r: usize| async move {
        sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            "SELECT entity_id, relation_id FROM ranks.ranking_items WHERE ranking_id = $1 \
             ORDER BY entity_id",
        )
        .bind(u(RANKS[r]))
        .fetch_all(pool)
        .await
        .unwrap()
    };

    assert_eq!(
        state().await,
        (vec![(u(A), 3), (u(B), 2), (u(C), 2)], 3),
        "three ballots"
    );
    assert_eq!(
        items_of(0).await,
        vec![
            (u(A), Some(u(vote_rel(0, 0)))),
            (u(B), Some(u(vote_rel(0, 1)))),
            (u(C), Some(u(vote_rel(0, 2)))),
        ],
        "items keep their vote relation ids"
    );
    let block_rel: Option<Uuid> =
        sqlx::query_scalar("SELECT block_relation_id FROM ranks.rankings WHERE id = $1")
            .bind(u(RANKS[0]))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(block_rel, Some(u(link_rel(0))));

    // --- ranker 1 clears their ranking: deletes only (votes + vote entities) ---
    let mut clear = EditBuilder::new(gid(0x3591_0C1E));
    for v in 0..3 {
        clear = clear
            .delete_relation(gid(vote_rel(0, v)))
            .delete_entity(gid(0x3591_0E00 + v));
    }
    apply(0, clear.build(), 20).await;
    assert!(
        items_of(0).await.is_empty(),
        "the cleared ballot has no items"
    );
    assert_eq!(
        state().await,
        (vec![(u(A), 2), (u(B), 1), (u(C), 1)], 2),
        "a cleared ranker is not a ballot and is not credited as a source"
    );

    // --- ranker 2 removes one item (B): a partial delete -----------------------
    apply(
        1,
        deletes(EditBuilder::new(gid(0x3591_0D01)), &[vote_rel(1, 1)]).build(),
        21,
    )
    .await;
    assert_eq!(items_of(1).await, vec![(u(A), Some(u(vote_rel(1, 0))))]);
    assert_eq!(
        state().await,
        (vec![(u(A), 2), (u(C), 1)], 2),
        "B was only on the cleared ballot and the removed item"
    );

    // --- re-submissions: deletes + creates in one edit, both orders ------------
    // Ranker 3, creates first: [C, A] -> [B, C].
    let resubmit = votes(
        EditBuilder::new(gid(0x3591_0D02)),
        2,
        &[(vote_rel(2, 0x10), B), (vote_rel(2, 0x11), C)],
    );
    apply(
        2,
        deletes(resubmit, &[vote_rel(2, 0), vote_rel(2, 1)]).build(),
        22,
    )
    .await;
    assert_eq!(
        items_of(2).await,
        vec![
            (u(B), Some(u(vote_rel(2, 0x10)))),
            (u(C), Some(u(vote_rel(2, 0x11)))),
        ]
    );
    // Ranker 2, deletes first (the SDK's order): [A] -> [C].
    let resubmit = deletes(EditBuilder::new(gid(0x3591_0D03)), &[vote_rel(1, 0)]);
    apply(1, votes(resubmit, 1, &[(vote_rel(1, 0x10), C)]).build(), 23).await;
    assert_eq!(items_of(1).await, vec![(u(C), Some(u(vote_rel(1, 0x10))))]);
    assert_eq!(
        state().await,
        (vec![(u(B), 1), (u(C), 2)], 2),
        "exactly the new ballots"
    );

    // --- a cleared ranker submits again ----------------------------------------
    apply(
        0,
        votes(
            EditBuilder::new(gid(0x3591_0D04)),
            0,
            &[(vote_rel(0, 0x10), A)],
        )
        .build(),
        24,
    )
    .await;
    assert_eq!(state().await, (vec![(u(A), 1), (u(B), 1), (u(C), 2)], 3));

    // --- a deleted RANK_BLOCK link takes ranker 3 out of the block -------------
    apply(
        2,
        deletes(EditBuilder::new(gid(0x3591_0D05)), &[link_rel(2)]).build(),
        25,
    )
    .await;
    assert_eq!(state().await, (vec![(u(A), 1), (u(C), 1)], 2));
    assert_eq!(
        storage.block_id_for_ranking(u(RANKS[2])).await.unwrap(),
        None
    );

    // --- an item from before migration 0125 (no relation id) -------------------
    // Ranker 1's item loses its relation id, as if an older indexer wrote it;
    // the graph still holds the vote relation, so the backfill fills it in and
    // a later delete of that relation is honoured.
    sqlx::query("UPDATE ranks.ranking_items SET relation_id = NULL WHERE ranking_id = $1")
        .bind(u(RANKS[0]))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, \
         to_space_id, space_id) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(u(vote_rel(0, 0x10)))
    .bind(u(0x3591_0E10))
    .bind(Uuid::parse_str(RANK_VOTES_RELATION_TYPE_ID).unwrap())
    .bind(u(RANKS[0]))
    .bind(u(A))
    .bind(u(SPACE))
    .bind(u(MEMBERS[0]))
    .execute(pool)
    .await
    .unwrap();
    let (filled_items, _) = storage.fill_missing_relation_ids().await.unwrap();
    assert!(filled_items >= 1);
    assert_eq!(items_of(0).await, vec![(u(A), Some(u(vote_rel(0, 0x10))))]);
    apply(
        0,
        deletes(EditBuilder::new(gid(0x3591_0D06)), &[vote_rel(0, 0x10)]).build(),
        26,
    )
    .await;
    assert!(items_of(0).await.is_empty());
    assert_eq!(state().await, (vec![(u(C), 1)], 1));
}

/// GEO-3591: the exact edits the web sends, mirrored op for op from geo-sdk
/// 0.20.5. `buildVoteOps` emits, per vote, a RANK_VOTES `CreateRelation` with
/// its own reified vote entity, then a `CreateEntity` of that vote entity with
/// the ordinal value. `updateRank` emits, per existing vote, a `DeleteRelation`
/// of the vote relation then a `DeleteEntity` of its vote entity, followed by
/// `buildVoteOps` for the new votes. It never touches the Rank entity or its
/// RANK_BLOCK link. Clearing a ranking is `ranks.update` with `votes: []`:
/// deletes only.
#[tokio::test]
async fn sdk_shaped_update_rank_clears_and_reranks() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();

    const SPACE: u128 = 0xE2E5_3592_0001;
    const MEMBERS: [u128; 2] = [0xE2E5_3592_0011, 0xE2E5_3592_0012];
    const BLOCK: u128 = 0xE2E5_3592_0021;
    const RANKS: [u128; 2] = [0xE2E5_3592_0041, 0xE2E5_3592_0042];
    const A: u128 = 0xE2E5_3592_0031;
    const B: u128 = 0xE2E5_3592_0032;
    const C: u128 = 0xE2E5_3592_0033;

    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM ranks.members WHERE space_id = $1",
        "DELETE FROM spaces WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(SPACE)).execute(pool).await.unwrap();
    }
    let rank_ids: Vec<Uuid> = RANKS.iter().map(|r| u(*r)).collect();
    for sql in [
        "DELETE FROM ranks.ranking_items WHERE ranking_id = ANY($1)",
        "DELETE FROM ranks.rankings WHERE id = ANY($1)",
    ] {
        sqlx::query(sql)
            .bind(&rank_ids[..])
            .execute(pool)
            .await
            .unwrap();
    }
    for sql in [
        "DELETE FROM ranks.ranking_scores WHERE block_id = $1",
        "DELETE FROM ranks.ranking_blocks WHERE id = $1",
        "DELETE FROM ranks.ranking_block_config WHERE block_id = $1",
    ] {
        sqlx::query(sql).bind(u(BLOCK)).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e3592')")
        .bind(u(SPACE))
        .execute(pool)
        .await
        .unwrap();
    for m in MEMBERS {
        sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
            .bind(u(m))
            .bind(u(SPACE))
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO ranks.ranking_block_config (block_id, max_rows, scoring_mode) \
         VALUES ($1, 2000, 'average')",
    )
    .bind(u(BLOCK))
    .execute(pool)
    .await
    .unwrap();
    let block_edit = EditBuilder::new(gid(0x3592_0001))
        .create_relation(|r| {
            r.id(gid(0x3592_0100))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(BLOCK))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_entity(gid(BLOCK), |e| {
            e.text(sid(NAME_PROPERTY_ID), "SDK-shaped (e2e)", None)
        })
        .build();
    apply_detected_edit(&detect(&block_edit, u(SPACE), 1, 0), u(SPACE), &storage)
        .await
        .unwrap();

    /// One vote as the SDK writes it: the relation id and its reified vote entity id.
    #[derive(Clone, Copy)]
    struct Vote {
        relation: u128,
        entity: u128,
    }
    const POSITIONS: [&str; 3] = ["a0", "a1", "a2"];
    // geo-sdk buildVoteOps (ORDINAL): CreateRelation + CreateEntity per vote.
    let build_vote_ops = |mut edit: EditBuilder<'static>, rank: u128, votes: &[(Vote, u128)]| {
        for (i, (vote, ranked)) in votes.iter().enumerate() {
            let position = POSITIONS[i];
            let vote = *vote;
            edit = edit
                .create_relation(|b| {
                    b.id(gid(vote.relation))
                        .entity(gid(vote.entity))
                        .from(gid(rank))
                        .to(gid(*ranked))
                        .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                        .to_space(gid(SPACE))
                        .position(position)
                })
                .create_entity(gid(vote.entity), |e| {
                    e.text(sid(VOTE_ORDINAL_VALUE_PROPERTY_ID), position, None)
                });
        }
        edit
    };
    // geo-sdk updateRank: DeleteRelation + DeleteEntity per existing vote, then
    // buildVoteOps for the new ones.
    let update_rank = |rank: u128, existing: &[Vote], votes: &[(Vote, u128)]| {
        let mut edit = EditBuilder::new(gid(rank ^ 0x0BDA7E));
        for vote in existing {
            edit = edit
                .delete_relation(gid(vote.relation))
                .delete_entity(gid(vote.entity));
        }
        build_vote_ops(edit, rank, votes).build()
    };
    let vote = |rank: usize, n: u128| Vote {
        relation: 0x3592_0000 + ((rank as u128 + 1) << 12) + n,
        entity: 0x3592_0000 + ((rank as u128 + 1) << 12) + 0x800 + n,
    };
    let apply = |r: usize, edit: grc_20::Edit<'static>, block_number: i64| {
        let storage = &storage;
        async move {
            let member = u(MEMBERS[r]);
            apply_detected_edit(&detect(&edit, member, block_number, 0), member, storage)
                .await
                .unwrap();
        }
    };
    let state = || async {
        let rows: Vec<(Uuid, i32)> = sqlx::query_as(
            "SELECT entity_id, ballots FROM ranks.ranking_scores WHERE block_id = $1 \
             ORDER BY entity_id",
        )
        .bind(u(BLOCK))
        .fetch_all(pool)
        .await
        .unwrap();
        let aggregated = Uuid::parse_str(AGGREGATED_RANKINGS_RELATION_TYPE_ID).unwrap();
        let prov: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM relations WHERE from_entity_id = $1 AND type_id = $2",
        )
        .bind(u(BLOCK))
        .bind(aggregated)
        .fetch_one(pool)
        .await
        .unwrap();
        (rows, prov)
    };

    // --- first submissions: Rank entity + RANK_BLOCK link + SDK vote ops --------
    let first: [Vec<(Vote, u128)>; 2] = [
        vec![(vote(0, 0), A), (vote(0, 1), B), (vote(0, 2), C)],
        vec![(vote(1, 0), A), (vote(1, 1), C)],
    ];
    for (r, votes) in first.iter().enumerate() {
        let edit = EditBuilder::new(gid(RANKS[r] ^ 0xED17))
            .create_entity(gid(RANKS[r]), |e| {
                e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
            })
            .create_relation(|b| {
                b.id(gid(0x3592_0F00 + r as u128))
                    .relation_type(sid(TYPE_RELATION_TYPE_ID))
                    .from(gid(RANKS[r]))
                    .to(sid(RANK_TYPE_ID))
            })
            .create_relation(|b| {
                b.id(gid(0x3592_0F10 + r as u128))
                    .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                    .from(gid(RANKS[r]))
                    .to(gid(BLOCK))
            });
        apply(
            r,
            build_vote_ops(edit, RANKS[r], votes).build(),
            10 + r as i64,
        )
        .await;
    }
    assert_eq!(state().await, (vec![(u(A), 2), (u(B), 1), (u(C), 2)], 2));

    // --- re-rank: ranker 1 goes from [A, B, C] to [C, A] ------------------------
    let existing: Vec<Vote> = first[0].iter().map(|(v, _)| *v).collect();
    let rerank = vec![(vote(0, 0x10), C), (vote(0, 0x11), A)];
    apply(0, update_rank(RANKS[0], &existing, &rerank), 20).await;
    let items: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT entity_id, relation_id FROM ranks.ranking_items WHERE ranking_id = $1 \
         ORDER BY position",
    )
    .bind(u(RANKS[0]))
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        items,
        vec![
            (u(C), Some(u(vote(0, 0x10).relation))),
            (u(A), Some(u(vote(0, 0x11).relation))),
        ],
        "exactly the new ballot"
    );
    assert_eq!(state().await, (vec![(u(A), 2), (u(C), 2)], 2));

    // --- clear: ranks.update with votes: [] (deletes only) ----------------------
    let existing: Vec<Vote> = rerank.iter().map(|(v, _)| *v).collect();
    apply(0, update_rank(RANKS[0], &existing, &[]), 21).await;
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ranks.ranking_items WHERE ranking_id = $1")
            .bind(u(RANKS[0]))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(left, 0);
    // The Rank and its link are untouched; it just isn't a ballot any more.
    assert_eq!(
        storage.block_id_for_ranking(u(RANKS[0])).await.unwrap(),
        Some(u(BLOCK))
    );
    assert_eq!(
        state().await,
        (vec![(u(A), 1), (u(C), 1)], 1),
        "only ranker 2's ballot counts"
    );

    // --- and ranks again from empty: no deletes, just the new votes -------------
    apply(0, update_rank(RANKS[0], &[], &[(vote(0, 0x20), B)]), 22).await;
    assert_eq!(state().await, (vec![(u(A), 1), (u(B), 1), (u(C), 1)], 2));
}
