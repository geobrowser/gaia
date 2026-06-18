//! Integration tests for V2 governance storage (GEO-481, Shape B).
//!
//! Proposals are split into identity (`proposals`) + append-only versions
//! (`proposal_versions`). Per-version mutable state (voting settings,
//! tallies, name) lives on `proposal_versions` scoped by
//! `(proposal_id, proposal_version)`. Identity-level fields
//! (`executed_at`, `current_version`) live on `proposals`.
//!
//! Votes and actions are version-scoped — prior-version votes are history, not
//! deleted on update.
//!
//! Prerequisites:
//! - PostgreSQL running with migrations applied (through 0057 — V2 governance)
//! - `DATABASE_URL` environment variable set
//!
//! Run with:
//!   `cargo test --package kg-indexer --test governance_storage_integration -- --ignored`

use kg_indexer::models::governance::{
    ProposalActionItem, ProposalActionPayload, ProposalIdentity, ProposalVersionItem,
    ProposalVoteItem, SpaceVotingSettingsItem, VoteOption, VotingMode,
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

/// Insert a DAO space row so governance FKs resolve.
async fn ensure_space(pool: &sqlx::Pool<sqlx::Postgres>, id: Uuid) {
    sqlx::query(
        r#"INSERT INTO spaces (id, type, address)
           VALUES ($1, 'DAO'::"spaceTypes", '0x0000000000000000000000000000000000000000')
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("failed to insert space");
}

async fn cleanup_proposal(
    pool: &sqlx::Pool<sqlx::Postgres>,
    proposal_id: Uuid,
    space_ids: &[Uuid],
) {
    sqlx::query("DELETE FROM proposal_tally_queue WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposal_votes WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposal_actions WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposal_versions WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(space_ids)
        .execute(pool)
        .await
        .ok();
}

async fn cleanup_space_voting_settings(pool: &sqlx::Pool<sqlx::Postgres>, space_id: Uuid) {
    sqlx::query("DELETE FROM space_voting_settings WHERE space_id = $1")
        .bind(space_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(space_id)
        .execute(pool)
        .await
        .ok();
}

fn identity(proposal_id: Uuid, space_id: Uuid, proposer_id: Uuid) -> ProposalIdentity {
    ProposalIdentity {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        created_at: 1_700_000_000,
        created_at_block: 1,
    }
}

fn version_slow(name: Option<&str>) -> ProposalVersionItem {
    ProposalVersionItem {
        voting_mode: VotingMode::Slow,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 10,
        threshold: 500_000,
        partial_percentage_support_threshold: 500_000,
        universal_percentage_support_threshold: 750_000,
        flat_support_threshold: 3,
        execute_by: Some(3_000),
        name: name.map(String::from),
        version_created_at: 1_700_000_000,
        version_created_at_block: 1,
    }
}

fn version_fast(flat_threshold: i64, execute_by: Option<i64>) -> ProposalVersionItem {
    ProposalVersionItem {
        voting_mode: VotingMode::Fast,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 1,
        threshold: flat_threshold,
        partial_percentage_support_threshold: 0,
        universal_percentage_support_threshold: 0,
        flat_support_threshold: flat_threshold,
        execute_by,
        name: None,
        version_created_at: 1_700_000_000,
        version_created_at_block: 1,
    }
}

/// Insert identity + version-1 row in a single transaction. Returns committed state.
async fn seed_proposal_v1(
    storage: &Storage,
    pool: &sqlx::Pool<sqlx::Postgres>,
    proposal_id: Uuid,
    space_id: Uuid,
    proposer_id: Uuid,
    version: &ProposalVersionItem,
) {
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposal_identity(&identity(proposal_id, space_id, proposer_id), &mut tx)
        .await
        .expect("insert_proposal_identity failed");
    storage
        .insert_proposal_version_initial(proposal_id, version, &mut tx)
        .await
        .expect("insert_proposal_version_initial failed");
    tx.commit().await.unwrap();
}

fn settings_item(space_id: Uuid, partial: i64, updated_at: i64) -> SpaceVotingSettingsItem {
    SpaceVotingSettingsItem {
        space_id,
        partial_percentage_support_threshold: partial,
        universal_percentage_support_threshold: 2_000_000,
        flat_support_threshold: 3,
        quorum: 4,
        duration: 5,
        disable_fast_path_access_for_new_members: false,
        execution_grace_period: 6,
        updated_at,
        updated_at_block: 1,
    }
}

// --------------------------------------------------------------------------
// upsert_space_voting_settings overwrites all fields on conflict
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_upsert_space_voting_settings_overwrites_on_conflict() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;

    let first = settings_item(space_id, 500_000, 1_000);
    let mut tx = pool.begin().await.unwrap();
    storage
        .upsert_space_voting_settings(&first, &mut tx)
        .await
        .expect("first upsert failed");
    tx.commit().await.unwrap();

    let second = settings_item(space_id, 999_999, 2_000);
    let mut tx = pool.begin().await.unwrap();
    storage
        .upsert_space_voting_settings(&second, &mut tx)
        .await
        .expect("second upsert failed");
    tx.commit().await.unwrap();

    let row: (i64, i64) = sqlx::query_as(
        r#"SELECT partial_percentage_support_threshold, updated_at_block::bigint
           FROM space_voting_settings WHERE space_id = $1"#,
    )
    .bind(space_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        row.0, 999_999,
        "partial threshold must reflect latest upsert"
    );
    assert_eq!(row.1, 2_000, "updated_at_block must reflect latest upsert");

    cleanup_space_voting_settings(&pool, space_id).await;
}

// --------------------------------------------------------------------------
// CREATE writes identity + version-1 with V2 columns
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_create_writes_identity_and_v1() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;

    let v1 = version_slow(Some("Test"));
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    // Identity row has current_version = 1.
    let cv: (i32,) = sqlx::query_as("SELECT current_version FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cv.0, 1, "current_version must be 1 on create");

    // Version-1 row has the V2 columns.
    let v: (i32, i64, i64, i64, Option<i64>, Option<String>) = sqlx::query_as(
        r#"SELECT proposal_version,
                  partial_percentage_support_threshold,
                  universal_percentage_support_threshold,
                  flat_support_threshold,
                  execute_by,
                  name
           FROM proposal_versions WHERE proposal_id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(v.0, 1);
    assert_eq!(v.1, 500_000);
    assert_eq!(v.2, 750_000);
    assert_eq!(v.3, 3);
    assert_eq!(v.4, Some(3_000));
    assert_eq!(v.5, Some("Test".into()));

    cleanup_proposal(&pool, proposal_id, &[space_id, proposer_id]).await;
}

// --------------------------------------------------------------------------
// UPDATE appends a new version row and bumps current_version.
// Prior-version votes are preserved as history.
// Denormalized counts on the new version start at 0.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_update_appends_new_version() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;

    let v1 = version_slow(Some("V1"));
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    // Seed 3 v1 votes.
    let voter_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
    for voter_id in &voter_ids {
        ensure_space(&pool, *voter_id).await;
    }
    let mut tx = pool.begin().await.unwrap();
    for voter_id in &voter_ids {
        let vote = ProposalVoteItem {
            proposal_id,
            voter_id: *voter_id,
            space_id,
            vote: VoteOption::Yes,
            created_at: 1_700_000_100,
            created_at_block: 2,
            proposal_version: 1,
        };
        storage
            .insert_proposal_votes(std::slice::from_ref(&vote), &mut tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    // Simulate tally worker having populated v1 yes_count.
    sqlx::query(
        r#"UPDATE proposal_versions SET yes_count = 3
           WHERE proposal_id = $1 AND proposal_version = 1"#,
    )
    .bind(proposal_id)
    .execute(&pool)
    .await
    .unwrap();

    // Append v2 with a new name + bumped partial threshold.
    // `version_created_at_block` must differ from v1 — it acts as the replay
    // idempotency key (see `proposal_versions_idempotency_key`).
    let mut v2 = version_slow(Some("V2"));
    v2.partial_percentage_support_threshold = 600_000;
    v2.version_created_at_block = 2;
    let mut tx = pool.begin().await.unwrap();
    let new_version = storage
        .insert_new_proposal_version(proposal_id, &v2, &mut tx)
        .await
        .expect("insert_new_proposal_version failed");
    tx.commit().await.unwrap();

    assert_eq!(new_version, 2, "append must return the new version number");

    // current_version bumped; version-2 row has fresh counts.
    let (cv, v2_yes, v2_partial, v2_name): (i32, i64, i64, Option<String>) = sqlx::query_as(
        r#"SELECT p.current_version, pv.yes_count, pv.partial_percentage_support_threshold, pv.name
           FROM proposals p
           JOIN proposal_versions pv
             ON pv.proposal_id = p.id AND pv.proposal_version = p.current_version
           WHERE p.id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cv, 2, "current_version should be bumped");
    assert_eq!(v2_yes, 0, "new-version yes_count must start at 0");
    assert_eq!(v2_partial, 600_000);
    assert_eq!(v2_name, Some("V2".into()));

    // v1 row + its votes remain as history.
    let (v1_yes_hist, v1_vote_count): (i64, i64) = sqlx::query_as(
        r#"SELECT (SELECT yes_count FROM proposal_versions
                   WHERE proposal_id = $1 AND proposal_version = 1),
                  (SELECT COUNT(*) FROM proposal_votes
                   WHERE proposal_id = $1 AND proposal_version = 1)"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(v1_yes_hist, 3, "v1 tally must be preserved as history");
    assert_eq!(v1_vote_count, 3, "v1 votes must be preserved as history");

    let mut all_spaces = vec![space_id, proposer_id];
    all_spaces.extend(voter_ids);
    cleanup_proposal(&pool, proposal_id, &all_spaces).await;
}

// --------------------------------------------------------------------------
// Votes are version-scoped: a prior-version vote coexists with a new-version
// vote from the same voter.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_votes_scoped_by_version_across_proposal_update() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();
    let voter_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;
    ensure_space(&pool, voter_id).await;

    let v1 = version_slow(None);
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    // v1 vote.
    let vote_v1 = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::Yes,
        created_at: 1_700_000_100,
        created_at_block: 2,
        proposal_version: 1,
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposal_votes(std::slice::from_ref(&vote_v1), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Append v2. Must use a distinct `version_created_at_block` since that's
    // the replay idempotency key (see `proposal_versions_idempotency_key`).
    let mut v2 = v1.clone();
    v2.version_created_at_block = 2;
    let mut tx = pool.begin().await.unwrap();
    let new_version = storage
        .insert_new_proposal_version(proposal_id, &v2, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(new_version, 2);

    // v1 vote survives.
    let v1_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1 AND proposal_version = 1",
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(v1_count.0, 1, "v1 vote must survive as history");

    // Same voter votes on v2 — inserts cleanly (different PK component).
    let vote_v2 = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::No,
        created_at: 1_700_000_200,
        created_at_block: 3,
        proposal_version: 2,
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposal_votes(std::slice::from_ref(&vote_v2), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(total.0, 2, "v1 and v2 votes must coexist");

    cleanup_proposal(&pool, proposal_id, &[space_id, proposer_id, voter_id]).await;
}

// --------------------------------------------------------------------------
// Happy-path vote insert.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_insert_proposal_votes_writes_when_version_matches() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();
    let voter_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;
    ensure_space(&pool, voter_id).await;

    let v1 = version_slow(None);
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    let vote = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::Yes,
        created_at: 1_700_000_100,
        created_at_block: 2,
        proposal_version: 1,
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposal_votes(std::slice::from_ref(&vote), &mut tx)
        .await
        .expect("insert_proposal_votes failed");
    tx.commit().await.unwrap();

    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 1);

    cleanup_proposal(&pool, proposal_id, &[space_id, proposer_id, voter_id]).await;
}

// --------------------------------------------------------------------------
// update_proposal_settings mutates the current version row without bumping
// proposal_version or current_version (fast→slow escalation semantics).
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_update_proposal_settings_preserves_version() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;

    let v1 = version_fast(5, Some(3_000));
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    let mut tx = pool.begin().await.unwrap();
    storage
        .update_proposal_settings(
            proposal_id,
            "Slow",
            1_100,
            2_100,
            20,
            500_000,
            500_000,
            750_000,
            3,
            Some(4_000),
            &mut tx,
        )
        .await
        .expect("update_proposal_settings failed");
    tx.commit().await.unwrap();

    // current_version still 1; version-1 row updated in place.
    let (cv, pv_ver, mode, partial, execute_by): (i32, i32, String, i64, Option<i64>) =
        sqlx::query_as(
            r#"SELECT p.current_version, pv.proposal_version,
                      pv.voting_mode::text,
                      pv.partial_percentage_support_threshold, pv.execute_by
               FROM proposals p
               JOIN proposal_versions pv
                 ON pv.proposal_id = p.id AND pv.proposal_version = p.current_version
               WHERE p.id = $1"#,
        )
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(cv, 1, "escalation must NOT bump current_version");
    assert_eq!(pv_ver, 1);
    assert_eq!(mode, "Slow");
    assert_eq!(partial, 500_000);
    assert_eq!(execute_by, Some(4_000));

    cleanup_proposal(&pool, proposal_id, &[space_id, proposer_id]).await;
}

// --------------------------------------------------------------------------
// process_tally_queue counts only votes on the current version. A v1 vote
// is history after a v2 append and must not contribute to v2 tallies.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_process_tally_queue_counts_only_current_version() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();
    let voter_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;
    ensure_space(&pool, voter_id).await;

    let mut v1 = version_slow(None);
    v1.flat_support_threshold = 99;
    v1.execute_by = Some(9_999_999_999);
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    // v1 yes vote.
    let v1_vote = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::Yes,
        created_at: 1_700_000_100,
        created_at_block: 2,
        proposal_version: 1,
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposal_votes(std::slice::from_ref(&v1_vote), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Append v2. Must use a distinct `version_created_at_block` since that's
    // the replay idempotency key (see `proposal_versions_idempotency_key`).
    let mut v2 = v1.clone();
    v2.version_created_at_block = 2;
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_new_proposal_version(proposal_id, &v2, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Queue + process. No v2 votes exist yet — current-version tally should be 0.
    let mut tx = pool.begin().await.unwrap();
    storage
        .queue_tally_update(proposal_id, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    storage
        .process_tally_queue(10)
        .await
        .expect("process_tally_queue failed");

    let (yes, no, abstain): (i64, i64, i64) = sqlx::query_as(
        r#"SELECT pv.yes_count, pv.no_count, pv.abstain_count
           FROM proposals p
           JOIN proposal_versions pv
             ON pv.proposal_id = p.id AND pv.proposal_version = p.current_version
           WHERE p.id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(yes, 0, "v1 vote is history — must not count for v2");
    assert_eq!(no, 0);
    assert_eq!(abstain, 0);

    // Cast a v2 vote and re-tally.
    let v2_vote = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::Yes,
        created_at: 1_700_000_200,
        created_at_block: 3,
        proposal_version: 2,
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposal_votes(std::slice::from_ref(&v2_vote), &mut tx)
        .await
        .unwrap();
    storage
        .queue_tally_update(proposal_id, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    storage.process_tally_queue(10).await.unwrap();

    let yes_v2: (i64,) = sqlx::query_as(
        r#"SELECT pv.yes_count
           FROM proposals p
           JOIN proposal_versions pv
             ON pv.proposal_id = p.id AND pv.proposal_version = p.current_version
           WHERE p.id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(yes_v2.0, 1, "v2 vote must count in current-version tally");

    cleanup_proposal(&pool, proposal_id, &[space_id, proposer_id, voter_id]).await;
}

// --------------------------------------------------------------------------
// insert_new_proposal_version is idempotent on Kafka replay.
//
// Kafka delivery is at-least-once, so the same ProposalUpdated event can be
// re-processed. The `proposal_versions_idempotency_key` UNIQUE constraint on
// `(proposal_id, version_created_at_block)` combined with the
// `ON CONFLICT DO NOTHING` guard in `insert_new_proposal_version` must cause
// replays to be no-ops:
//   * both calls return the same version number,
//   * `proposals.current_version` only bumps once,
//   * exactly one `proposal_versions` row exists for that version.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_insert_new_proposal_version_is_idempotent_on_replay() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;

    // v1 seeded at block 1 (via insert_proposal_version_initial).
    let v1 = version_slow(Some("V1"));
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    // v2 at block 2.
    let mut v2 = version_slow(Some("V2"));
    v2.partial_percentage_support_threshold = 600_000;
    v2.version_created_at_block = 2;

    // First call: fresh event → inserts v2, bumps current_version to 2.
    let mut tx = pool.begin().await.unwrap();
    let returned_first = storage
        .insert_new_proposal_version(proposal_id, &v2, &mut tx)
        .await
        .expect("first insert_new_proposal_version failed");
    tx.commit().await.unwrap();
    assert_eq!(returned_first, 2, "first call must return version 2");

    // Second call with IDENTICAL input: simulated Kafka replay. Must NOT
    // insert a new row, NOT bump current_version, and MUST return the same
    // version number as the first call.
    let mut tx = pool.begin().await.unwrap();
    let returned_replay = storage
        .insert_new_proposal_version(proposal_id, &v2, &mut tx)
        .await
        .expect("replay insert_new_proposal_version failed");
    tx.commit().await.unwrap();
    assert_eq!(
        returned_replay, 2,
        "replay must return the already-assigned version number"
    );

    // current_version must still be 2 (not 3) after replay.
    let (current_version,): (i32,) =
        sqlx::query_as("SELECT current_version FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        current_version, 2,
        "current_version must not bump on replay"
    );

    // Exactly one row in proposal_versions for version 2.
    let (v2_row_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposal_versions
         WHERE proposal_id = $1 AND proposal_version = 2",
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        v2_row_count, 1,
        "replay must not duplicate the proposal_versions row"
    );

    // And only 2 total rows for this proposal (v1 + v2, no spurious v3).
    let (total_rows,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proposal_versions WHERE proposal_id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        total_rows, 2,
        "replay must not create a spurious v3 (or any additional version)"
    );

    cleanup_proposal(&pool, proposal_id, &[space_id, proposer_id]).await;
}

// --------------------------------------------------------------------------
// update_proposal_executed writes to proposals (identity), not proposal_versions.
//
// GEO-531 moved `executed_at` from `proposal_versions` to `proposals`. Execution
// is semantically identity-level — only the current version of a proposal can
// ever be executed, so the field belongs on the identity row, not the
// versioned one. This test pins that behavior.
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_update_proposal_executed_writes_to_identity() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;

    let v1 = version_slow(Some("Executed proposal"));
    seed_proposal_v1(&storage, &pool, proposal_id, space_id, proposer_id, &v1).await;

    // Bump to v2 so we can verify the write lands on identity, not on a
    // specific version row.
    let v2 = ProposalVersionItem {
        version_created_at_block: 2,
        ..version_slow(Some("Executed proposal v2"))
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_new_proposal_version(proposal_id, &v2, &mut tx)
        .await
        .expect("insert v2 failed");
    tx.commit().await.unwrap();

    // Stamp executed_at.
    let executed_at_ts: i64 = 1_700_000_500;
    let mut tx = pool.begin().await.unwrap();
    storage
        .update_proposal_executed(proposal_id, executed_at_ts, &mut tx)
        .await
        .expect("update_proposal_executed failed");
    tx.commit().await.unwrap();

    // The value must be readable from `proposals` directly — no join.
    let (from_identity,): (Option<i64>,) =
        sqlx::query_as("SELECT executed_at FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .expect("read from proposals failed");
    assert_eq!(
        from_identity,
        Some(executed_at_ts),
        "executed_at must live on proposals (identity) after GEO-531"
    );

    cleanup_proposal(&pool, proposal_id, &[space_id, proposer_id]).await;
}

// Silence unused-import warnings for types not exercised in every test.
#[allow(dead_code)]
fn _unused(_a: ProposalActionItem, _p: ProposalActionPayload) {}
