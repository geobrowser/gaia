//! Integration tests for the V2 governance storage methods introduced by GEO-481.
//!
//! These tests verify proposal versioning, vote reset on update, version-gated
//! vote writes, and the new `space_voting_settings` upsert against a real
//! PostgreSQL database with the V2 schema migration applied.
//!
//! Prerequisites:
//! - PostgreSQL running with migrations applied (including 0057 — V2 governance)
//! - `DATABASE_URL` environment variable set
//!
//! Run with:
//!   `cargo test --package kg-indexer --test governance_storage_integration -- --ignored`

use kg_indexer::models::governance::{
    ProposalActionItem, ProposalActionPayload, ProposalItem, ProposalVoteItem,
    SpaceVotingSettingsItem, VoteOption, VotingMode,
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
        total_editors: 0,
        updated_at,
        updated_at_block: 1,
    }
}

// --------------------------------------------------------------------------
// upsert_space_voting_settings preserves total_editors across updates
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_upsert_space_voting_settings_preserves_total_editors() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;

    // First upsert: inserts with total_editors = 0 (handler always passes 0).
    let first = settings_item(space_id, 500_000, 1_000);
    let mut tx = pool.begin().await.unwrap();
    storage
        .upsert_space_voting_settings(&first, &mut tx)
        .await
        .expect("first upsert failed");
    tx.commit().await.unwrap();

    // Simulate GEO-482's editor-counter updating total_editors out-of-band.
    sqlx::query("UPDATE space_voting_settings SET total_editors = 5 WHERE space_id = $1")
        .bind(space_id)
        .execute(&pool)
        .await
        .unwrap();

    // Second upsert: new settings (partial bumped), handler still passes total_editors=0.
    let second = settings_item(space_id, 999_999, 2_000);
    let mut tx = pool.begin().await.unwrap();
    storage
        .upsert_space_voting_settings(&second, &mut tx)
        .await
        .expect("second upsert failed");
    tx.commit().await.unwrap();

    // Settings were updated; total_editors stayed at 5 (not clobbered by the 0 from the item).
    let row: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT partial_percentage_support_threshold, total_editors, updated_at_block::bigint
           FROM space_voting_settings WHERE space_id = $1"#,
    )
    .bind(space_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        row.0, 999_999,
        "partial threshold should reflect latest upsert"
    );
    assert_eq!(
        row.1, 5,
        "total_editors must be preserved across upserts (maintained by GEO-482)"
    );

    cleanup_space_voting_settings(&pool, space_id).await;
}

// --------------------------------------------------------------------------
// insert_proposals writes V2 columns with proposal_version = 1
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_insert_proposals_writes_v2_columns() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;

    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Slow,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 10,
        threshold: 500_000,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: Some("Test".to_string()),
        proposal_version: 1,
        partial_percentage_support_threshold: 500_000,
        universal_percentage_support_threshold: 750_000,
        flat_support_threshold: 3,
        execute_by: Some(3_000),
    };

    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal), &mut tx)
        .await
        .expect("insert_proposals failed");
    tx.commit().await.unwrap();

    let row: (i32, i64, i64, i64, Option<i64>) = sqlx::query_as(
        r#"SELECT proposal_version,
                  partial_percentage_support_threshold,
                  universal_percentage_support_threshold,
                  flat_support_threshold,
                  execute_by
           FROM proposals WHERE id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, 1, "proposal_version should default to 1 on create");
    assert_eq!(row.1, 500_000);
    assert_eq!(row.2, 750_000);
    assert_eq!(row.3, 3);
    assert_eq!(row.4, Some(3_000));

    // cleanup
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id][..])
        .execute(&pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// update_proposal atomically bumps version, deletes votes, resets tallies
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_update_proposal_bumps_version_and_resets_votes() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;

    let proposal_v1 = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Slow,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 10,
        threshold: 500_000,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: Some("V1".to_string()),
        proposal_version: 1,
        partial_percentage_support_threshold: 500_000,
        universal_percentage_support_threshold: 750_000,
        flat_support_threshold: 3,
        execute_by: Some(3_000),
    };

    // Insert proposal + 3 votes, simulate yes_count populated.
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal_v1), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Seed 3 v1 votes via the storage path.
    let mut tx = pool.begin().await.unwrap();
    for _ in 0..3 {
        let voter_id = Uuid::new_v4();
        ensure_space(&pool, voter_id).await;
        let vote = ProposalVoteItem {
            proposal_id,
            voter_id,
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

    sqlx::query("UPDATE proposals SET yes_count = 3 WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .unwrap();

    // Sanity: 3 votes, yes_count = 3, version = 1.
    let (votes_before, yes_before, version_before): (i64, i64, i32) = sqlx::query_as(
        r#"SELECT (SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1),
                  p.yes_count, p.proposal_version
           FROM proposals p WHERE p.id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(votes_before, 3);
    assert_eq!(yes_before, 3);
    assert_eq!(version_before, 1);

    // Call update_proposal with a new payload.
    let proposal_v2 = ProposalItem {
        name: Some("V2".to_string()),
        partial_percentage_support_threshold: 600_000,
        ..proposal_v1.clone()
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .update_proposal(&proposal_v2, &mut tx)
        .await
        .expect("update_proposal failed");
    tx.commit().await.unwrap();

    // Version bumped to 2, denormalized counts reset. V2 semantics:
    // prior-version votes are PRESERVED as history — scoped by proposal_version.
    let (version_after, yes_after, partial_after, votes_total, votes_v1): (
        i32,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        r#"SELECT p.proposal_version, p.yes_count, p.partial_percentage_support_threshold,
                  (SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1),
                  (SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1 AND proposal_version = 1)
           FROM proposals p WHERE p.id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        version_after, 2,
        "proposal_version should increment on update"
    );
    assert_eq!(yes_after, 0, "denormalized yes_count should reset to 0");
    assert_eq!(
        partial_after, 600_000,
        "settings should reflect update payload"
    );
    assert_eq!(
        votes_total, 3,
        "v1 votes should be preserved as history, not deleted"
    );
    assert_eq!(votes_v1, 3, "all 3 votes remain scoped to version 1");

    // cleanup
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id][..])
        .execute(&pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// Votes are version-scoped: prior-version votes survive a proposal update
// and coexist with new-version votes as history.
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

    // Insert proposal v1.
    let proposal_v1 = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Slow,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 10,
        threshold: 500_000,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: None,
        proposal_version: 1,
        partial_percentage_support_threshold: 500_000,
        universal_percentage_support_threshold: 750_000,
        flat_support_threshold: 3,
        execute_by: Some(3_000),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal_v1), &mut tx)
        .await
        .unwrap();

    // Vote v1.
    let vote_v1 = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::Yes,
        created_at: 1_700_000_100,
        created_at_block: 2,
        proposal_version: 1,
    };
    storage
        .insert_proposal_votes(std::slice::from_ref(&vote_v1), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Bump to v2.
    let mut tx = pool.begin().await.unwrap();
    storage
        .update_proposal(&proposal_v1, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // v1 vote should still be there (history preserved).
    let v1_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1 AND proposal_version = 1",
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        v1_count.0, 1,
        "v1 vote must survive as history after proposal update"
    );

    // Vote again on v2 — same voter, different version, should insert cleanly.
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

    // Both versions should now coexist for the same (proposal, voter).
    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(total.0, 2, "v1 and v2 votes should coexist");

    // cleanup
    sqlx::query("DELETE FROM proposal_votes WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id, voter_id][..])
        .execute(&pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// insert_proposal_votes writes votes when version matches
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

    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Slow,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 10,
        threshold: 500_000,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: None,
        proposal_version: 1,
        partial_percentage_support_threshold: 500_000,
        universal_percentage_support_threshold: 750_000,
        flat_support_threshold: 3,
        execute_by: Some(3_000),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

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

    // cleanup
    sqlx::query("DELETE FROM proposal_votes WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id, voter_id][..])
        .execute(&pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// update_proposal_settings writes V2 fields without touching proposal_version
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

    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Fast,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 10,
        threshold: 5,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: None,
        proposal_version: 1,
        partial_percentage_support_threshold: 0,
        universal_percentage_support_threshold: 0,
        flat_support_threshold: 5,
        execute_by: Some(3_000),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Fast→slow escalation: new settings, NO version bump.
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

    let (version, mode, partial, execute_by): (i32, String, i64, Option<i64>) = sqlx::query_as(
        r#"SELECT proposal_version, voting_mode::text,
                  partial_percentage_support_threshold, execute_by
           FROM proposals WHERE id = $1"#,
    )
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(version, 1, "escalation must NOT bump proposal_version");
    assert_eq!(mode, "Slow");
    assert_eq!(partial, 500_000);
    assert_eq!(execute_by, Some(4_000));

    // cleanup
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id][..])
        .execute(&pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// process_tally_queue uses flat_support_threshold for fast-path auto-exec
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_process_tally_queue_auto_exec_uses_flat_threshold() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();
    let voter_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;
    ensure_space(&pool, voter_id).await;

    // Fast-path proposal: flat_support_threshold = 1, legacy threshold deliberately
    // set HIGH (99) to prove we read flat_support_threshold, not the legacy column.
    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Fast,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 1,
        threshold: 99,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: None,
        proposal_version: 1,
        partial_percentage_support_threshold: 0,
        universal_percentage_support_threshold: 0,
        flat_support_threshold: 1,
        execute_by: Some(9_999_999_999),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal), &mut tx)
        .await
        .unwrap();

    // Cast one yes vote.
    let vote = ProposalVoteItem {
        proposal_id,
        voter_id,
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
    storage
        .queue_tally_update(proposal_id, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    storage
        .process_tally_queue(10)
        .await
        .expect("process_tally_queue failed");

    // Auto-exec should have fired based on flat_support_threshold == 1 (yes_count >= 1),
    // NOT the legacy threshold of 99 which would have blocked execution.
    let executed_at: Option<i64> =
        sqlx::query_as::<_, (Option<i64>,)>("SELECT executed_at FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    assert!(
        executed_at.is_some(),
        "fast-path auto-exec should fire when yes_count >= flat_support_threshold"
    );

    // cleanup
    sqlx::query("DELETE FROM proposal_votes WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id, voter_id][..])
        .execute(&pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// process_tally_queue SKIPS auto-exec when past execute_by deadline
// --------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_process_tally_queue_respects_execute_by_deadline() {
    let pool = get_pool().await;
    let storage = setup_storage().await;
    let space_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let proposer_id = Uuid::new_v4();
    let voter_id = Uuid::new_v4();

    ensure_space(&pool, space_id).await;
    ensure_space(&pool, proposer_id).await;
    ensure_space(&pool, voter_id).await;

    // Fast-path proposal with an execute_by in the PAST relative to the vote timestamp.
    // vote.created_at = 1_700_000_100; execute_by = 1_700_000_050 (50 seconds earlier).
    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Fast,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 1,
        threshold: 1,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: None,
        proposal_version: 1,
        partial_percentage_support_threshold: 0,
        universal_percentage_support_threshold: 0,
        flat_support_threshold: 1,
        execute_by: Some(1_700_000_050),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal), &mut tx)
        .await
        .unwrap();

    let vote = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::Yes,
        created_at: 1_700_000_100, // past execute_by
        created_at_block: 2,
        proposal_version: 1,
    };
    storage
        .insert_proposal_votes(std::slice::from_ref(&vote), &mut tx)
        .await
        .unwrap();
    storage
        .queue_tally_update(proposal_id, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    storage
        .process_tally_queue(10)
        .await
        .expect("process_tally_queue failed");

    let executed_at: Option<i64> =
        sqlx::query_as::<_, (Option<i64>,)>("SELECT executed_at FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    assert!(
        executed_at.is_none(),
        "auto-exec must NOT fire when latest vote is past execute_by"
    );

    // cleanup
    sqlx::query("DELETE FROM proposal_votes WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id, voter_id][..])
        .execute(&pool)
        .await
        .ok();
}

// --------------------------------------------------------------------------
// process_tally_queue counts only current-version votes (version scoping)
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

    // Insert Slow proposal + 1 v1 Yes vote.
    let proposal = ProposalItem {
        id: proposal_id,
        space_id,
        proposed_by: proposer_id,
        voting_mode: VotingMode::Slow,
        start_time: 1_000,
        end_time: 2_000,
        quorum: 1,
        threshold: 500_000,
        executed_at: None,
        created_at: 1_700_000_000,
        created_at_block: 1,
        name: None,
        proposal_version: 1,
        partial_percentage_support_threshold: 500_000,
        universal_percentage_support_threshold: 750_000,
        flat_support_threshold: 99,
        execute_by: Some(9_999_999_999),
    };
    let mut tx = pool.begin().await.unwrap();
    storage
        .insert_proposals(std::slice::from_ref(&proposal), &mut tx)
        .await
        .unwrap();
    let v1_vote = ProposalVoteItem {
        proposal_id,
        voter_id,
        space_id,
        vote: VoteOption::Yes,
        created_at: 1_700_000_100,
        created_at_block: 2,
        proposal_version: 1,
    };
    storage
        .insert_proposal_votes(std::slice::from_ref(&v1_vote), &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Bump to v2 (v1 vote remains in history).
    let mut tx = pool.begin().await.unwrap();
    storage.update_proposal(&proposal, &mut tx).await.unwrap();
    tx.commit().await.unwrap();

    // Queue + process. With only a v1 vote and the proposal now at v2,
    // the current-version tally should be 0 yes votes.
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

    let (yes, no, abstain): (i64, i64, i64) =
        sqlx::query_as("SELECT yes_count, no_count, abstain_count FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        yes, 0,
        "tally must count only current-version votes; v1 vote is history"
    );
    assert_eq!(no, 0);
    assert_eq!(abstain, 0);

    // Now cast a v2 vote — tally should reflect it.
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

    let yes_v2: (i64,) = sqlx::query_as("SELECT yes_count FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(yes_v2.0, 1, "v2 vote should count in current-version tally");

    // cleanup
    sqlx::query("DELETE FROM proposal_votes WHERE proposal_id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM proposals WHERE id = $1")
        .bind(proposal_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spaces WHERE id = ANY($1)")
        .bind(&[space_id, proposer_id, voter_id][..])
        .execute(&pool)
        .await
        .ok();
}

// Silence unused-import warnings for models not exercised in every test.
#[allow(dead_code)]
fn _unused(_p: ProposalActionItem, _x: ProposalActionPayload) {}
