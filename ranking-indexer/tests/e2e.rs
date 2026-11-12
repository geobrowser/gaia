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

/// One e2e test at a time. Every applied edit drains the shared
/// `ranks.pending_recomputes` queue (GEO-3591), so a test running in parallel
/// would recompute, or wait on, another test's queued blocks: that can hide a
/// broken retry path (the other test rescores the block) or stall a test that
/// holds a block's lock on purpose.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
            "INSERT INTO ranks.ranking_items (ranking_id, rank_space_id, entity_id, space_id, position) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(u(RANK))
        .bind(u(MEMBER_SPACE))
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
        candidates.contains(&(u(RANK), u(MEMBER_SPACE))),
        "find_unregistered_ranks must surface the orphaned rank"
    );

    let recovered = storage
        .get_rank_config_from_kg(u(RANK), u(MEMBER_SPACE))
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

    assert!(storage.recover_ranking(&recovered).await.unwrap());
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
    let _serial = SERIAL.lock().await;
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
        storage
            .block_id_for_ranking(u(RANKS[2]), u(MEMBERS[2]))
            .await
            .unwrap(),
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
    let _serial = SERIAL.lock().await;
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
        storage
            .block_id_for_ranking(u(RANKS[0]), u(MEMBERS[0]))
            .await
            .unwrap(),
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

/// A DAO block in `space` with `members`, clean of any earlier run of the
/// same ids. `mode` is a `ranking_block_config` scoring mode, or `None` for
/// the default (sum). GEO-3591 review tests.
async fn fresh_dao_block(
    storage: &Storage,
    space: u128,
    members: &[u128],
    block: u128,
    ranks: &[u128],
    mode: Option<&str>,
) {
    let pool = storage.pool();
    for sql in [
        "DELETE FROM relations WHERE space_id = $1",
        "DELETE FROM ranks.members WHERE space_id = $1",
        "DELETE FROM spaces WHERE id = $1",
    ] {
        sqlx::query(sql).bind(u(space)).execute(pool).await.unwrap();
    }
    let member_ids: Vec<Uuid> = members.iter().map(|m| u(*m)).collect();
    sqlx::query("DELETE FROM relations WHERE space_id = ANY($1)")
        .bind(&member_ids[..])
        .execute(pool)
        .await
        .unwrap();
    let rank_ids: Vec<Uuid> = ranks.iter().map(|r| u(*r)).collect();
    for sql in [
        "DELETE FROM ranks.ranking_items WHERE ranking_id = ANY($1)",
        "DELETE FROM ranks.rankings WHERE id = ANY($1)",
        "DELETE FROM ranks.deleted_rankings WHERE id = ANY($1)",
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
        "DELETE FROM ranks.pending_recomputes WHERE block_id = $1",
    ] {
        sqlx::query(sql).bind(u(block)).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xe2e3591r')")
        .bind(u(space))
        .execute(pool)
        .await
        .unwrap();
    for m in members {
        sqlx::query("INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)")
            .bind(u(*m))
            .bind(u(space))
            .execute(pool)
            .await
            .unwrap();
    }
    if let Some(mode) = mode {
        sqlx::query(
            "INSERT INTO ranks.ranking_block_config (block_id, max_rows, scoring_mode) \
             VALUES ($1, 2000, $2)",
        )
        .bind(u(block))
        .bind(mode)
        .execute(pool)
        .await
        .unwrap();
    }
    let edit = EditBuilder::new(gid(block ^ 0xB10C))
        .create_relation(|r| {
            r.id(gid(block ^ 0x7E))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(block))
                .to(sid(RANKING_BLOCK_TYPE_ID))
        })
        .create_entity(gid(block), |e| {
            e.text(sid(NAME_PROPERTY_ID), "GEO-3591 review (e2e)", None)
        })
        .build();
    apply_detected_edit(&detect(&edit, u(space), 1, 0), u(space), storage)
        .await
        .unwrap();
}

/// A full ballot edit as the web publishes one: the Rank entity, its TYPES
/// and RANK_BLOCK relations (ids `rel + 0xA0`, `rel + 0xB0`) and one vote per
/// entity (ids `rel + i`).
fn ballot_edit(
    rank: u128,
    block: u128,
    rel: u128,
    space: u128,
    entities: &[u128],
) -> grc_20::Edit<'static> {
    let mut edit = EditBuilder::new(gid(rel ^ 0xED17))
        .create_relation(|b| {
            b.id(gid(rel + 0xA0))
                .relation_type(sid(TYPE_RELATION_TYPE_ID))
                .from(gid(rank))
                .to(sid(RANK_TYPE_ID))
        })
        .create_entity(gid(rank), |e| {
            e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
        })
        .create_relation(|b| {
            b.id(gid(rel + 0xB0))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(rank))
                .to(gid(block))
        });
    const POSITIONS: [&str; 4] = ["a0", "a1", "a2", "a3"];
    for (i, entity) in entities.iter().enumerate() {
        let position = POSITIONS[i];
        edit = edit.create_relation(|b| {
            b.id(gid(rel + i as u128))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(rank))
                .to(gid(*entity))
                .to_space(gid(space))
                .position(position)
        });
    }
    edit.build()
}

async fn apply_in(storage: &Storage, space: u128, edit: &grc_20::Edit<'static>, block_number: i64) {
    apply_detected_edit(&detect(edit, u(space), block_number, 0), u(space), storage)
        .await
        .unwrap();
}

/// `(entity, ballots)` of a block's scores, by entity id.
async fn score_rows(storage: &Storage, block: u128) -> Vec<(Uuid, i32)> {
    sqlx::query_as(
        "SELECT entity_id, ballots FROM ranks.ranking_scores WHERE block_id = $1 ORDER BY entity_id",
    )
    .bind(u(block))
    .fetch_all(storage.pool())
    .await
    .unwrap()
}

/// GEO-3591 review: deletes and vote creates are scoped to the Rank's own
/// space, as the kg-indexer scopes them. An edit in another space that
/// deletes a ranker's votes, link or Rank, or creates votes on their Rank,
/// changes nothing. Also: the relation-id fill takes only a relation in the
/// Rank's space, and the lowest id when several match.
#[tokio::test]
async fn rank_edits_from_another_space_change_nothing() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();
    const SPACE: u128 = 0xE2E5_3593_0001;
    const VICTIM: u128 = 0xE2E5_3593_0011;
    const OTHER: u128 = 0xE2E5_3593_0012;
    const BLOCK: u128 = 0xE2E5_3593_0021;
    const RANK: u128 = 0xE2E5_3593_0041;
    const A: u128 = 0xE2E5_3593_0031;
    const B: u128 = 0xE2E5_3593_0032;
    const REL: u128 = 0x3593_1000;
    fresh_dao_block(&storage, SPACE, &[VICTIM, OTHER], BLOCK, &[RANK], None).await;
    apply_in(
        &storage,
        VICTIM,
        &ballot_edit(RANK, BLOCK, REL, SPACE, &[A, B]),
        10,
    )
    .await;
    let before = score_rows(&storage, BLOCK).await;
    assert_eq!(before.len(), 2);

    // From OTHER's space: delete both votes, the link and the Rank, and
    // create a vote on the Rank.
    let attack = EditBuilder::new(gid(0x3593_0A77))
        .delete_relation(gid(REL))
        .delete_relation(gid(REL + 1))
        .delete_relation(gid(REL + 0xB0))
        .delete_entity(gid(RANK))
        .create_relation(|b| {
            b.id(gid(REL + 0x50))
                .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(B))
                .to_space(gid(SPACE))
                .position("a0")
        })
        .build();
    apply_in(&storage, OTHER, &attack, 20).await;

    let items: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT entity_id, relation_id FROM ranks.ranking_items \
         WHERE ranking_id = $1 AND rank_space_id = $2 ORDER BY entity_id",
    )
    .bind(u(RANK))
    .bind(u(VICTIM))
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        items,
        vec![(u(A), Some(u(REL))), (u(B), Some(u(REL + 1)))],
        "the victim's ballot is untouched"
    );
    assert_eq!(
        storage
            .block_id_for_ranking(u(RANK), u(VICTIM))
            .await
            .unwrap(),
        Some(u(BLOCK)),
        "still linked"
    );
    let tombstoned: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ranks.deleted_rankings WHERE id = $1")
            .bind(u(RANK))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(tombstoned, 0);
    assert_eq!(score_rows(&storage, BLOCK).await, before);

    // The fill: A's item loses its relation id. The graph holds a matching
    // relation authored in OTHER's space (lower id) and two in the victim's
    // space; the lower of the victim's two is taken.
    sqlx::query("UPDATE ranks.ranking_items SET relation_id = NULL WHERE ranking_id = $1")
        .bind(u(RANK))
        .execute(pool)
        .await
        .unwrap();
    let votes_type = Uuid::parse_str(RANK_VOTES_RELATION_TYPE_ID).unwrap();
    for (rel, space) in [
        (REL + 0x60, OTHER),
        (REL + 0x72, VICTIM),
        (REL + 0x71, VICTIM),
    ] {
        sqlx::query(
            "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, \
             to_space_id, space_id) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(u(rel))
        .bind(u(rel ^ 0xE))
        .bind(votes_type)
        .bind(u(RANK))
        .bind(u(A))
        .bind(u(SPACE))
        .bind(u(space))
        .execute(pool)
        .await
        .unwrap();
    }
    storage.fill_missing_relation_ids().await.unwrap();
    let a_rel: Option<Uuid> = sqlx::query_scalar(
        "SELECT relation_id FROM ranks.ranking_items WHERE ranking_id = $1 AND entity_id = $2",
    )
    .bind(u(RANK))
    .bind(u(A))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(a_rel, Some(u(REL + 0x71)));
}

/// GEO-3591 review, on a default (sum) block:
/// - a cleared ranking supersedes the same ranker's older Rank (empty ballots
///   are dropped after dedup, as 0124's `ranking_member_ballots` does);
/// - deleting the cleared Rank entity lets the older one count again, and the
///   delete sticks: the backfill does not register the Rank again although
///   its TYPES relation is still in the graph;
/// - a Rank recovered from the graph carries its RANK_BLOCK relation id.
#[tokio::test]
async fn deleted_rank_stays_deleted_and_a_cleared_rank_supersedes_an_older_one() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();
    const SPACE: u128 = 0xE2E5_3594_0001;
    const M1: u128 = 0xE2E5_3594_0011;
    const M2: u128 = 0xE2E5_3594_0012;
    const BLOCK: u128 = 0xE2E5_3594_0021;
    const OLD: u128 = 0xE2E5_3594_0041;
    const NEW: u128 = 0xE2E5_3594_0042;
    const OTHER_RANK: u128 = 0xE2E5_3594_0043;
    const LOST: u128 = 0xE2E5_3594_0044;
    const A: u128 = 0xE2E5_3594_0031;
    const B: u128 = 0xE2E5_3594_0032;
    const C: u128 = 0xE2E5_3594_0033;
    fresh_dao_block(
        &storage,
        SPACE,
        &[M1, M2],
        BLOCK,
        &[OLD, NEW, OTHER_RANK, LOST],
        None,
    )
    .await;

    // M1 ranks [A] in OLD, then [B] in NEW; M2 ranks [C].
    apply_in(
        &storage,
        M1,
        &ballot_edit(OLD, BLOCK, 0x3594_1000, SPACE, &[A]),
        10,
    )
    .await;
    apply_in(
        &storage,
        M1,
        &ballot_edit(NEW, BLOCK, 0x3594_2000, SPACE, &[B]),
        11,
    )
    .await;
    apply_in(
        &storage,
        M2,
        &ballot_edit(OTHER_RANK, BLOCK, 0x3594_3000, SPACE, &[C]),
        12,
    )
    .await;
    assert_eq!(
        score_rows(&storage, BLOCK).await,
        vec![(u(B), 1), (u(C), 1)],
        "NEW supersedes OLD"
    );

    // M1 clears NEW. It is still M1's latest Rank, so OLD does not come back.
    let clear = EditBuilder::new(gid(0x3594_0C1E))
        .delete_relation(gid(0x3594_2000))
        .build();
    apply_in(&storage, M1, &clear, 20).await;
    assert_eq!(
        score_rows(&storage, BLOCK).await,
        vec![(u(C), 1)],
        "a cleared ranking still supersedes the same ranker's older one"
    );

    // M1 deletes the NEW Rank entity: it is gone, and OLD counts again.
    let delete_rank = EditBuilder::new(gid(0x3594_0DE1))
        .delete_entity(gid(NEW))
        .build();
    apply_in(&storage, M1, &delete_rank, 21).await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM ranks.rankings WHERE id = $1")
        .bind(u(NEW))
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(
        score_rows(&storage, BLOCK).await,
        vec![(u(A), 1), (u(C), 1)]
    );

    // The kg-indexer keeps NEW's TYPES and RANK_BLOCK relations (it does not
    // apply DeleteEntity). The backfill must not register NEW again.
    let pid = |s: &str| Uuid::parse_str(s).unwrap();
    for (rel, ty, to) in [
        (
            0x3594_20A0u128,
            pid(TYPE_RELATION_TYPE_ID),
            pid(RANK_TYPE_ID),
        ),
        (0x3594_20B0, pid(RANK_BLOCK_RELATION_TYPE_ID), u(BLOCK)),
    ] {
        sqlx::query(
            "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(u(rel))
        .bind(u(rel ^ 0xE))
        .bind(ty)
        .bind(u(NEW))
        .bind(to)
        .bind(u(M1))
        .execute(pool)
        .await
        .unwrap();
    }
    assert!(!storage
        .find_unregistered_ranks()
        .await
        .unwrap()
        .contains(&(u(NEW), u(M1))));

    // A Rank that was never registered (LOST) is recovered with its link's
    // relation id, so a later delete of the link unlinks it.
    for (rel, ty, to) in [
        (
            0x3594_40A0u128,
            pid(TYPE_RELATION_TYPE_ID),
            pid(RANK_TYPE_ID),
        ),
        (0x3594_40B0, pid(RANK_BLOCK_RELATION_TYPE_ID), u(BLOCK)),
    ] {
        sqlx::query(
            "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(u(rel))
        .bind(u(rel ^ 0xE))
        .bind(ty)
        .bind(u(LOST))
        .bind(to)
        .bind(u(M2))
        .execute(pool)
        .await
        .unwrap();
    }
    assert!(storage
        .find_unregistered_ranks()
        .await
        .unwrap()
        .contains(&(u(LOST), u(M2))));
    let recovered = storage
        .get_rank_config_from_kg(u(LOST), u(M2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.block_relation_id, Some(u(0x3594_40B0)));
    assert!(storage.recover_ranking(&recovered).await.unwrap());
    let unlink = EditBuilder::new(gid(0x3594_0D11))
        .delete_relation(gid(0x3594_40B0))
        .build();
    apply_in(&storage, M2, &unlink, 22).await;
    assert_eq!(
        storage.block_id_for_ranking(u(LOST), u(M2)).await.unwrap(),
        None
    );

    // Creating NEW again undoes its delete.
    apply_in(
        &storage,
        M1,
        &ballot_edit(NEW, BLOCK, 0x3594_5000, SPACE, &[B]),
        30,
    )
    .await;
    assert_eq!(
        score_rows(&storage, BLOCK).await,
        vec![(u(B), 1), (u(C), 1)]
    );
    let tombstoned: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ranks.deleted_rankings WHERE id = $1")
            .bind(u(NEW))
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(tombstoned, 0);
}

/// GEO-3591 review: a deletes-only edit whose recompute fails (here: the
/// block's lock is held, and the attempt is abandoned) is rescored when the
/// same edit is redelivered, although the retry's deletes match nothing.
#[tokio::test]
async fn a_retried_clear_still_rescores_the_block() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    const SPACE: u128 = 0xE2E5_3595_0001;
    const M1: u128 = 0xE2E5_3595_0011;
    const M2: u128 = 0xE2E5_3595_0012;
    const BLOCK: u128 = 0xE2E5_3595_0021;
    const R1: u128 = 0xE2E5_3595_0041;
    const R2: u128 = 0xE2E5_3595_0042;
    const A: u128 = 0xE2E5_3595_0031;
    const B: u128 = 0xE2E5_3595_0032;
    fresh_dao_block(&storage, SPACE, &[M1, M2], BLOCK, &[R1, R2], None).await;
    apply_in(
        &storage,
        M1,
        &ballot_edit(R1, BLOCK, 0x3595_1000, SPACE, &[A, B]),
        10,
    )
    .await;
    apply_in(
        &storage,
        M2,
        &ballot_edit(R2, BLOCK, 0x3595_2000, SPACE, &[B]),
        11,
    )
    .await;
    assert_eq!(
        score_rows(&storage, BLOCK).await,
        vec![(u(A), 1), (u(B), 2)]
    );

    let clear = EditBuilder::new(gid(0x3595_0C1E))
        .delete_relation(gid(0x3595_1000))
        .delete_entity(gid(0x3595_1E00))
        .delete_relation(gid(0x3595_1001))
        .delete_entity(gid(0x3595_1E01))
        .build();
    let detected = detect(&clear, u(M1), 20, 0);

    // First delivery: the deletes commit, the recompute cannot get the lock.
    let held = storage.lock_block_for_recompute(u(BLOCK)).await.unwrap();
    let first = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        apply_detected_edit(&detected, u(M1), &storage),
    )
    .await;
    assert!(first.is_err(), "the first recompute was blocked");
    held.commit().await.unwrap();
    assert_eq!(
        score_rows(&storage, BLOCK).await,
        vec![(u(A), 1), (u(B), 2)],
        "not rescored yet"
    );

    // Redelivery: the deletes find nothing, the queued block is rescored.
    apply_detected_edit(&detected, u(M1), &storage)
        .await
        .unwrap();
    assert_eq!(score_rows(&storage, BLOCK).await, vec![(u(B), 1)]);
    // The drain is not this edit's own work, so what it mints is stamped with
    // the chain cursor, not with this edit's block (20).
    let (prov, _) = ranking_indexer::publish::provenance_ids(u(BLOCK), u(R2));
    let stamped: String = sqlx::query_scalar("SELECT updated_at_block FROM entities WHERE id = $1")
        .bind(prov)
        .fetch_one(storage.pool())
        .await
        .unwrap();
    let chain = storage.current_chain_meta().await.unwrap().number;
    assert_ne!(chain, 20, "precondition: the cursor differs from the edit");
    assert_eq!(stamped, chain.to_string());
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ranks.pending_recomputes WHERE block_id = $1")
            .bind(u(BLOCK))
            .fetch_one(storage.pool())
            .await
            .unwrap();
    assert_eq!(queued, 0, "the recompute cleared the queue");
}

/// Queue entries of a block in `ranks.pending_recomputes`, with attempts.
async fn queued(storage: &Storage, block: u128) -> Vec<(i64, i32)> {
    sqlx::query_as(
        "SELECT seq, attempts FROM ranks.pending_recomputes WHERE block_id = $1 ORDER BY seq",
    )
    .bind(u(block))
    .fetch_all(storage.pool())
    .await
    .unwrap()
}

/// GEO-3591 review 2 + Patrick's review: a queued block whose recompute keeps
/// failing does not fail unrelated edits. The edit's own block is rescored
/// first; the failing block is tried best effort, counted, and after
/// `LIVE_DRAIN_MAX_ATTEMPTS` failures left to the backfill. The backfill's
/// queue pass keeps trying it until `BACKFILL_DEAD_AFTER`, then marks it dead
/// and stops (so `backfill_blocks` does not fail every hour); resetting the
/// entry re-enqueues it. The failure is a trigger that rejects the block's
/// applied-config update.
#[tokio::test]
async fn a_failing_queued_block_does_not_fail_unrelated_deletes() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();
    const SPACE: u128 = 0xE2E5_3596_0001;
    const STUCK_SPACE: u128 = 0xE2E5_3596_0002;
    const M1: u128 = 0xE2E5_3596_0011;
    const M2: u128 = 0xE2E5_3596_0012;
    const BLOCK: u128 = 0xE2E5_3596_0021;
    const STUCK: u128 = 0xE2E5_3596_0022;
    const R1: u128 = 0xE2E5_3596_0041;
    const R2: u128 = 0xE2E5_3596_0042;
    const A: u128 = 0xE2E5_3596_0031;
    const B: u128 = 0xE2E5_3596_0032;
    use ranking_indexer::recompute::{
        drain_pending_recomputes, DrainMode, BACKFILL_DEAD_AFTER, LIVE_DRAIN_MAX_ATTEMPTS,
    };
    // A run that failed half way leaves its trigger behind.
    sqlx::query("DROP TRIGGER IF EXISTS e2e_fail_3596 ON ranks.ranking_block_config")
        .execute(pool)
        .await
        .unwrap();
    fresh_dao_block(&storage, SPACE, &[M1, M2], BLOCK, &[R1, R2], None).await;
    fresh_dao_block(&storage, STUCK_SPACE, &[], STUCK, &[], Some("sum")).await;
    apply_in(
        &storage,
        M1,
        &ballot_edit(R1, BLOCK, 0x3596_1000, SPACE, &[A, B]),
        10,
    )
    .await;
    apply_in(
        &storage,
        M2,
        &ballot_edit(R2, BLOCK, 0x3596_2000, SPACE, &[B]),
        11,
    )
    .await;

    // STUCK's recompute fails: its applied-config update is rejected. (Its
    // config is marked unapplied first, so every recompute tries to update it.)
    sqlx::query(
        "UPDATE ranks.ranking_block_config SET applied_max_rows = NULL, applied_scoring_mode = NULL \
         WHERE block_id = $1",
    )
    .bind(u(STUCK))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION ranks.e2e_fail_3596() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.block_id = '{}' THEN RAISE EXCEPTION 'e2e: stuck block'; END IF; RETURN NEW; END $$",
        u(STUCK)
    )))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("DROP TRIGGER IF EXISTS e2e_fail_3596 ON ranks.ranking_block_config")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER e2e_fail_3596 BEFORE UPDATE ON ranks.ranking_block_config \
         FOR EACH ROW EXECUTE FUNCTION ranks.e2e_fail_3596()",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE ranks.ranking_block_config SET updated_at = now() WHERE block_id = $1")
        .bind(u(STUCK))
        .execute(pool)
        .await
        .expect_err("the trigger rejects STUCK's config updates");
    sqlx::query("INSERT INTO ranks.pending_recomputes (block_id) VALUES ($1)")
        .bind(u(STUCK))
        .execute(pool)
        .await
        .unwrap();

    // M1 clears their ballot: the edit succeeds and its own block is rescored.
    let clear = EditBuilder::new(gid(0x3596_0C1E))
        .delete_relation(gid(0x3596_1000))
        .delete_relation(gid(0x3596_1001))
        .build();
    apply_detected_edit(&detect(&clear, u(M1), 20, 0), u(M1), &storage)
        .await
        .expect("a failing queued block must not fail this edit");
    assert_eq!(score_rows(&storage, BLOCK).await, vec![(u(B), 1)]);
    assert_eq!(queued(&storage, STUCK).await[0].1, 1, "one failed attempt");

    // Unrelated edits keep trying STUCK until the cap...
    for n in 0..4u128 {
        let unrelated = EditBuilder::new(gid(0x3596_0E00 + n))
            .delete_relation(gid(0x3596_0F00 + n))
            .build();
        apply_detected_edit(
            &detect(&unrelated, u(M2), 30 + n as i64, 0),
            u(M2),
            &storage,
        )
        .await
        .unwrap();
    }
    // ...then leave it alone.
    assert_eq!(queued(&storage, STUCK).await[0].1, LIVE_DRAIN_MAX_ATTEMPTS);

    // The backfill's queue pass keeps trying it, until it is marked dead...
    let none = std::collections::HashSet::new();
    let mut runs = 0;
    while !sqlx::query_scalar::<_, Option<bool>>(
        "SELECT bool_or(dead_at IS NOT NULL) FROM ranks.pending_recomputes WHERE block_id = $1",
    )
    .bind(u(STUCK))
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or(false)
    {
        let (_, failed) =
            drain_pending_recomputes(&storage, Utc::now(), DrainMode::Backfill, &none).await;
        assert!(failed >= 1);
        runs += 1;
        assert!(runs <= BACKFILL_DEAD_AFTER, "never marked dead");
    }
    assert_eq!(queued(&storage, STUCK).await[0].1, BACKFILL_DEAD_AFTER);
    // ...after which it is skipped: no failure, nothing recomputed.
    let blocks = storage.pending_recompute_blocks(None).await.unwrap();
    assert!(!blocks.contains(&u(STUCK)), "a dead entry is not drained");
    assert!(storage.dead_pending_recompute_blocks().await.unwrap() >= 1);

    // Fixed and reset, it is recomputed and leaves the queue.
    sqlx::query("DROP TRIGGER e2e_fail_3596 ON ranks.ranking_block_config")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION ranks.e2e_fail_3596()")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ranks.pending_recomputes SET dead_at = NULL, attempts = 0 WHERE block_id = $1",
    )
    .bind(u(STUCK))
    .execute(pool)
    .await
    .unwrap();
    let (recomputed, failed) =
        drain_pending_recomputes(&storage, Utc::now(), DrainMode::Backfill, &none).await;
    assert_eq!(failed, 0);
    assert!(recomputed >= 1);
    assert!(queued(&storage, STUCK).await.is_empty());
}

/// Patrick's review: the drain only tries a queued block's lock. A block
/// another recompute holds is skipped (still queued, no failure counted), so a
/// live edit never waits behind it.
#[tokio::test]
async fn the_drain_skips_a_block_another_recompute_holds() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    const SPACE: u128 = 0xE2E5_359B_0001;
    const BUSY_SPACE: u128 = 0xE2E5_359B_0002;
    const M1: u128 = 0xE2E5_359B_0011;
    const BLOCK: u128 = 0xE2E5_359B_0021;
    const BUSY: u128 = 0xE2E5_359B_0022;
    const RANK: u128 = 0xE2E5_359B_0041;
    const A: u128 = 0xE2E5_359B_0031;
    fresh_dao_block(&storage, SPACE, &[M1], BLOCK, &[RANK], None).await;
    fresh_dao_block(&storage, BUSY_SPACE, &[], BUSY, &[], None).await;
    sqlx::query("INSERT INTO ranks.pending_recomputes (block_id) VALUES ($1)")
        .bind(u(BUSY))
        .execute(storage.pool())
        .await
        .unwrap();
    let held = storage.lock_block_for_recompute(u(BUSY)).await.unwrap();

    let edit = ballot_edit(RANK, BLOCK, 0x359B_1000, SPACE, &[A]);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        apply_detected_edit(&detect(&edit, u(M1), 10, 0), u(M1), &storage),
    )
    .await
    .expect("the edit does not wait on a block it did not touch")
    .unwrap();
    assert_eq!(score_rows(&storage, BLOCK).await, vec![(u(A), 1)]);
    assert_eq!(queued(&storage, BUSY).await.len(), 1, "still queued");
    assert_eq!(
        queued(&storage, BUSY).await[0].1,
        0,
        "a skip is not a failure"
    );
    held.commit().await.unwrap();
    sqlx::query("DELETE FROM ranks.pending_recomputes WHERE block_id = $1")
        .bind(u(BUSY))
        .execute(storage.pool())
        .await
        .unwrap();
}

/// GEO-3591 review 2: a recompute clears only the queue entries it saw before
/// reading its inputs. An entry committed after that (whatever its sequence
/// number) stays queued.
#[tokio::test]
async fn a_recompute_clears_only_the_queue_entries_it_saw() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    let block = Uuid::new_v4();
    let insert = || async {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO ranks.pending_recomputes (block_id) VALUES ($1) RETURNING seq",
        )
        .bind(block)
        .fetch_one(storage.pool())
        .await
        .unwrap()
    };
    let before = insert().await;
    let mut lock = storage.lock_block_for_recompute(block).await.unwrap();
    let seen = storage
        .pending_recompute_seen(&mut lock, block)
        .await
        .unwrap();
    assert_eq!(seen, vec![before]);
    let after = insert().await;
    storage
        .clear_pending_recompute(&mut lock, &seen)
        .await
        .unwrap();
    lock.commit().await.unwrap();
    let left: Vec<i64> =
        sqlx::query_scalar("SELECT seq FROM ranks.pending_recomputes WHERE block_id = $1")
            .bind(block)
            .fetch_all(storage.pool())
            .await
            .unwrap();
    assert_eq!(left, vec![after]);
    sqlx::query("DELETE FROM ranks.pending_recomputes WHERE block_id = $1")
        .bind(block)
        .execute(storage.pool())
        .await
        .unwrap();
}

/// GEO-3591 review 2: a Rank deleted before it has a row is tombstoned too:
/// one created and deleted (with its link) in one edit, and one deleted while
/// still unregistered. A delete from another space tombstones nothing for
/// the Rank's own space.
#[tokio::test]
async fn a_rank_deleted_before_it_has_a_row_is_not_recovered() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();
    const SPACE: u128 = 0xE2E5_3597_0001;
    const M1: u128 = 0xE2E5_3597_0011;
    const M2: u128 = 0xE2E5_3597_0012;
    const BLOCK: u128 = 0xE2E5_3597_0021;
    const SAME_EDIT: u128 = 0xE2E5_3597_0041;
    const UNREGISTERED: u128 = 0xE2E5_3597_0042;
    const OTHERS: u128 = 0xE2E5_3597_0043;
    const A: u128 = 0xE2E5_3597_0031;
    fresh_dao_block(
        &storage,
        SPACE,
        &[M1, M2],
        BLOCK,
        &[SAME_EDIT, UNREGISTERED, OTHERS],
        None,
    )
    .await;
    let types_rel = |rel: u128, rank: u128, space: u128| async move {
        sqlx::query(
            "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(u(rel))
        .bind(u(rel ^ 0xE))
        .bind(Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap())
        .bind(u(rank))
        .bind(Uuid::parse_str(RANK_TYPE_ID).unwrap())
        .bind(u(space))
        .execute(pool)
        .await
        .unwrap();
    };

    // Created, linked, voted and deleted in one edit. The kg-indexer keeps the
    // TYPES relation; it may not have indexed it yet when we tombstone.
    let mut edit = ballot_edit(SAME_EDIT, BLOCK, 0x3597_1000, SPACE, &[A]);
    edit.ops
        .push(grc_20::Op::DeleteEntity(grc_20::model::op::DeleteEntity {
            id: gid(SAME_EDIT),
            context: None,
        }));
    apply_in(&storage, M1, &edit, 10).await;
    types_rel(0x3597_10A0, SAME_EDIT, M1).await;

    // Typed in the graph (in M1) but never registered, then deleted by M1.
    types_rel(0x3597_20A0, UNREGISTERED, M1).await;
    // Typed in M2 but "deleted" from M1: not M1's to delete.
    types_rel(0x3597_30A0, OTHERS, M2).await;
    let delete = EditBuilder::new(gid(0x3597_0DE1))
        .delete_entity(gid(UNREGISTERED))
        .delete_entity(gid(OTHERS))
        .build();
    apply_in(&storage, M1, &delete, 11).await;

    let candidates = storage.find_unregistered_ranks().await.unwrap();
    assert!(
        !candidates.contains(&(u(SAME_EDIT), u(M1))),
        "created and deleted in one edit"
    );
    assert!(
        !candidates.contains(&(u(UNREGISTERED), u(M1))),
        "deleted while unregistered"
    );
    assert!(
        candidates.contains(&(u(OTHERS), u(M2))),
        "a delete from another space is ignored"
    );
}

/// GEO-3591 review 2 (base gap): a Rank moving to another block by a new
/// RANK_BLOCK link leaves the old block, which is rescored.
#[tokio::test]
async fn a_rank_moved_to_another_block_leaves_the_old_one() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    const SPACE: u128 = 0xE2E5_3598_0001;
    const SPACE2: u128 = 0xE2E5_3598_0002;
    const M1: u128 = 0xE2E5_3598_0011;
    const OLD_BLOCK: u128 = 0xE2E5_3598_0021;
    const NEW_BLOCK: u128 = 0xE2E5_3598_0022;
    const RANK: u128 = 0xE2E5_3598_0041;
    const A: u128 = 0xE2E5_3598_0031;
    fresh_dao_block(&storage, SPACE, &[M1], OLD_BLOCK, &[RANK], None).await;
    fresh_dao_block(&storage, SPACE2, &[M1], NEW_BLOCK, &[], None).await;
    apply_in(
        &storage,
        M1,
        &ballot_edit(RANK, OLD_BLOCK, 0x3598_1000, SPACE, &[A]),
        10,
    )
    .await;
    assert_eq!(score_rows(&storage, OLD_BLOCK).await, vec![(u(A), 1)]);

    let relink = EditBuilder::new(gid(0x3598_0E11))
        .create_relation(|b| {
            b.id(gid(0x3598_10B1))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(NEW_BLOCK))
        })
        .build();
    apply_in(&storage, M1, &relink, 11).await;
    assert!(
        score_rows(&storage, OLD_BLOCK).await.is_empty(),
        "left the old block"
    );
    assert_eq!(score_rows(&storage, NEW_BLOCK).await, vec![(u(A), 1)]);
    assert!(queued(&storage, OLD_BLOCK).await.is_empty());
}

/// GEO-3591 review 2: the blocks an edit's own deletes change are rescored by
/// that edit, even when the queue would not offer them (here: the block has
/// an old entry that the live consumer gave up on).
#[tokio::test]
async fn an_edits_own_deletes_are_rescored_even_when_the_queue_gave_up() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    const SPACE: u128 = 0xE2E5_3599_0001;
    const M1: u128 = 0xE2E5_3599_0011;
    const BLOCK: u128 = 0xE2E5_3599_0021;
    const RANK: u128 = 0xE2E5_3599_0041;
    const A: u128 = 0xE2E5_3599_0031;
    fresh_dao_block(&storage, SPACE, &[M1], BLOCK, &[RANK], None).await;
    apply_in(
        &storage,
        M1,
        &ballot_edit(RANK, BLOCK, 0x3599_1000, SPACE, &[A]),
        10,
    )
    .await;
    sqlx::query("INSERT INTO ranks.pending_recomputes (block_id, attempts) VALUES ($1, 99)")
        .bind(u(BLOCK))
        .execute(storage.pool())
        .await
        .unwrap();
    let clear = EditBuilder::new(gid(0x3599_0C1E))
        .delete_relation(gid(0x3599_1000))
        .build();
    apply_in(&storage, M1, &clear, 20).await;
    assert!(score_rows(&storage, BLOCK).await.is_empty());
}

/// GEO-3591 review 3: a move-only edit (a new RANK_BLOCK link) whose recompute
/// of the old block fails is redelivered; the redelivery sees no move, but the
/// old block queued the first time is still rescored.
#[tokio::test]
async fn a_retried_move_still_rescores_the_old_block() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    const SPACE: u128 = 0xE2E5_359A_0001;
    const SPACE2: u128 = 0xE2E5_359A_0002;
    const M1: u128 = 0xE2E5_359A_0011;
    const OLD_BLOCK: u128 = 0xE2E5_359A_0021;
    const NEW_BLOCK: u128 = 0xE2E5_359A_0022;
    const RANK: u128 = 0xE2E5_359A_0041;
    const A: u128 = 0xE2E5_359A_0031;
    fresh_dao_block(&storage, SPACE, &[M1], OLD_BLOCK, &[RANK], None).await;
    fresh_dao_block(&storage, SPACE2, &[M1], NEW_BLOCK, &[], None).await;
    apply_in(
        &storage,
        M1,
        &ballot_edit(RANK, OLD_BLOCK, 0x359A_1000, SPACE, &[A]),
        10,
    )
    .await;
    assert_eq!(score_rows(&storage, OLD_BLOCK).await, vec![(u(A), 1)]);

    let relink = EditBuilder::new(gid(0x359A_0E11))
        .create_relation(|b| {
            b.id(gid(0x359A_10B1))
                .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                .from(gid(RANK))
                .to(gid(NEW_BLOCK))
        })
        .build();
    let detected = detect(&relink, u(M1), 11, 0);

    // First delivery: the link moves and the old block is queued, but its
    // recompute cannot get the lock and the attempt is abandoned.
    let held = storage
        .lock_block_for_recompute(u(OLD_BLOCK))
        .await
        .unwrap();
    let first = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        apply_detected_edit(&detected, u(M1), &storage),
    )
    .await;
    assert!(first.is_err(), "the old block's recompute was blocked");
    held.commit().await.unwrap();
    assert_eq!(score_rows(&storage, OLD_BLOCK).await, vec![(u(A), 1)]);

    // Redelivery: no move is seen any more, the queue still rescores it.
    apply_detected_edit(&detected, u(M1), &storage)
        .await
        .unwrap();
    assert!(score_rows(&storage, OLD_BLOCK).await.is_empty());
    assert_eq!(score_rows(&storage, NEW_BLOCK).await, vec![(u(A), 1)]);
    assert!(queued(&storage, OLD_BLOCK).await.is_empty());
}

/// Patrick's review: the same Rank id registered from another space is that
/// space's own ballot. It can't replace, delete or unlink the original's
/// items, and the original's delete sticks: recovery works on (id, space)
/// pairs and never over a tombstone.
#[tokio::test]
async fn a_rank_id_registered_from_another_space_is_a_separate_ballot() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();
    const SPACE: u128 = 0xE2E5_359C_0001;
    const VICTIM: u128 = 0xE2E5_359C_0011;
    const OTHER: u128 = 0xE2E5_359C_0012;
    const BLOCK: u128 = 0xE2E5_359C_0021;
    const R: u128 = 0xE2E5_359C_0041;
    const A: u128 = 0xE2E5_359C_0031;
    const B: u128 = 0xE2E5_359C_0032;
    const C: u128 = 0xE2E5_359C_0033;
    fresh_dao_block(&storage, SPACE, &[VICTIM, OTHER], BLOCK, &[R], None).await;
    apply_in(
        &storage,
        VICTIM,
        &ballot_edit(R, BLOCK, 0x359C_1000, SPACE, &[A, B]),
        10,
    )
    .await;

    // OTHER registers R in its own space, links it, votes [C], and deletes the
    // victim's vote and link relations from there.
    apply_in(
        &storage,
        OTHER,
        &ballot_edit(R, BLOCK, 0x359C_2000, SPACE, &[C]),
        11,
    )
    .await;
    let attack = EditBuilder::new(gid(0x359C_0A77))
        .delete_relation(gid(0x359C_1000))
        .delete_relation(gid(0x359C_1001))
        .delete_relation(gid(0x359C_10B0))
        .build();
    apply_in(&storage, OTHER, &attack, 12).await;

    let items_of = |space: u128| async move {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT entity_id FROM ranks.ranking_items WHERE ranking_id = $1 AND rank_space_id = $2 \
             ORDER BY entity_id",
        )
        .bind(u(R))
        .bind(u(space))
        .fetch_all(pool)
        .await
        .unwrap()
    };
    assert_eq!(
        items_of(VICTIM).await,
        vec![u(A), u(B)],
        "the victim's items are untouched"
    );
    assert_eq!(items_of(OTHER).await, vec![u(C)], "OTHER's own ballot");
    assert_eq!(
        storage.block_id_for_ranking(u(R), u(VICTIM)).await.unwrap(),
        Some(u(BLOCK))
    );
    // Two ballots, one per space (and one provenance relation for the id).
    assert_eq!(
        score_rows(&storage, BLOCK).await,
        vec![(u(A), 1), (u(B), 1), (u(C), 1)]
    );

    // The victim deletes R. OTHER's ballot stays; the victim's is gone.
    let delete = EditBuilder::new(gid(0x359C_0DE1))
        .delete_entity(gid(R))
        .build();
    apply_in(&storage, VICTIM, &delete, 20).await;
    assert!(items_of(VICTIM).await.is_empty());
    assert_eq!(score_rows(&storage, BLOCK).await, vec![(u(C), 1)]);

    // R is typed in both spaces in the graph. Recovery finds neither pair (the
    // victim's is tombstoned, OTHER's is registered) and can't resurrect the
    // victim's even when handed it directly.
    for (rel, space) in [(0x359C_10A0u128, VICTIM), (0x359C_20A0, OTHER)] {
        sqlx::query(
            "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(u(rel))
        .bind(u(rel ^ 0xE))
        .bind(Uuid::parse_str(TYPE_RELATION_TYPE_ID).unwrap())
        .bind(u(R))
        .bind(Uuid::parse_str(RANK_TYPE_ID).unwrap())
        .bind(u(space))
        .execute(pool)
        .await
        .unwrap();
    }
    let candidates = storage.find_unregistered_ranks().await.unwrap();
    assert!(!candidates.iter().any(|(id, _)| *id == u(R)));
    let config = storage
        .get_rank_config_from_kg(u(R), u(VICTIM))
        .await
        .unwrap()
        .expect("typed in the victim's space");
    assert!(
        !storage.recover_ranking(&config).await.unwrap(),
        "recovery never registers over a tombstone"
    );
    let tombstoned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ranks.deleted_rankings WHERE id = $1 AND space_id = $2",
    )
    .bind(u(R))
    .bind(u(VICTIM))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(tombstoned, 1, "and never clears it");
    assert_eq!(
        storage.block_id_for_ranking(u(R), u(VICTIM)).await.unwrap(),
        None
    );
}

/// Patrick's review: ballots cleared before the delete handling was deployed.
/// A registered Rank whose items all lack a relation id and with no vote
/// relation left in its space has its items removed and its block rescored;
/// a Rank whose vote relations are still in the graph is left alone. A dry
/// run only counts.
#[tokio::test]
async fn reconcile_removes_ballots_cleared_before_the_deploy() {
    let Some(url) = e2e_database_url() else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let storage = Storage::new(&url).await.expect("connect");
    let pool = storage.pool();
    const SPACE: u128 = 0xE2E5_359D_0001;
    const M1: u128 = 0xE2E5_359D_0011;
    const M2: u128 = 0xE2E5_359D_0012;
    const BLOCK: u128 = 0xE2E5_359D_0021;
    const CLEARED: u128 = 0xE2E5_359D_0041;
    const KEPT: u128 = 0xE2E5_359D_0042;
    const A: u128 = 0xE2E5_359D_0031;
    const B: u128 = 0xE2E5_359D_0032;
    fresh_dao_block(&storage, SPACE, &[M1, M2], BLOCK, &[CLEARED, KEPT], None).await;
    apply_in(
        &storage,
        M1,
        &ballot_edit(CLEARED, BLOCK, 0x359D_1000, SPACE, &[A]),
        10,
    )
    .await;
    apply_in(
        &storage,
        M2,
        &ballot_edit(KEPT, BLOCK, 0x359D_2000, SPACE, &[B]),
        11,
    )
    .await;
    // As written before 0125: no relation ids. KEPT's vote relation is still
    // in the graph; CLEARED's was deleted before the new indexer ran.
    sqlx::query("UPDATE ranks.ranking_items SET relation_id = NULL WHERE ranking_id = ANY($1)")
        .bind(&[u(CLEARED), u(KEPT)][..])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, \
         to_space_id, space_id) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(u(0x359D_2000))
    .bind(u(0x359D_2E00))
    .bind(Uuid::parse_str(RANK_VOTES_RELATION_TYPE_ID).unwrap())
    .bind(u(KEPT))
    .bind(u(B))
    .bind(u(SPACE))
    .bind(u(M2))
    .execute(pool)
    .await
    .unwrap();
    let count_items = |rank: u128| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM ranks.ranking_items WHERE ranking_id = $1",
        )
        .bind(u(rank))
        .fetch_one(pool)
        .await
        .unwrap()
    };

    let (rankings, items, blocks) = storage.reconcile_cleared_ballots(true).await.unwrap();
    assert!(
        rankings >= 1 && items >= 1 && blocks >= 1,
        "the dry run counts CLEARED"
    );
    assert_eq!(count_items(CLEARED).await, 1, "a dry run writes nothing");

    storage.reconcile_cleared_ballots(false).await.unwrap();
    assert_eq!(count_items(CLEARED).await, 0);
    assert_eq!(
        count_items(KEPT).await,
        1,
        "a ballot still in the graph is kept"
    );
    assert!(
        !queued(&storage, BLOCK).await.is_empty(),
        "its block is queued"
    );
    ranking_indexer::recompute::drain_pending_recomputes(
        &storage,
        Utc::now(),
        ranking_indexer::recompute::DrainMode::Backfill,
        &std::collections::HashSet::new(),
    )
    .await;
    assert_eq!(score_rows(&storage, BLOCK).await, vec![(u(B), 1)]);
}
