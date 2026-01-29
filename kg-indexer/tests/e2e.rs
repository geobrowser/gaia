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

    pub const ENTITY_PERSON_2: [u8; 16] = make_id(0xF2);
    pub const ENTITY_ORG_1: [u8; 16] = make_id(0xF3);
    pub const ENTITY_PROJECT_1: [u8; 16] = make_id(0xF4);
    pub const ENTITY_DOC_1: [u8; 16] = make_id(0xF5);
    pub const ENTITY_TOPIC_1: [u8; 16] = make_id(0xF6);
    pub const ENTITY_SQUASH: [u8; 16] = make_id(0xF7);

    pub const PROPERTY_NAME: [u8; 16] = make_id(0xD1);
    pub const PROPERTY_DESCRIPTION: [u8; 16] = make_id(0xD2);

    pub const RELATION_1: [u8; 16] = make_id(0xA1);
    pub const RELATION_2: [u8; 16] = make_id(0xA2);
    pub const RELATION_3: [u8; 16] = make_id(0xA3);

    pub const EDIT_ROOT_2: [u8; 16] = make_id(0xE2);
    pub const EDIT_A_1: [u8; 16] = make_id(0xEA);

    pub const CONTEXT_ROOT_ID: [u8; 16] = ENTITY_ORG_1;
    pub const CONTEXT_EDGE_TYPE_ID: [u8; 16] = make_id(0xC2);

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

    /// Expected subspace relationships (parent, child) from subspace_verified calls
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
        ]
    }

    /// All 7 proposal IDs
    pub fn all_proposal_ids() -> Vec<Uuid> {
        vec![
            uuid_from_bytes(PROPOSAL_1),
            uuid_from_bytes(PROPOSAL_2),
            uuid_from_bytes(PROPOSAL_3),
            uuid_from_bytes(PROPOSAL_4),
            uuid_from_bytes(PROPOSAL_5),
            uuid_from_bytes(PROPOSAL_6),
            uuid_from_bytes(PROPOSAL_7),
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

    assert_eq!(count.0, 10, "Expected 10 subspaces, found {}", count.0);
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

    assert_eq!(count.0, 7, "Expected 7 proposals, found {}", count.0);
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

    // All 7 proposals should be executed (have executed_at set)
    let executed_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proposals WHERE executed_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .expect("Failed to count executed proposals");

    assert_eq!(
        executed_count.0, 7,
        "All 7 proposals should be executed, found {} executed",
        executed_count.0
    );
}

#[tokio::test]
async fn test_values_written_from_edits() {
    let pool = get_pool().await;

    let entity_org = uuid_from_bytes(expected::ENTITY_ORG_1);
    let entity_project = uuid_from_bytes(expected::ENTITY_PROJECT_1);
    let entity_doc = uuid_from_bytes(expected::ENTITY_DOC_1);
    let entity_topic = uuid_from_bytes(expected::ENTITY_TOPIC_1);
    let property_name = uuid_from_bytes(expected::PROPERTY_NAME);
    let property_description = uuid_from_bytes(expected::PROPERTY_DESCRIPTION);

    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM values WHERE entity_id IN ($1, $2, $3, $4)",
    )
    .bind(entity_org)
    .bind(entity_project)
    .bind(entity_doc)
    .bind(entity_topic)
    .fetch_one(&pool)
    .await
    .expect("Failed to count values for edit entities");

    assert!(
        count.0 >= 4,
        "Expected values for edit entities, found {}",
        count.0
    );

    let has_name: (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM values WHERE property_id = $1)",
    )
    .bind(property_name)
    .fetch_one(&pool)
    .await
    .expect("Failed to query name values");

    assert!(has_name.0, "Expected name values to be present");

    let has_description: (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM values WHERE property_id = $1)",
    )
    .bind(property_description)
    .fetch_one(&pool)
    .await
    .expect("Failed to query description values");

    assert!(has_description.0, "Expected description values to be present");
}

#[tokio::test]
async fn test_relations_written_from_edits() {
    let pool = get_pool().await;

    let relation_1 = uuid_from_bytes(expected::RELATION_1);
    let relation_2 = uuid_from_bytes(expected::RELATION_2);

    let exists_1: (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM relations WHERE id = $1)")
            .bind(relation_1)
            .fetch_one(&pool)
            .await
            .expect("Failed to query relation 1");

    let exists_2: (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM relations WHERE id = $1)")
            .bind(relation_2)
            .fetch_one(&pool)
            .await
            .expect("Failed to query relation 2");

    assert!(exists_1.0, "Expected relation 1 to exist");
    assert!(exists_2.0, "Expected relation 2 to exist");
}

#[tokio::test]
async fn test_relation_metadata_updated() {
    let pool = get_pool().await;

    let relation_1 = uuid_from_bytes(expected::RELATION_1);
    let space_b = uuid_from_bytes(expected::SPACE_B);
    let space_a = uuid_from_bytes(expected::SPACE_A);
    let edit_root_2 = uuid_from_bytes(expected::EDIT_ROOT_2);
    let edit_a_1 = uuid_from_bytes(expected::EDIT_A_1);

    let row: Option<(Uuid, Uuid, Uuid, Uuid, Option<String>, Option<bool>)> = sqlx::query_as(
        "SELECT from_space_id, to_space_id, from_version_id, to_version_id, position, verified FROM relations WHERE id = $1",
    )
    .bind(relation_1)
    .fetch_optional(&pool)
    .await
    .expect("Failed to query relation metadata");

    let (from_space_id, to_space_id, from_version_id, to_version_id, position, verified) =
        row.expect("Relation 1 should exist");

    assert_eq!(from_space_id, space_b);
    assert_eq!(to_space_id, space_a);
    assert_eq!(from_version_id, edit_root_2);
    assert_eq!(to_version_id, edit_a_1);
    assert_eq!(position.as_deref(), Some("2"));
    assert_eq!(verified, Some(false));
}

#[tokio::test]
async fn test_relation_unset_fields() {
    let pool = get_pool().await;

    let relation_2 = uuid_from_bytes(expected::RELATION_2);

    let row: Option<(
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        Option<String>,
        Option<bool>,
    )> = sqlx::query_as(
        "SELECT from_space_id, to_space_id, from_version_id, to_version_id, position, verified FROM relations WHERE id = $1",
    )
    .bind(relation_2)
    .fetch_optional(&pool)
    .await
    .expect("Failed to query relation 2 metadata");

    let (from_space_id, to_space_id, from_version_id, to_version_id, position, verified) =
        row.expect("Relation 2 should exist");

    assert!(from_space_id.is_none());
    assert!(to_space_id.is_none());
    assert!(from_version_id.is_none());
    assert!(to_version_id.is_none());
    assert!(position.is_none());
    assert!(verified.is_none());
}

#[tokio::test]
async fn test_relation_delete() {
    let pool = get_pool().await;

    let relation_3 = uuid_from_bytes(expected::RELATION_3);

    let exists: (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM relations WHERE id = $1)")
            .bind(relation_3)
            .fetch_one(&pool)
            .await
            .expect("Failed to query relation 3");

    assert!(!exists.0, "Relation 3 should be deleted");
}

#[tokio::test]
async fn test_value_squashing_last_write_wins() {
    let pool = get_pool().await;

    let entity = uuid_from_bytes(expected::ENTITY_SQUASH);
    let property = uuid_from_bytes(expected::PROPERTY_NAME);

    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT string FROM values WHERE entity_id = $1 AND property_id = $2",
    )
    .bind(entity)
    .bind(property)
    .fetch_optional(&pool)
    .await
    .expect("Failed to query squashed value");

    let (value,) = row.expect("Squashed value should exist");
    assert_eq!(value.as_deref(), Some("Final"));
}

#[tokio::test]
async fn test_value_delete_via_unset() {
    let pool = get_pool().await;

    let entity = uuid_from_bytes(expected::ENTITY_PERSON_2);
    let property = uuid_from_bytes(expected::PROPERTY_NAME);

    let exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM values WHERE entity_id = $1 AND property_id = $2)",
    )
    .bind(entity)
    .bind(property)
    .fetch_one(&pool)
    .await
    .expect("Failed to query deleted value");

    assert!(!exists.0, "Person 2 name should have been deleted");
}

#[tokio::test]
async fn test_edit_versions_written() {
    let pool = get_pool().await;

    let edit_id = uuid_from_bytes(expected::EDIT_A_1);

    let row: Option<(i64, i64, i64)> = sqlx::query_as(
        "SELECT block_number, sequence, version_key FROM edit_versions WHERE edit_id = $1",
    )
    .bind(edit_id)
    .fetch_optional(&pool)
    .await
    .expect("Failed to query edit_versions");

    let (block_number, sequence, version_key) = row.expect("Edit version should exist");
    assert_eq!(block_number, 0);
    assert_eq!(version_key, (block_number << 32) | sequence);
}

#[tokio::test]
async fn test_value_versions_written() {
    let pool = get_pool().await;

    let entity = uuid_from_bytes(expected::ENTITY_ORG_1);
    let property = uuid_from_bytes(expected::PROPERTY_NAME);
    let space = uuid_from_bytes(expected::SPACE_A);

    let row: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT valid_from_key, valid_to_key FROM value_versions WHERE entity_id = $1 AND property_id = $2 AND space_id = $3",
    )
    .bind(entity)
    .bind(property)
    .bind(space)
    .fetch_optional(&pool)
    .await
    .expect("Failed to query value_versions");

    let (valid_from_key, valid_to_key) = row.expect("Value version should exist");
    assert_eq!(valid_to_key, None);
    assert!(valid_from_key >= 0);
}

#[tokio::test]
async fn test_relation_versions_written() {
    let pool = get_pool().await;

    let relation_id = uuid_from_bytes(expected::RELATION_1);

    let row: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT valid_from_key, valid_to_key FROM relation_versions WHERE relation_id = $1",
    )
    .bind(relation_id)
    .fetch_optional(&pool)
    .await
    .expect("Failed to query relation_versions");

    let (valid_from_key, valid_to_key) = row.expect("Relation version should exist");
    assert_eq!(valid_to_key, None);
    assert!(valid_from_key >= 0);
}

#[tokio::test]
async fn test_context_metadata_written() {
    let pool = get_pool().await;

    let entity = uuid_from_bytes(expected::ENTITY_ORG_1);
    let property = uuid_from_bytes(expected::PROPERTY_NAME);
    let space = uuid_from_bytes(expected::SPACE_A);
    let context_root = uuid_from_bytes(expected::CONTEXT_ROOT_ID);
    let context_edge = uuid_from_bytes(expected::CONTEXT_EDGE_TYPE_ID);

    let row: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT context_root_id, context_edge_type_id FROM value_versions WHERE entity_id = $1 AND property_id = $2 AND space_id = $3 ORDER BY valid_from_key DESC LIMIT 1",
    )
    .bind(entity)
    .bind(property)
    .bind(space)
    .fetch_optional(&pool)
    .await
    .expect("Failed to query value_versions context metadata");

    let (context_root_id, context_edge_type_id) = row.expect("Context metadata should exist");
    assert_eq!(context_root_id, Some(context_root));
    assert_eq!(context_edge_type_id, Some(context_edge));
}
