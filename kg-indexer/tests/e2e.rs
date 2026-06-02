//! E2E tests for kg-indexer
//!
//! These tests verify that the kg-indexer correctly processes events from
//! hermes-pipeline and indexes them into PostgreSQL.
//!
//! Prerequisites:
//! - PostgreSQL running with migrations applied
//! - hermes-pipeline has run in mock mode and produced events to Kafka
//! - kg-indexer has consumed all events from Kafka
//!
//! Run with: cargo test --package kg-indexer --test e2e

use sqlx::postgres::PgPoolOptions;
use std::env;
use uuid::Uuid;

/// Helper to create UUIDs matching the make_id() format from mock_events.rs
/// Creates a valid RFC 4122 UUID v4 with version and variant bits set
const fn make_id(last_byte: u8) -> [u8; 16] {
    [0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, last_byte]
}

fn uuid_from_bytes(bytes: [u8; 16]) -> Uuid {
    Uuid::from_bytes(bytes)
}

/// Expected space IDs from test_topology
mod expected {
    use super::*;

    pub const ROOT_SPACE_ID: [u8; 16] = make_id(0x01);
    pub const SPACE_A: [u8; 16] = make_id(0x0A);
    pub const SPACE_B: [u8; 16] = make_id(0x0B);
    pub const SPACE_C: [u8; 16] = make_id(0x0C);
    pub const SPACE_D: [u8; 16] = make_id(0x0D);
    pub const SPACE_E: [u8; 16] = make_id(0x0E);
    pub const SPACE_F: [u8; 16] = make_id(0x0F);
    pub const SPACE_G: [u8; 16] = make_id(0x10);
    pub const SPACE_H: [u8; 16] = make_id(0x11);
    pub const SPACE_I: [u8; 16] = make_id(0x12);
    pub const SPACE_J: [u8; 16] = make_id(0x13);
    pub const SPACE_X: [u8; 16] = make_id(0x20);
    pub const SPACE_Y: [u8; 16] = make_id(0x21);
    pub const SPACE_Z: [u8; 16] = make_id(0x22);
    pub const SPACE_W: [u8; 16] = make_id(0x23);
    pub const SPACE_P: [u8; 16] = make_id(0x30);
    pub const SPACE_Q: [u8; 16] = make_id(0x31);
    pub const SPACE_S: [u8; 16] = make_id(0x40);

    pub const PROPOSAL_1: [u8; 16] = make_id(0xA1);
    pub const PROPOSAL_2: [u8; 16] = make_id(0xA2);
    pub const PROPOSAL_3: [u8; 16] = make_id(0xA3);
    pub const PROPOSAL_4: [u8; 16] = make_id(0xA4);
    pub const PROPOSAL_5: [u8; 16] = make_id(0xA5);
    pub const PROPOSAL_6: [u8; 16] = make_id(0xA6);
    pub const PROPOSAL_7: [u8; 16] = make_id(0xA7);
    // Subspace proposal IDs
    pub const PROPOSAL_8: [u8; 16] = make_id(0xA8);
    pub const PROPOSAL_9: [u8; 16] = make_id(0xA9);
    pub const PROPOSAL_10: [u8; 16] = make_id(0xAA);
    pub const PROPOSAL_11: [u8; 16] = make_id(0xAB);
    pub const PROPOSAL_12: [u8; 16] = make_id(0xAC);
    pub const PROPOSAL_13: [u8; 16] = make_id(0xAD);
    pub const PROPOSAL_14: [u8; 16] = make_id(0xAE);
    pub const PROPOSAL_15: [u8; 16] = make_id(0xAF);

    // Subspace proposal targets
    pub const SUBSPACE_TARGET_VERIFIED: [u8; 16] = make_id(0xC1);
    pub const SUBSPACE_TARGET_UNVERIFIED: [u8; 16] = make_id(0xC2);
    pub const SUBSPACE_TARGET_RELATED: [u8; 16] = make_id(0xC3);
    pub const SUBSPACE_TARGET_UNRELATED: [u8; 16] = make_id(0xC4);
    pub const SUBSPACE_TARGET_TOPIC_DECLARED: [u8; 16] = make_id(0xC5);
    pub const SUBSPACE_TARGET_TOPIC_REMOVED: [u8; 16] = make_id(0xC6);
    pub const SPACE_TARGET_TOPIC_SET: [u8; 16] = make_id(0xD1);
    pub const SPACE_TARGET_TOPIC_REMOVED: [u8; 16] = make_id(0xD2);

    // Topic IDs for trust pipeline subtopic tests
    pub const TOPIC_H: [u8; 16] = make_id(0x91);
    pub const TOPIC_E: [u8; 16] = make_id(0x8E);
    pub const TOPIC_SHARED: [u8; 16] = make_id(0xF0);
    pub const TOPIC_A: [u8; 16] = make_id(0x8A);
    pub const TOPIC_Q: [u8; 16] = make_id(0xB1);
    pub const TOPIC_REMOVED: [u8; 16] = make_id(0x92);

    // Top-level space topic IDs (declared / removed via TOPIC_DECLARED & TOPIC_REMOVED actions).
    pub const SPACE_TOPIC_KEPT: [u8; 16] = make_id(0x93);
    pub const SPACE_TOPIC_CLEARED: [u8; 16] = make_id(0x94);
    pub const SPACE_TOPIC_STALE: [u8; 16] = make_id(0x95);

    /// All 18 space IDs that should be created
    pub fn all_space_ids() -> Vec<Uuid> {
        vec![
            uuid_from_bytes(ROOT_SPACE_ID),
            uuid_from_bytes(SPACE_A),
            uuid_from_bytes(SPACE_B),
            uuid_from_bytes(SPACE_C),
            uuid_from_bytes(SPACE_D),
            uuid_from_bytes(SPACE_E),
            uuid_from_bytes(SPACE_F),
            uuid_from_bytes(SPACE_G),
            uuid_from_bytes(SPACE_H),
            uuid_from_bytes(SPACE_I),
            uuid_from_bytes(SPACE_J),
            uuid_from_bytes(SPACE_X),
            uuid_from_bytes(SPACE_Y),
            uuid_from_bytes(SPACE_Z),
            uuid_from_bytes(SPACE_W),
            uuid_from_bytes(SPACE_P),
            uuid_from_bytes(SPACE_Q),
            uuid_from_bytes(SPACE_S),
        ]
    }

    /// Expected subspace relationships (parent, child) from subspace_verified + subspace_related calls
    pub fn subspace_relationships() -> Vec<(Uuid, Uuid)> {
        vec![
            (uuid_from_bytes(ROOT_SPACE_ID), uuid_from_bytes(SPACE_A)),
            (uuid_from_bytes(ROOT_SPACE_ID), uuid_from_bytes(SPACE_B)),
            (uuid_from_bytes(SPACE_A), uuid_from_bytes(SPACE_C)),
            (uuid_from_bytes(SPACE_B), uuid_from_bytes(SPACE_E)),
            (uuid_from_bytes(SPACE_C), uuid_from_bytes(SPACE_F)),
            (uuid_from_bytes(SPACE_H), uuid_from_bytes(SPACE_I)),
            (uuid_from_bytes(SPACE_H), uuid_from_bytes(SPACE_J)),
            (uuid_from_bytes(SPACE_X), uuid_from_bytes(SPACE_Y)),
            (uuid_from_bytes(SPACE_Y), uuid_from_bytes(SPACE_Z)),
            (uuid_from_bytes(SPACE_P), uuid_from_bytes(SPACE_Q)),
            // Related subspaces
            (uuid_from_bytes(ROOT_SPACE_ID), uuid_from_bytes(SPACE_H)),
            (uuid_from_bytes(SPACE_A), uuid_from_bytes(SPACE_D)),
            (uuid_from_bytes(SPACE_C), uuid_from_bytes(SPACE_G)),
            (uuid_from_bytes(SPACE_X), uuid_from_bytes(SPACE_W)),
        ]
    }

    /// All 15 proposal IDs (7 original + 6 subspace proposals + 2 space-topic proposals)
    pub fn all_proposal_ids() -> Vec<Uuid> {
        vec![
            uuid_from_bytes(PROPOSAL_1),
            uuid_from_bytes(PROPOSAL_2),
            uuid_from_bytes(PROPOSAL_3),
            uuid_from_bytes(PROPOSAL_4),
            uuid_from_bytes(PROPOSAL_5),
            uuid_from_bytes(PROPOSAL_6),
            uuid_from_bytes(PROPOSAL_7),
            uuid_from_bytes(PROPOSAL_8),
            uuid_from_bytes(PROPOSAL_9),
            uuid_from_bytes(PROPOSAL_10),
            uuid_from_bytes(PROPOSAL_11),
            uuid_from_bytes(PROPOSAL_12),
            uuid_from_bytes(PROPOSAL_13),
            uuid_from_bytes(PROPOSAL_14),
            uuid_from_bytes(PROPOSAL_15),
        ]
    }

    /// Expected subspace topic entries (space_id, topic_id) from trust pipeline.
    ///
    /// The trust pipeline stores (source_space_id, topic_id) where source_space_id
    /// is the parent space (from_id), not the subspace. So for:
    ///   subspace_topic_declared(SPACE_B, SPACE_H, TOPIC_H)
    /// the stored row is (SPACE_B, TOPIC_H), not (SPACE_H, TOPIC_H).
    ///
    /// 6 declared via subspace_topic_declared, 1 declared then removed (TOPIC_REMOVED).
    /// Net result: 5 rows in subspace_topics (the removed one shouldn't be there).
    pub fn subspace_topics() -> Vec<(Uuid, Uuid)> {
        vec![
            (uuid_from_bytes(SPACE_B), uuid_from_bytes(TOPIC_H)), // subspace_topic_declared(SPACE_B, SPACE_H, TOPIC_H)
            (uuid_from_bytes(ROOT_SPACE_ID), uuid_from_bytes(TOPIC_E)), // subspace_topic_declared(ROOT, SPACE_E, TOPIC_E)
            (uuid_from_bytes(SPACE_A), uuid_from_bytes(TOPIC_SHARED)), // subspace_topic_declared(SPACE_A, SPACE_A, TOPIC_SHARED)
            (uuid_from_bytes(SPACE_X), uuid_from_bytes(TOPIC_A)), // subspace_topic_declared(SPACE_X, SPACE_A, TOPIC_A)
            (uuid_from_bytes(SPACE_P), uuid_from_bytes(TOPIC_Q)), // subspace_topic_declared(SPACE_P, SPACE_Q, TOPIC_Q)
        ]
    }

    /// Expected proposal action types: (proposal_id, action_type, optional target_id)
    pub fn proposal_action_types() -> Vec<(Uuid, &'static str, Option<Uuid>)> {
        vec![
            // Original proposals 1-7
            (uuid_from_bytes(PROPOSAL_1), "AddMember", None), // target is make_id(0x11), not a known space
            (uuid_from_bytes(PROPOSAL_2), "RemoveMember", None), // target is make_id(0x12)
            (uuid_from_bytes(PROPOSAL_3), "AddEditor", None), // target is make_id(0x50)
            (uuid_from_bytes(PROPOSAL_4), "RemoveEditor", None), // target is make_id(0x51)
            (uuid_from_bytes(PROPOSAL_5), "Flag", None),
            (uuid_from_bytes(PROPOSAL_6), "Unflag", None),
            // Proposal 7 (Publish) - content_uri is present but target_id is null
            (uuid_from_bytes(PROPOSAL_7), "Publish", None),
            // Subspace proposals 8-13
            (
                uuid_from_bytes(PROPOSAL_8),
                "SubspaceVerified",
                Some(uuid_from_bytes(SUBSPACE_TARGET_VERIFIED)),
            ),
            (
                uuid_from_bytes(PROPOSAL_9),
                "SubspaceUnverified",
                Some(uuid_from_bytes(SUBSPACE_TARGET_UNVERIFIED)),
            ),
            (
                uuid_from_bytes(PROPOSAL_10),
                "SubspaceRelated",
                Some(uuid_from_bytes(SUBSPACE_TARGET_RELATED)),
            ),
            (
                uuid_from_bytes(PROPOSAL_11),
                "SubspaceUnrelated",
                Some(uuid_from_bytes(SUBSPACE_TARGET_UNRELATED)),
            ),
            (
                uuid_from_bytes(PROPOSAL_12),
                "SubspaceTopicDeclared",
                Some(uuid_from_bytes(SUBSPACE_TARGET_TOPIC_DECLARED)),
            ),
            (
                uuid_from_bytes(PROPOSAL_13),
                "SubspaceTopicRemoved",
                Some(uuid_from_bytes(SUBSPACE_TARGET_TOPIC_REMOVED)),
            ),
            (
                uuid_from_bytes(PROPOSAL_14),
                "SetTopic",
                Some(uuid_from_bytes(SPACE_TARGET_TOPIC_SET)),
            ),
            (
                uuid_from_bytes(PROPOSAL_15),
                "UnsetTopic",
                Some(uuid_from_bytes(SPACE_TARGET_TOPIC_REMOVED)),
            ),
        ]
    }
}

async fn get_pool() -> sqlx::Pool<sqlx::Postgres> {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database")
}

#[tokio::test]
async fn test_all_spaces_exist() {
    let pool = get_pool().await;

    for space_id in expected::all_space_ids() {
        let exists: (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM spaces WHERE id = $1)")
            .bind(space_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query space");

        assert!(
            exists.0,
            "Space {} should exist but was not found",
            space_id
        );
    }

    // Verify total count
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM spaces")
        .fetch_one(&pool)
        .await
        .expect("Failed to count spaces");

    assert_eq!(count.0, 18, "Expected 18 spaces, found {}", count.0);
}

#[tokio::test]
async fn test_subspace_relationships() {
    let pool = get_pool().await;

    for (parent_id, child_id) in expected::subspace_relationships() {
        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM subspaces WHERE parent_space_id = $1 AND child_space_id = $2)",
        )
        .bind(parent_id)
        .bind(child_id)
        .fetch_one(&pool)
        .await
        .expect("Failed to query subspace");

        assert!(
            exists.0,
            "Subspace relationship {} -> {} should exist",
            parent_id, child_id
        );
    }

    // Verify total count
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM subspaces")
        .fetch_one(&pool)
        .await
        .expect("Failed to count subspaces");

    assert_eq!(
        count.0, 14,
        "Expected 14 subspaces (10 verified + 4 related), found {}",
        count.0
    );
}

#[tokio::test]
async fn test_all_proposals_exist() {
    let pool = get_pool().await;

    for proposal_id in expected::all_proposal_ids() {
        let exists: (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM proposals WHERE id = $1)")
                .bind(proposal_id)
                .fetch_one(&pool)
                .await
                .expect("Failed to query proposal");

        assert!(
            exists.0,
            "Proposal {} should exist but was not found",
            proposal_id
        );
    }

    // Verify total count
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM proposals")
        .fetch_one(&pool)
        .await
        .expect("Failed to count proposals");

    assert_eq!(count.0, 15, "Expected 15 proposals, found {}", count.0);
}

#[tokio::test]
async fn test_proposal_1_details() {
    let pool = get_pool().await;

    let proposal_id = uuid_from_bytes(expected::PROPOSAL_1);
    let space_a = uuid_from_bytes(expected::SPACE_A);

    // Verify proposal exists in SPACE_A with Fast voting mode
    let proposal: Option<(Uuid, String)> =
        sqlx::query_as("SELECT space_id, voting_mode::text FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_optional(&pool)
            .await
            .expect("Failed to query proposal");

    let (space_id, voting_mode) = proposal.expect("Proposal 1 should exist");
    assert_eq!(space_id, space_a, "Proposal 1 should be in SPACE_A");
    assert_eq!(
        voting_mode, "Fast",
        "Proposal 1 should have Fast voting mode"
    );
}

#[tokio::test]
async fn test_proposal_2_details() {
    let pool = get_pool().await;

    let proposal_id = uuid_from_bytes(expected::PROPOSAL_2);
    let space_a = uuid_from_bytes(expected::SPACE_A);

    // Verify proposal exists in SPACE_A with Slow voting mode
    let proposal: Option<(Uuid, String)> =
        sqlx::query_as("SELECT space_id, voting_mode::text FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_optional(&pool)
            .await
            .expect("Failed to query proposal");

    let (space_id, voting_mode) = proposal.expect("Proposal 2 should exist");
    assert_eq!(space_id, space_a, "Proposal 2 should be in SPACE_A");
    assert_eq!(
        voting_mode, "Slow",
        "Proposal 2 should have Slow voting mode"
    );
}

#[tokio::test]
async fn test_proposal_votes_exist() {
    let pool = get_pool().await;

    // Proposal 1 should have 2 Yes votes (from SPACE_B and SPACE_C)
    let proposal_1 = uuid_from_bytes(expected::PROPOSAL_1);
    let vote_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1 AND vote = 'Yes'",
    )
    .bind(proposal_1)
    .fetch_one(&pool)
    .await
    .expect("Failed to count votes");

    assert_eq!(
        vote_count.0, 2,
        "Proposal 1 should have 2 Yes votes, found {}",
        vote_count.0
    );

    // Proposal 2 should have votes including a No vote
    let proposal_2 = uuid_from_bytes(expected::PROPOSAL_2);
    let no_votes: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposal_votes WHERE proposal_id = $1 AND vote = 'No'",
    )
    .bind(proposal_2)
    .fetch_one(&pool)
    .await
    .expect("Failed to count No votes");

    assert_eq!(
        no_votes.0, 1,
        "Proposal 2 should have 1 No vote, found {}",
        no_votes.0
    );
}

#[tokio::test]
async fn test_editors_in_space_a() {
    let pool = get_pool().await;

    let space_a = uuid_from_bytes(expected::SPACE_A);

    // SPACE_A should have USER_2 as an editor (from editor_added call)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM editors WHERE space_id = $1")
        .bind(space_a)
        .fetch_one(&pool)
        .await
        .expect("Failed to count editors");

    assert!(
        count.0 >= 1,
        "SPACE_A should have at least 1 editor, found {}",
        count.0
    );
}

#[tokio::test]
async fn test_members_in_space_a() {
    let pool = get_pool().await;

    let space_a = uuid_from_bytes(expected::SPACE_A);

    // SPACE_A should have members (from member_added calls and proposal executions)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM members WHERE space_id = $1")
        .bind(space_a)
        .fetch_one(&pool)
        .await
        .expect("Failed to count members");

    assert!(
        count.0 >= 1,
        "SPACE_A should have at least 1 member, found {}",
        count.0
    );
}

#[tokio::test]
async fn test_dao_space_p_has_editor() {
    let pool = get_pool().await;

    let space_p = uuid_from_bytes(expected::SPACE_P);

    // SPACE_P is a DAO space initialized with EDITOR_Q as initial editor
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM editors WHERE space_id = $1")
        .bind(space_p)
        .fetch_one(&pool)
        .await
        .expect("Failed to count editors for SPACE_P");

    assert_eq!(
        count.0, 1,
        "SPACE_P (DAO) should have exactly 1 initial editor, found {}",
        count.0
    );
}

#[tokio::test]
async fn test_executed_proposals() {
    let pool = get_pool().await;

    // All 15 proposals should be executed (have executed_at set)
    let executed_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proposals WHERE executed_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .expect("Failed to count executed proposals");

    assert_eq!(
        executed_count.0, 15,
        "All 15 proposals should be executed, found {} executed",
        executed_count.0
    );
}

/// Verify subspace_topics rows from the trust pipeline (SUBSPACE_TOPIC_DECLARED events).
///
/// The topology declares 6 subtopics but removes 1 (TOPIC_REMOVED), leaving 5.
#[tokio::test]
async fn test_subspace_topics() {
    let pool = get_pool().await;

    for (space_id, topic_id) in expected::subspace_topics() {
        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM subspace_topics WHERE space_id = $1 AND topic_id = $2)",
        )
        .bind(space_id)
        .bind(topic_id)
        .fetch_one(&pool)
        .await
        .expect("Failed to query subspace_topics");

        assert!(
            exists.0,
            "Subspace topic ({}, {}) should exist",
            space_id, topic_id
        );
    }

    // The removed topic should NOT be present
    // The parent space is SPACE_A (from_id in the subspace_topic_removed call),
    // not SPACE_C (the subspace).
    let removed_space = uuid_from_bytes(expected::SPACE_A);
    let removed_topic = uuid_from_bytes(expected::TOPIC_REMOVED);
    let exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM subspace_topics WHERE space_id = $1 AND topic_id = $2)",
    )
    .bind(removed_space)
    .bind(removed_topic)
    .fetch_one(&pool)
    .await
    .expect("Failed to query removed topic");

    assert!(
        !exists.0,
        "Removed topic ({}, {}) should NOT exist",
        removed_space, removed_topic
    );

    // Verify total count: 5 declared (TOPIC_REMOVED was declared then removed)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM subspace_topics")
        .fetch_one(&pool)
        .await
        .expect("Failed to count subspace_topics");

    assert_eq!(
        count.0, 5,
        "Expected 5 subspace_topics (6 declared - 1 removed), found {}",
        count.0
    );
}

/// Verify the top-level `spaces.topic_id` flow for TOPIC_DECLARED + TOPIC_REMOVED.
///
/// The mock topology emits:
///   - topic_declared(SPACE_J, SPACE_TOPIC_KEPT)
///   - topic_declared(SPACE_I, SPACE_TOPIC_CLEARED)
///   - topic_removed(SPACE_I, SPACE_TOPIC_CLEARED)
///
/// Expected final state:
///   - SPACE_J.topic_id = SPACE_TOPIC_KEPT
///   - SPACE_I.topic_id IS NULL (declared then cleared)
///   - The `entities` row for SPACE_TOPIC_CLEARED still exists (we only clear
///     the assignment, never delete the topic concept itself).
#[tokio::test]
async fn test_space_topic_declared_and_removed() {
    let pool = get_pool().await;

    let space_j = uuid_from_bytes(expected::SPACE_J);
    let space_i = uuid_from_bytes(expected::SPACE_I);
    let kept = uuid_from_bytes(expected::SPACE_TOPIC_KEPT);
    let cleared = uuid_from_bytes(expected::SPACE_TOPIC_CLEARED);

    // SPACE_J: topic_id should be set
    let kept_row: (Option<Uuid>,) = sqlx::query_as("SELECT topic_id FROM spaces WHERE id = $1")
        .bind(space_j)
        .fetch_one(&pool)
        .await
        .expect("Failed to query SPACE_J.topic_id");

    assert_eq!(
        kept_row.0,
        Some(kept),
        "SPACE_J.topic_id should be SPACE_TOPIC_KEPT (and survive the stale TOPIC_REMOVED)"
    );

    // SPACE_I: topic_id should be NULL after declare + remove
    let cleared_row: (Option<Uuid>,) = sqlx::query_as("SELECT topic_id FROM spaces WHERE id = $1")
        .bind(space_i)
        .fetch_one(&pool)
        .await
        .expect("Failed to query SPACE_I.topic_id");

    assert!(
        cleared_row.0.is_none(),
        "SPACE_I.topic_id should be NULL after TOPIC_REMOVED, got {:?}",
        cleared_row.0
    );

    // The topic concept entities must persist regardless of the assignment state.
    for topic_id in [kept, cleared] {
        let exists: (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM entities WHERE id = $1)")
            .bind(topic_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query entities");
        assert!(
            exists.0,
            "entities row for topic {} should exist after TOPIC_REMOVED",
            topic_id
        );
    }
}

/// Verify the conditional `clear_space_topic`: a TOPIC_REMOVED whose topicId does
/// NOT match the space's current topic must be a no-op.
///
/// The mock topology declares SPACE_TOPIC_KEPT on SPACE_J, then emits a
/// `topic_removed(SPACE_J, SPACE_TOPIC_STALE)`. Because SPACE_TOPIC_STALE is not
/// the current topic, the clear must not fire and SPACE_J must retain
/// SPACE_TOPIC_KEPT.
#[tokio::test]
async fn test_space_topic_stale_removal_is_noop() {
    let pool = get_pool().await;

    let space_j = uuid_from_bytes(expected::SPACE_J);
    let kept = uuid_from_bytes(expected::SPACE_TOPIC_KEPT);

    let row: (Option<Uuid>,) = sqlx::query_as("SELECT topic_id FROM spaces WHERE id = $1")
        .bind(space_j)
        .fetch_one(&pool)
        .await
        .expect("Failed to query SPACE_J.topic_id");

    assert_eq!(
        row.0,
        Some(kept),
        "A TOPIC_REMOVED for a non-current topic must not clear SPACE_J.topic_id"
    );
}

/// Verify proposal action types and target IDs for all 15 proposals.
///
/// This covers both the original proposal action types (AddMember, RemoveMember, etc.)
/// and the new ping-based proposal actions decoded from calldata.
#[tokio::test]
async fn test_proposal_action_types() {
    let pool = get_pool().await;

    for (proposal_id, expected_type, expected_target) in expected::proposal_action_types() {
        // Each proposal has exactly 1 action
        let row: Option<(String, Option<Uuid>)> = sqlx::query_as(
            "SELECT action_type::text, target_id FROM proposal_actions WHERE proposal_id = $1",
        )
        .bind(proposal_id)
        .fetch_optional(&pool)
        .await
        .expect("Failed to query proposal_actions");

        let (action_type, target_id) = row.unwrap_or_else(|| {
            panic!(
                "Proposal {} should have an action but none found",
                proposal_id
            )
        });

        assert_eq!(
            action_type, expected_type,
            "Proposal {} action_type: expected '{}', got '{}'",
            proposal_id, expected_type, action_type
        );

        if let Some(expected) = expected_target {
            assert_eq!(
                target_id,
                Some(expected),
                "Proposal {} target_id: expected Some({}), got {:?}",
                proposal_id,
                expected,
                target_id
            );
        }
    }
}
