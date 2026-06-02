//! End-to-end test runner for the notification service.
//!
//! Tests three editor-count scenarios:
//! - Space with 0 editors: events produce zero webhook calls
//! - Space with 1 editor: events produce 1 × webhooks calls
//! - Space with 3 editors: all 6 event types produce 3 × webhooks calls each
//!
//! Steps:
//! 1. Starts a mock webhook server
//! 2. Seeds the database (webhooks, editors across 3 spaces, expired proposals)
//! 3. Produces governance events to Kafka for all 3 spaces
//! 4. Waits for expected webhook calls
//! 5. Verifies correctness, fan-out counts, and absence of false positives

use std::collections::HashMap;
use std::env;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use prost::Message;
use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use sha2::Sha256;
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

// Space with 3 editors (full test — all 6 event types)
const SPACE_3E_BYTES: [u8; 16] = [0x01; 16];
const EDITOR_A_BYTES: [u8; 16] = [0xA0; 16];
const EDITOR_B_BYTES: [u8; 16] = [0xB0; 16];
const EDITOR_C_BYTES: [u8; 16] = [0xC0; 16];

// Space with 1 editor (PROPOSAL_CREATED only)
const SPACE_1E_BYTES: [u8; 16] = [0x11; 16];
const EDITOR_SOLO_BYTES: [u8; 16] = [0xD0; 16];

// Space with 0 editors (PROPOSAL_CREATED only — expect zero calls)
const SPACE_0E_BYTES: [u8; 16] = [0x22; 16];

const PROPOSER_ID_BYTES: [u8; 16] = [0x02; 16];
const VOTER_ID_BYTES: [u8; 16] = [0x0A; 16];
// A prior voter on the 3-editor space's UPDATED proposal who is NOT an editor —
// proves voter-targeted delivery of "a new version of a proposal you voted on".
const UPDATE_VOTER_BYTES: [u8; 16] = [0x0B; 16];

// Proposal IDs for 3-editor space (one per on-chain event type + rejection)
const PROP_3E_CREATED_BYTES: [u8; 16] = [0x03; 16];
const PROP_3E_UPDATED_BYTES: [u8; 16] = [0x06; 16];
const PROP_3E_VOTED_BYTES: [u8; 16] = [0x07; 16];
const PROP_3E_EXECUTED_BYTES: [u8; 16] = [0x04; 16];
const PROP_3E_SETTINGS_BYTES: [u8; 16] = [0x08; 16];
const PROP_3E_REJECTED_BYTES: [u8; 16] = [0x05; 16];

// Proposal IDs for 1-editor and 0-editor spaces
const PROP_1E_CREATED_BYTES: [u8; 16] = [0x31; 16];
const PROP_0E_CREATED_BYTES: [u8; 16] = [0x32; 16];

// Bounty test fixtures
// Bounty space with 2 editors (receives interest notifications)
const BOUNTY_SPACE_BYTES: [u8; 16] = [0xB1; 16];
const BOUNTY_EDITOR_1_BYTES: [u8; 16] = [0xB2; 16];
const BOUNTY_EDITOR_2_BYTES: [u8; 16] = [0xB3; 16];
// Curator's personal space (the person expressing interest / receiving allocation)
const CURATOR_SPACE_BYTES: [u8; 16] = [0xC1; 16];
const CURATOR_ENTITY_BYTES: [u8; 16] = [0xC2; 16];
// Bounty entity
const BOUNTY_ENTITY_BYTES: [u8; 16] = [0xBE; 16];
// Relation IDs
const INTEREST_RELATION_BYTES: [u8; 16] = [0xE1; 16];
const ALLOCATED_RELATION_BYTES: [u8; 16] = [0xE2; 16];
const PAYOUT_RELATION_BYTES: [u8; 16] = [0xE3; 16];

// Well-known bounty relation type UUIDs (must match models.rs)
const INTEREST_TYPE_BYTES: [u8; 16] = [
    0xff, 0x7e, 0x1b, 0x44, 0x44, 0xa2, 0x41, 0x91, 0x87, 0x32, 0x4e, 0x6c, 0x22, 0x2a, 0xfe, 0x07,
];
const ALLOCATED_TYPE_BYTES: [u8; 16] = [
    0xcf, 0xeb, 0x64, 0x22, 0x23, 0xc5, 0x4d, 0xf4, 0xb3, 0xf9, 0x37, 0x5a, 0x48, 0x9d, 0x9e, 0x22,
];
const PAYOUT_TYPE_BYTES: [u8; 16] = [
    0xfd, 0xda, 0xca, 0xae, 0x85, 0x13, 0x8a, 0x43, 0xec, 0x1a, 0x50, 0xff, 0x71, 0x56, 0x4d, 0x42,
];

// Phase 3a — bounty created: a NEW bounty entity created in the bounty space.
const NEW_BOUNTY_ENTITY_BYTES: [u8; 16] = [0xBC; 16];

// Phase 2a — proposal comment fixtures (isolated in their own space so the
// 3-editor space counts are unaffected).
const COMMENT_SPACE_BYTES: [u8; 16] = [0x71; 16];
const COMMENT_PROPOSAL_BYTES: [u8; 16] = [0x72; 16];
const COMMENT_PROPOSER_BYTES: [u8; 16] = [0x73; 16]; // recipient of the comment notification
const COMMENT_MEMBER_BYTES: [u8; 16] = [0x74; 16]; // member of COMMENT_SPACE (allowed commenter)
const COMMENT_NONMEMBER_BYTES: [u8; 16] = [0x75; 16]; // NOT a member (commenter must be filtered out)
const COMMENT_ENTITY_BYTES: [u8; 16] = [0x76; 16]; // the comment entity (allowed)
const COMMENT_ENTITY_2_BYTES: [u8; 16] = [0x78; 16]; // the comment entity (non-member, filtered)

// Phase 2b — general comment thread fixtures (a reply into a seeded thread).
const THREAD_ROOT_BYTES: [u8; 16] = [0x86; 16]; // the (non-proposal) thing being commented on
const THREAD_HOME_SPACE_BYTES: [u8; 16] = [0x87; 16]; // root's home space == creator (recipient)
const THREAD_GENERIC_TYPE_BYTES: [u8; 16] = [0x8F; 16]; // an arbitrary (non-bounty) type for the root
const EXISTING_COMMENT_BYTES: [u8; 16] = [0x88; 16]; // a prior comment on the root (seeded)
const THREAD_PARTICIPANT_SPACE_BYTES: [u8; 16] = [0x89; 16]; // author of the prior comment (recipient)
const REPLY_COMMENT_BYTES: [u8; 16] = [0x8a; 16]; // the NEW reply (produced)
const REPLY_AUTHOR_SPACE_BYTES: [u8; 16] = [0x8b; 16]; // author of the reply (must NOT be notified)

const GOVERNANCE_TOPIC: &str = "space.governance";
const KNOWLEDGE_EDITS_TOPIC: &str = "knowledge.edits";
const NUM_WEBHOOKS: usize = 3;

// Expected calls (3 webhooks each):
// 3-editor space (66):
//   proposal_created            3 editors                  × 3 = 9
//   proposal_updated            3 editors + 1 prior voter  × 3 = 12
//   proposal_voted              3 editors + proposer       × 3 = 12
//   proposal_executed           3 editors + proposer       × 3 = 12
//   proposal_settings_updated   3 editors                  × 3 = 9
//   proposal_rejected           3 editors + proposer       × 3 = 12
// 1-editor space: 1 event type  × 1 editor  × 3 = 3
// 0-editor space: 1 event type  × 0 editors × 3 = 0
// Bounty interest:  1 event × 2 editors × 3 = 6
// Bounty allocated: 1 event × 1 curator × 3 = 3
// Bounty payout:    1 event × 1 curator × 3 = 3
// Bounty created:   1 event × 2 bounty editors × 3 = 6   (Phase 3a)
// Proposal comment: 1 event × 1 proposer × 3 = 3         (Phase 2a; the non-member
//                   comment is filtered out and produces 0 calls)
// Comment thread:   1 reply × 2 recipients × 3 = 6       (Phase 2b; prior participant
//                   + root creator; the reply author is excluded)
//
// The proposer (0x02), prior voter (0x0B), and comment proposer (0x73) are
// intentionally NOT editors, so these counts also prove targeted delivery
// reaches non-editors.
const EXPECTED_CALLS: usize = 66 + 3 + 6 + 3 + 3 + 6 + 3 + 6;

const GOVERNANCE_EVENT_TYPES: &[&str] = &[
    "proposal_created",
    "proposal_updated",
    "proposal_voted",
    "proposal_executed",
    "proposal_settings_updated",
    "proposal_rejected",
];

const ALL_EVENT_TYPES: &[&str] = &[
    "proposal_created",
    "proposal_updated",
    "proposal_voted",
    "proposal_executed",
    "proposal_settings_updated",
    "proposal_rejected",
    "bounty_interest",
    "bounty_allocated",
    "bounty_payout",
    "bounty_created",
    "proposal_comment",
    "comment",
];

// ---------------------------------------------------------------------------
// Webhook call capture
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct WebhookCall {
    body: serde_json::Value,
    raw_body: Vec<u8>,
    signature: String,
    idempotency_key: String,
}

async fn webhook_handler(
    State(tx): State<mpsc::UnboundedSender<WebhookCall>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("x-geo-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body_json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let idempotency_key = body_json["idempotency_key"]
        .as_str()
        .unwrap_or("")
        .to_string();

    tx.send(WebhookCall {
        body: body_json,
        raw_body: body.to_vec(),
        signature,
        idempotency_key,
    })
    .ok();

    StatusCode::OK
}

// ---------------------------------------------------------------------------
// HMAC verification
// ---------------------------------------------------------------------------

fn verify_hmac(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let prefix = "sha256=";
    if !signature_header.starts_with(prefix) {
        return false;
    }
    let received_hex = &signature_header[prefix.len()..];
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes()) == received_hex
}

// ---------------------------------------------------------------------------
// Database seeding
// ---------------------------------------------------------------------------

async fn seed_database(
    pool: &PgPool,
    webhook_port: u16,
    webhook_secret: &str,
) -> Result<(), sqlx::Error> {
    // Register webhooks
    for i in 0..NUM_WEBHOOKS {
        sqlx::query(
            "INSERT INTO app_webhooks (app_name, url, secret) VALUES ($1, $2, $3) \
             ON CONFLICT (app_name) DO NOTHING",
        )
        .bind(format!("e2e-app-{}", i))
        .bind(format!("http://localhost:{}/webhook", webhook_port))
        .bind(webhook_secret)
        .execute(pool)
        .await?;
    }

    // 3-editor space
    let space_3e = Uuid::from_bytes(SPACE_3E_BYTES);
    for bytes in [EDITOR_A_BYTES, EDITOR_B_BYTES, EDITOR_C_BYTES] {
        sqlx::query(
            "INSERT INTO editors (member_space_id, space_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::from_bytes(bytes))
        .bind(space_3e)
        .execute(pool)
        .await?;
    }

    // 1-editor space
    let space_1e = Uuid::from_bytes(SPACE_1E_BYTES);
    sqlx::query(
        "INSERT INTO editors (member_space_id, space_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::from_bytes(EDITOR_SOLO_BYTES))
    .bind(space_1e)
    .execute(pool)
    .await?;

    // 0-editor space: no rows inserted

    // Expired proposals for rejection polling (one per space that has editors)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs() as i64;
    let proposer = Uuid::from_bytes(PROPOSER_ID_BYTES);

    sqlx::query(
        "INSERT INTO proposals (id, space_id, proposed_by, start_time, end_time, created_at, created_at_block) \
         VALUES ($1, $2, $3, $4, $5, '0', '0') ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::from_bytes(PROP_3E_REJECTED_BYTES))
    .bind(space_3e)
    .bind(proposer)
    .bind(now - 7200)
    .bind(now - 3600)
    .execute(pool)
    .await?;

    // Proposals for the VOTED and EXECUTED events so the indexer can resolve the
    // proposer (find_proposer_for_proposal) and deliver "your proposal was voted
    // on / approved" to them. end_time is in the FUTURE so the rejection poller
    // does NOT also reject these (it only rejects end_time < now).
    for prop in [PROP_3E_VOTED_BYTES, PROP_3E_EXECUTED_BYTES] {
        sqlx::query(
            "INSERT INTO proposals (id, space_id, proposed_by, start_time, end_time, created_at, created_at_block) \
             VALUES ($1, $2, $3, $4, $5, '0', '0') ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::from_bytes(prop))
        .bind(space_3e)
        .bind(proposer)
        .bind(now - 3600)
        .bind(now + 3600)
        .execute(pool)
        .await?;
    }

    // A prior vote on the UPDATED proposal by a NON-editor, so the indexer
    // delivers "a new version of a proposal you voted on" to that voter.
    sqlx::query(
        "INSERT INTO proposal_votes (proposal_id, voter_id, space_id, vote) \
         VALUES ($1, $2, $3, 'yes') ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::from_bytes(PROP_3E_UPDATED_BYTES))
    .bind(Uuid::from_bytes(UPDATE_VOTER_BYTES))
    .bind(space_3e)
    .execute(pool)
    .await?;

    // Bounty space with 2 editors
    let bounty_space = Uuid::from_bytes(BOUNTY_SPACE_BYTES);
    for bytes in [BOUNTY_EDITOR_1_BYTES, BOUNTY_EDITOR_2_BYTES] {
        sqlx::query(
            "INSERT INTO editors (member_space_id, space_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::from_bytes(bytes))
        .bind(bounty_space)
        .execute(pool)
        .await?;
    }

    // Seed the bounty entity's Types relation (from=bounty, to=BOUNTY_TYPE)
    // so lookup_bounty_space can resolve BOUNTY_ENTITY -> BOUNTY_SPACE.
    // TYPE_RELATION_TYPE_ID = 8f151ba4-de20-4e3c-9cb4-99ddf96f48f1
    // BOUNTY_TYPE_ID = 808af0ba-d588-4e33-91f0-9dd4b25e18be
    sqlx::query(
        "INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id) \
         VALUES ($1, $2, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, $3, '808af0ba-d588-4e33-91f0-9dd4b25e18be'::uuid) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(bounty_space)
    .bind(Uuid::from_bytes(BOUNTY_ENTITY_BYTES))
    .execute(pool)
    .await?;

    // Decoy: a Types relation from the same bounty entity pointing to a DIFFERENT type.
    // This must NOT be matched by lookup_bounty_space (tests the to_entity_id constraint).
    let wrong_space = Uuid::from_bytes([0xBB; 16]);
    sqlx::query(
        "INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id) \
         VALUES ($1, $2, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, $3, 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'::uuid) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(wrong_space)
    .bind(Uuid::from_bytes(BOUNTY_ENTITY_BYTES))
    .execute(pool)
    .await?;

    // Decoy: a non-Types relation from the same bounty entity.
    // This must NOT be matched by lookup_bounty_space (tests the type_id constraint).
    sqlx::query(
        "INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id) \
         VALUES ($1, $2, 'cccccccc-cccc-cccc-cccc-cccccccccccc'::uuid, $3, '808af0ba-d588-4e33-91f0-9dd4b25e18be'::uuid) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(wrong_space)
    .bind(Uuid::from_bytes(BOUNTY_ENTITY_BYTES))
    .execute(pool)
    .await?;

    // Seed space rows (needed for lookup_entity_space JOIN)
    sqlx::query("INSERT INTO spaces (id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(bounty_space)
        .execute(pool)
        .await?;

    // Curator personal space (for allocated/payout resolution)
    let curator_space = Uuid::from_bytes(CURATOR_SPACE_BYTES);
    sqlx::query("INSERT INTO spaces (id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(curator_space)
        .execute(pool)
        .await?;

    // Seed curator entity→space relation (from=curator_entity, to=SPACE_TYPE, space=curator_space)
    // SPACE_TYPE = 362c1dbd-dc64-44bb-a3c4-652f38a642d7
    sqlx::query(
        "INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id) \
         VALUES ($1, $2, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, $3, '362c1dbd-dc64-44bb-a3c4-652f38a642d7'::uuid) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(curator_space)
    .bind(Uuid::from_bytes(CURATOR_ENTITY_BYTES))
    .execute(pool)
    .await?;

    // Decoy: curator entity with a non-SPACE_TYPE to_entity in a different space.
    // Must NOT be matched by lookup_entity_space.
    sqlx::query(
        "INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id) \
         VALUES ($1, $2, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, $3, 'dddddddd-dddd-dddd-dddd-dddddddddddd'::uuid) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(wrong_space)
    .bind(Uuid::from_bytes(CURATOR_ENTITY_BYTES))
    .execute(pool)
    .await?;

    // Add curator as editor of their own space (for single-user notifications)
    sqlx::query(
        "INSERT INTO editors (member_space_id, space_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(curator_space)
    .bind(curator_space)
    .execute(pool)
    .await?;

    // Phase 2a: a proposal in its own space for the comment test. end_time in the
    // future so the rejection poller ignores it. The recipient is the proposer.
    sqlx::query(
        "INSERT INTO proposals (id, space_id, proposed_by, start_time, end_time, created_at, created_at_block) \
         VALUES ($1, $2, $3, $4, $5, '0', '0') ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::from_bytes(COMMENT_PROPOSAL_BYTES))
    .bind(Uuid::from_bytes(COMMENT_SPACE_BYTES))
    .bind(Uuid::from_bytes(COMMENT_PROPOSER_BYTES))
    .bind(now - 3600)
    .bind(now + 3600)
    .execute(pool)
    .await?;

    // The allowed commenter is a *member* (not editor) of the proposal's space,
    // exercising the members branch of is_member_or_editor.
    sqlx::query(
        "INSERT INTO members (member_space_id, space_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::from_bytes(COMMENT_MEMBER_BYTES))
    .bind(Uuid::from_bytes(COMMENT_SPACE_BYTES))
    .execute(pool)
    .await?;

    // Phase 2b: seed an existing comment thread. The notification-indexer reads
    // these relations to resolve the thread root and its participants (kg-indexer
    // isn't running in the e2e, so we seed the relations directly).
    // (a) The thread root's Types relation → gives it a home space (== creator).
    sqlx::query(
        "INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id) \
         VALUES ($1, $2, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, $3, $4) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::from_bytes(THREAD_HOME_SPACE_BYTES))
    .bind(Uuid::from_bytes(THREAD_ROOT_BYTES))
    .bind(Uuid::from_bytes(THREAD_GENERIC_TYPE_BYTES))
    .execute(pool)
    .await?;
    // (b) A prior comment on the root, authored from a participant's space
    //     (Reply-to → root). Its space_id is that participant.
    sqlx::query(
        "INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id) \
         VALUES ($1, $2, '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid, $3, $4) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::from_bytes(THREAD_PARTICIPANT_SPACE_BYTES))
    .bind(Uuid::from_bytes(EXISTING_COMMENT_BYTES))
    .bind(Uuid::from_bytes(THREAD_ROOT_BYTES))
    .execute(pool)
    .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Kafka event production
// ---------------------------------------------------------------------------

fn make_meta(
    block_number: u64,
    created_at: u64,
) -> hermes_schema::pb::blockchain_metadata::BlockchainMetadata {
    hermes_schema::pb::blockchain_metadata::BlockchainMetadata {
        created_at,
        created_by: vec![],
        block_number,
        cursor: String::new(),
        sequence: 0,
        is_last: false,
    }
}

fn make_settings() -> hermes_schema::pb::governance::ProposalSettings {
    hermes_schema::pb::governance::ProposalSettings {
        start_date: 1700000000,
        last_date: 1700086400,
        voting_mode: 0,
        quorum: 1,
        flat_threshold: 1,
        percentage_threshold: 0,
    }
}

async fn send_event(
    producer: &FutureProducer,
    event_type: &str,
    payload: &[u8],
    key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let headers = OwnedHeaders::new().insert(Header {
        key: "event-type",
        value: Some(event_type.as_bytes()),
    });
    producer
        .send(
            FutureRecord::to(GOVERNANCE_TOPIC)
                .payload(payload)
                .key(key)
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;
    Ok(())
}

async fn produce_test_events(kafka_broker: &str) -> Result<(), Box<dyn std::error::Error>> {
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", kafka_broker)
        .set("message.timeout.ms", "30000")
        .create()?;

    // === 3-editor space: all 5 on-chain event types ===

    let msg = hermes_schema::pb::governance::HermesProposalCreated {
        space_id: SPACE_3E_BYTES.to_vec(),
        proposer_id: PROPOSER_ID_BYTES.to_vec(),
        proposal_id: PROP_3E_CREATED_BYTES.to_vec(),
        voting_mode: 0,
        actions: vec![],
        settings: Some(make_settings()),
        meta: Some(make_meta(12345, 1700000000)),
    };
    send_event(
        &producer,
        "PROPOSAL_CREATED",
        &msg.encode_to_vec(),
        "3e-created",
    )
    .await?;

    let msg = hermes_schema::pb::governance::HermesProposalUpdated {
        space_id: SPACE_3E_BYTES.to_vec(),
        proposer_id: PROPOSER_ID_BYTES.to_vec(),
        proposal_id: PROP_3E_UPDATED_BYTES.to_vec(),
        voting_mode: 0,
        actions: vec![],
        settings: Some(make_settings()),
        meta: Some(make_meta(12347, 1700002000)),
    };
    send_event(
        &producer,
        "PROPOSAL_UPDATED",
        &msg.encode_to_vec(),
        "3e-updated",
    )
    .await?;

    let msg = hermes_schema::pb::governance::HermesProposalVoted {
        voter_id: VOTER_ID_BYTES.to_vec(),
        space_id: SPACE_3E_BYTES.to_vec(),
        proposal_id: PROP_3E_VOTED_BYTES.to_vec(),
        vote: hermes_schema::pb::governance::ProposalVoteOption::VoteOptionYes as i32,
        meta: Some(make_meta(12348, 1700003000)),
    };
    send_event(
        &producer,
        "PROPOSAL_VOTED",
        &msg.encode_to_vec(),
        "3e-voted",
    )
    .await?;

    let msg = hermes_schema::pb::governance::HermesProposalExecuted {
        space_id: SPACE_3E_BYTES.to_vec(),
        proposal_id: PROP_3E_EXECUTED_BYTES.to_vec(),
        meta: Some(make_meta(12346, 1700001000)),
    };
    send_event(
        &producer,
        "PROPOSAL_EXECUTED",
        &msg.encode_to_vec(),
        "3e-executed",
    )
    .await?;

    let msg = hermes_schema::pb::governance::HermesProposalSettingsUpdated {
        space_id: SPACE_3E_BYTES.to_vec(),
        proposal_id: PROP_3E_SETTINGS_BYTES.to_vec(),
        settings: Some(hermes_schema::pb::governance::ProposalSettings {
            start_date: 1700000000,
            last_date: 1700172800,
            voting_mode: 1,
            quorum: 5,
            flat_threshold: 0,
            percentage_threshold: 5000000,
        }),
        meta: Some(make_meta(12349, 1700004000)),
    };
    send_event(
        &producer,
        "PROPOSAL_SETTINGS_UPDATED",
        &msg.encode_to_vec(),
        "3e-settings",
    )
    .await?;

    // === 1-editor space: PROPOSAL_CREATED only ===

    let msg = hermes_schema::pb::governance::HermesProposalCreated {
        space_id: SPACE_1E_BYTES.to_vec(),
        proposer_id: PROPOSER_ID_BYTES.to_vec(),
        proposal_id: PROP_1E_CREATED_BYTES.to_vec(),
        voting_mode: 0,
        actions: vec![],
        settings: Some(make_settings()),
        meta: Some(make_meta(20001, 1700010000)),
    };
    send_event(
        &producer,
        "PROPOSAL_CREATED",
        &msg.encode_to_vec(),
        "1e-created",
    )
    .await?;

    // === 0-editor space: PROPOSAL_CREATED only (expect zero calls) ===

    let msg = hermes_schema::pb::governance::HermesProposalCreated {
        space_id: SPACE_0E_BYTES.to_vec(),
        proposer_id: PROPOSER_ID_BYTES.to_vec(),
        proposal_id: PROP_0E_CREATED_BYTES.to_vec(),
        voting_mode: 0,
        actions: vec![],
        settings: Some(make_settings()),
        meta: Some(make_meta(30001, 1700020000)),
    };
    send_event(
        &producer,
        "PROPOSAL_CREATED",
        &msg.encode_to_vec(),
        "0e-created",
    )
    .await?;

    // === Bounty events via knowledge.edits topic ===

    let ke_topic = {
        let prefix = hermes_kafka::get_topic_prefix();
        format!("{}{}", prefix, KNOWLEDGE_EDITS_TOPIC)
    };

    // Interest: curator (in their personal space) → bounty entity
    let interest_edit = make_bounty_hermes_edit(
        INTEREST_RELATION_BYTES,
        INTEREST_TYPE_BYTES,
        CURATOR_ENTITY_BYTES,
        BOUNTY_ENTITY_BYTES,
        CURATOR_SPACE_BYTES,
        None,
        40001,
        1700030000,
        1,
    );
    let headers = rdkafka::message::OwnedHeaders::new();
    producer
        .send(
            FutureRecord::to(&ke_topic)
                .payload(&interest_edit.encode_to_vec())
                .key("bounty-interest")
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    // Allocated: bounty entity → curator, in bounty space
    let allocated_edit = make_bounty_hermes_edit(
        ALLOCATED_RELATION_BYTES,
        ALLOCATED_TYPE_BYTES,
        BOUNTY_ENTITY_BYTES,
        CURATOR_ENTITY_BYTES,
        BOUNTY_SPACE_BYTES,
        Some(CURATOR_SPACE_BYTES),
        40002,
        1700031000,
        2,
    );
    let headers = rdkafka::message::OwnedHeaders::new();
    producer
        .send(
            FutureRecord::to(&ke_topic)
                .payload(&allocated_edit.encode_to_vec())
                .key("bounty-allocated")
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    // Payout: bounty space → curator space
    let payout_edit = make_bounty_hermes_edit(
        PAYOUT_RELATION_BYTES,
        PAYOUT_TYPE_BYTES,
        BOUNTY_ENTITY_BYTES,
        CURATOR_ENTITY_BYTES,
        BOUNTY_SPACE_BYTES,
        Some(CURATOR_SPACE_BYTES),
        40003,
        1700032000,
        3,
    );
    let headers = rdkafka::message::OwnedHeaders::new();
    producer
        .send(
            FutureRecord::to(&ke_topic)
                .payload(&payout_edit.encode_to_vec())
                .key("bounty-payout")
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    // Phase 3a: a NEW bounty created in the bounty space (entity → Types → Bounty).
    let types_bytes = Uuid::parse_str("8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")
        .expect("valid Types id")
        .into_bytes();
    let bounty_type_bytes = Uuid::parse_str("808af0ba-d588-4e33-91f0-9dd4b25e18be")
        .expect("valid Bounty type id")
        .into_bytes();
    let bounty_created_edit = make_bounty_hermes_edit(
        [0xBD; 16],
        types_bytes,
        NEW_BOUNTY_ENTITY_BYTES,
        bounty_type_bytes,
        BOUNTY_SPACE_BYTES,
        None,
        40004,
        1700033000,
        4,
    );
    let headers = rdkafka::message::OwnedHeaders::new();
    producer
        .send(
            FutureRecord::to(&ke_topic)
                .payload(&bounty_created_edit.encode_to_vec())
                .key("bounty-created")
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    // Phase 2a: a comment on COMMENT_PROPOSAL by a member (allowed → proposer
    // notified) and by a non-member (must be filtered out → no notification).
    let comment_allowed = make_comment_hermes_edit(
        COMMENT_ENTITY_BYTES,
        COMMENT_PROPOSAL_BYTES,
        COMMENT_MEMBER_BYTES,
        40005,
        1700034000,
        5,
    );
    let headers = rdkafka::message::OwnedHeaders::new();
    producer
        .send(
            FutureRecord::to(&ke_topic)
                .payload(&comment_allowed.encode_to_vec())
                .key("comment-allowed")
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    let comment_blocked = make_comment_hermes_edit(
        COMMENT_ENTITY_2_BYTES,
        COMMENT_PROPOSAL_BYTES,
        COMMENT_NONMEMBER_BYTES,
        40006,
        1700035000,
        6,
    );
    let headers = rdkafka::message::OwnedHeaders::new();
    producer
        .send(
            FutureRecord::to(&ke_topic)
                .payload(&comment_blocked.encode_to_vec())
                .key("comment-blocked")
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    // Phase 2b: a reply into the seeded thread (reply → EXISTING_COMMENT). The
    // indexer walks to the root, notifying the prior participant + the root's
    // creator (home space), but NOT the reply's own author.
    let thread_reply = make_comment_hermes_edit(
        REPLY_COMMENT_BYTES,
        EXISTING_COMMENT_BYTES,
        REPLY_AUTHOR_SPACE_BYTES,
        40007,
        1700036000,
        7,
    );
    let headers = rdkafka::message::OwnedHeaders::new();
    producer
        .send(
            FutureRecord::to(&ke_topic)
                .payload(&thread_reply.encode_to_vec())
                .key("comment-thread-reply")
                .headers(headers),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    Ok(())
}

/// Build a HermesEdit protobuf containing a single CreateRelation op.
#[allow(clippy::too_many_arguments)]
fn make_bounty_hermes_edit(
    relation_id: [u8; 16],
    relation_type: [u8; 16],
    from: [u8; 16],
    to: [u8; 16],
    space_id: [u8; 16],
    to_space: Option<[u8; 16]>,
    block_number: u64,
    created_at: u64,
    sequence: u32,
) -> hermes_schema::pb::knowledge::HermesEdit {
    use std::borrow::Cow;

    let edit = grc_20::Edit {
        id: relation_id, // reuse relation_id as edit id for simplicity
        name: Cow::Borrowed("bounty edit"),
        authors: vec![from],
        created_at: created_at as i64,
        ops: vec![grc_20::Op::CreateRelation(grc_20::CreateRelation {
            id: relation_id,
            relation_type,
            from,
            from_is_value_ref: false,
            to,
            to_is_value_ref: false,
            from_space: None,
            from_version: None,
            to_space,
            to_version: None,
            entity: None,
            position: None,
            context: None,
        })],
    };
    let payload = grc_20::encode_edit(&edit).expect("GRC-20 encode should succeed");

    hermes_schema::pb::knowledge::HermesEdit {
        id: relation_id.to_vec(),
        name: "bounty edit".into(),
        payload,
        authors: vec![from.to_vec()],
        language: None,
        space_id: space_id.to_vec(),
        is_canonical: true,
        meta: Some(hermes_schema::pb::blockchain_metadata::BlockchainMetadata {
            created_at,
            created_by: vec![],
            block_number,
            cursor: String::new(),
            sequence,
            is_last: false,
        }),
    }
}

/// Build a HermesEdit for a comment: a Comment entity (`Types → Comment`) that
/// replies to `parent` (`Reply to → parent`), published from `commenter_space`.
fn make_comment_hermes_edit(
    comment_entity: [u8; 16],
    parent: [u8; 16],
    commenter_space: [u8; 16],
    block_number: u64,
    created_at: u64,
    sequence: u32,
) -> hermes_schema::pb::knowledge::HermesEdit {
    use std::borrow::Cow;

    let types = Uuid::parse_str("8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")
        .expect("valid Types id")
        .into_bytes();
    let comment_type = Uuid::parse_str("82f6123a-0323-4c6c-a811-701c5bc026e9")
        .expect("valid Comment type id")
        .into_bytes();
    let reply_to = Uuid::parse_str("310d4a24-0e5b-451c-b215-1bfce40d0fe6")
        .expect("valid Reply-to id")
        .into_bytes();

    let mk = |id: [u8; 16], rt: [u8; 16], from: [u8; 16], to: [u8; 16]| {
        grc_20::Op::CreateRelation(grc_20::CreateRelation {
            id,
            relation_type: rt,
            from,
            from_is_value_ref: false,
            to,
            to_is_value_ref: false,
            from_space: None,
            from_version: None,
            to_space: None,
            to_version: None,
            entity: None,
            position: None,
            context: None,
        })
    };

    let edit = grc_20::Edit {
        id: comment_entity,
        name: Cow::Borrowed("comment edit"),
        authors: vec![commenter_space],
        created_at: created_at as i64,
        ops: vec![
            // Comment entity is typed as Comment...
            mk([0xA1; 16], types, comment_entity, comment_type),
            // ...and replies to its parent.
            mk([0xA2; 16], reply_to, comment_entity, parent),
        ],
    };
    let payload = grc_20::encode_edit(&edit).expect("GRC-20 encode should succeed");

    hermes_schema::pb::knowledge::HermesEdit {
        id: comment_entity.to_vec(),
        name: "comment edit".into(),
        payload,
        authors: vec![commenter_space.to_vec()],
        language: None,
        space_id: commenter_space.to_vec(),
        is_canonical: true,
        meta: Some(hermes_schema::pb::blockchain_metadata::BlockchainMetadata {
            created_at,
            created_by: vec![],
            block_number,
            cursor: String::new(),
            sequence,
            is_last: false,
        }),
    }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

struct TestResults {
    passed: u32,
    failed: u32,
}

impl TestResults {
    fn new() -> Self {
        Self {
            passed: 0,
            failed: 0,
        }
    }

    fn check(&mut self, name: &str, ok: bool) {
        if ok {
            println!("  PASS: {}", name);
            self.passed += 1;
        } else {
            eprintln!("  FAIL: {}", name);
            self.failed += 1;
        }
    }
}

fn verify_calls(calls: &[WebhookCall], webhook_secret: &str) -> TestResults {
    let mut r = TestResults::new();

    let space_3e = Uuid::from_bytes(SPACE_3E_BYTES).to_string();
    let space_1e = Uuid::from_bytes(SPACE_1E_BYTES).to_string();
    let space_0e = Uuid::from_bytes(SPACE_0E_BYTES).to_string();

    let editors_3e: Vec<String> = [EDITOR_A_BYTES, EDITOR_B_BYTES, EDITOR_C_BYTES]
        .iter()
        .map(|b| Uuid::from_bytes(*b).to_string())
        .collect();
    let editor_solo = Uuid::from_bytes(EDITOR_SOLO_BYTES).to_string();
    // Phase 1 targeted recipients (intentionally NOT editors):
    let proposer_uid = Uuid::from_bytes(PROPOSER_ID_BYTES).to_string();
    let update_voter_uid = Uuid::from_bytes(UPDATE_VOTER_BYTES).to_string();

    // ===================================================================
    // Total count
    // ===================================================================
    r.check(
        &format!("total calls is exactly {}", EXPECTED_CALLS),
        calls.len() == EXPECTED_CALLS,
    );

    // ===================================================================
    // 0-editor space: zero calls
    // ===================================================================
    let calls_0e: Vec<_> = calls
        .iter()
        .filter(|c| c.body["space_id"].as_str() == Some(space_0e.as_str()))
        .collect();
    r.check("0-editor space: zero webhook calls", calls_0e.is_empty());

    // ===================================================================
    // 1-editor space: 1 event × 1 editor × 3 webhooks = 3 calls
    // ===================================================================
    let calls_1e: Vec<_> = calls
        .iter()
        .filter(|c| c.body["space_id"].as_str() == Some(space_1e.as_str()))
        .collect();
    r.check(
        "1-editor space: exactly 3 calls (1 event x 1 editor x 3 webhooks)",
        calls_1e.len() == 3,
    );
    // All should be proposal_created
    r.check(
        "1-editor space: all calls are proposal_created",
        calls_1e
            .iter()
            .all(|c| c.body["event_type"].as_str() == Some("proposal_created")),
    );
    // All should have the solo editor's user_space_id
    r.check(
        "1-editor space: all calls have correct user_space_id",
        calls_1e
            .iter()
            .all(|c| c.body["user_space_id"].as_str() == Some(editor_solo.as_str())),
    );
    // Correct proposal_id
    let prop_1e = Uuid::from_bytes(PROP_1E_CREATED_BYTES).to_string();
    r.check(
        "1-editor space: correct proposal_id",
        calls_1e
            .iter()
            .all(|c| c.body["proposal_id"].as_str() == Some(prop_1e.as_str())),
    );
    // HMAC valid
    if let Some(call) = calls_1e.first() {
        r.check(
            "1-editor space: valid HMAC signature",
            verify_hmac(webhook_secret, &call.raw_body, &call.signature),
        );
    }

    // ===================================================================
    // 3-editor space: 66 calls (editors on every event + targeted recipients)
    // ===================================================================
    let calls_3e: Vec<_> = calls
        .iter()
        .filter(|c| c.body["space_id"].as_str() == Some(space_3e.as_str()))
        .collect();
    r.check(
        "3-editor space: exactly 66 calls (editors + proposer/voter targeting)",
        calls_3e.len() == 66,
    );

    // Per governance event-type fan-out (3-editor space). Targeted events add one
    // non-editor recipient: the proposer (voted/executed/rejected) or a prior
    // voter (updated).
    for et in GOVERNANCE_EVENT_TYPES {
        let et_calls: Vec<_> = calls_3e
            .iter()
            .filter(|c| c.body["event_type"].as_str() == Some(et))
            .collect();
        let expected_recipients = match *et {
            "proposal_voted" | "proposal_executed" | "proposal_rejected" => editors_3e.len() + 1, // + proposer
            "proposal_updated" => editors_3e.len() + 1, // + prior voter
            _ => editors_3e.len(),
        };
        r.check(
            &format!("3e {}: {} calls", et, expected_recipients * NUM_WEBHOOKS),
            et_calls.len() == expected_recipients * NUM_WEBHOOKS,
        );
        // Each editor still gets exactly 3 (one per webhook) on every event type.
        for editor_str in &editors_3e {
            let n = et_calls
                .iter()
                .filter(|c| c.body["user_space_id"].as_str() == Some(editor_str.as_str()))
                .count();
            r.check(
                &format!("3e {}: editor {}.. got 3 deliveries", et, &editor_str[..8]),
                n == 3,
            );
        }
    }

    // Phase 1: targeted recipients beyond editors reach non-editors.
    for et in ["proposal_voted", "proposal_executed", "proposal_rejected"] {
        let n = calls_3e
            .iter()
            .filter(|c| {
                c.body["event_type"].as_str() == Some(et)
                    && c.body["user_space_id"].as_str() == Some(proposer_uid.as_str())
            })
            .count();
        r.check(
            &format!("3e {}: proposer (non-editor) notified on 3 webhooks", et),
            n == 3,
        );
    }
    let upd_voter_calls = calls_3e
        .iter()
        .filter(|c| {
            c.body["event_type"].as_str() == Some("proposal_updated")
                && c.body["user_space_id"].as_str() == Some(update_voter_uid.as_str())
        })
        .count();
    r.check(
        "3e proposal_updated: prior voter (non-editor) notified on 3 webhooks",
        upd_voter_calls == 3,
    );

    // ===================================================================
    // Every call has user_space_id and it's a known editor
    // ===================================================================
    let bounty_editor_1 = Uuid::from_bytes(BOUNTY_EDITOR_1_BYTES).to_string();
    let bounty_editor_2 = Uuid::from_bytes(BOUNTY_EDITOR_2_BYTES).to_string();
    let curator_space = Uuid::from_bytes(CURATOR_SPACE_BYTES).to_string();
    // Phase 2a: the proposal-comment recipient (the proposer; not an editor).
    let comment_proposer = Uuid::from_bytes(COMMENT_PROPOSER_BYTES).to_string();
    // Phase 2b: comment-thread recipients (prior participant + root creator).
    let thread_participant = Uuid::from_bytes(THREAD_PARTICIPANT_SPACE_BYTES).to_string();
    let thread_home = Uuid::from_bytes(THREAD_HOME_SPACE_BYTES).to_string();

    let all_known: Vec<String> = editors_3e
        .iter()
        .chain(std::iter::once(&editor_solo))
        .chain(std::iter::once(&bounty_editor_1))
        .chain(std::iter::once(&bounty_editor_2))
        .chain(std::iter::once(&curator_space))
        // Phase 1 targeted recipients (non-editors):
        .chain(std::iter::once(&proposer_uid))
        .chain(std::iter::once(&update_voter_uid))
        // Phase 2a: proposal-comment recipient (proposer):
        .chain(std::iter::once(&comment_proposer))
        // Phase 2b: comment-thread recipients (participant + root creator):
        .chain(std::iter::once(&thread_participant))
        .chain(std::iter::once(&thread_home))
        .cloned()
        .collect();

    let all_have_uid = calls
        .iter()
        .all(|c| c.body["user_space_id"].as_str().is_some());
    r.check("all calls have user_space_id", all_have_uid);

    let no_unknown = calls.iter().all(|c| {
        c.body["user_space_id"]
            .as_str()
            .is_some_and(|id| all_known.iter().any(|k| k == id))
    });
    r.check("no unexpected user_space_ids (false positive)", no_unknown);

    // ===================================================================
    // No unexpected event types
    // ===================================================================
    let known_types: std::collections::HashSet<&str> = ALL_EVENT_TYPES.iter().copied().collect();
    r.check(
        "no unexpected event types",
        calls.iter().all(|c| {
            c.body["event_type"]
                .as_str()
                .is_some_and(|t| known_types.contains(t))
        }),
    );

    // ===================================================================
    // HMAC signatures (spot-check one per event type)
    // ===================================================================
    // Governance HMAC spot-checks
    for et in &[
        "proposal_created",
        "proposal_updated",
        "proposal_voted",
        "proposal_executed",
        "proposal_settings_updated",
        "proposal_rejected",
    ] {
        if let Some(call) = calls_3e
            .iter()
            .find(|c| c.body["event_type"].as_str() == Some(et))
        {
            r.check(
                &format!("3e {}: valid HMAC", et),
                verify_hmac(webhook_secret, &call.raw_body, &call.signature),
            );
        }
    }
    // Bounty HMAC spot-checks
    for et in &["bounty_interest", "bounty_allocated", "bounty_payout"] {
        if let Some(call) = calls
            .iter()
            .find(|c| c.body["event_type"].as_str() == Some(et))
        {
            r.check(
                &format!("bounty {}: valid HMAC", et),
                verify_hmac(webhook_secret, &call.raw_body, &call.signature),
            );
        }
    }

    // ===================================================================
    // All calls have idempotency key (raw string format: base:user_space_id)
    // ===================================================================
    r.check(
        "all calls have idempotency key",
        calls.iter().all(|c| !c.idempotency_key.is_empty()),
    );
    r.check(
        "all idempotency keys contain colon-separated components",
        calls.iter().all(|c| {
            // Raw format: "{block}:{sequence}:{event_type}:{user_space_id}"
            // or for rejections: "{proposal_id}:proposal_rejected:{user_space_id}"
            c.idempotency_key.contains(':') && c.idempotency_key.len() > 10
        }),
    );
    // Each unique (event_type, user_space_id) pair should have a unique key.
    // Multiple webhooks receive the same key for the same user — that's correct.
    // So the number of distinct keys should equal events × users, not total calls.
    r.check("idempotency keys are unique per (event, user) pair", {
        let keys: std::collections::HashSet<&str> =
            calls.iter().map(|c| c.idempotency_key.as_str()).collect();
        // 3-editor space: per (event, user) pair —
        //   created 3 + settings 3 + updated 4 + voted 4 + executed 4 + rejected 4 = 22
        // 1-editor space: 1 event × 1 editor = 1
        // Bounty interest: 1 event × 2 editors = 2
        // Bounty allocated: 1 event × 1 curator = 1
        // Bounty payout: 1 event × 1 curator = 1
        // Bounty created: 1 event × 2 bounty editors = 2
        // Proposal comment: 1 event × 1 proposer = 1
        // Comment thread: 1 reply × 2 recipients = 2
        // Total: 32
        keys.len() == 32
    });

    // ===================================================================
    // Comprehensive per-event-type payload validation
    // ===================================================================

    let proposer = Uuid::from_bytes(PROPOSER_ID_BYTES).to_string();
    let voter = Uuid::from_bytes(VOTER_ID_BYTES).to_string();

    // Helper: validate common fields present on every notification
    let check_common = |r: &mut TestResults,
                        prefix: &str,
                        call: &WebhookCall,
                        expected_space: &str,
                        expected_block: Option<u64>,
                        expected_ts: Option<u64>| {
        r.check(
            &format!("{}: has version 1", prefix),
            call.body["version"].as_u64() == Some(1),
        );
        r.check(
            &format!("{}: has category 'governance'", prefix),
            call.body["category"].as_str() == Some("governance"),
        );
        r.check(
            &format!("{}: correct space_id", prefix),
            call.body["space_id"].as_str() == Some(expected_space),
        );
        r.check(
            &format!("{}: has user_space_id", prefix),
            call.body["user_space_id"].is_string(),
        );
        r.check(
            &format!("{}: has idempotency_key", prefix),
            call.body["idempotency_key"].is_string(),
        );
        if let Some(block) = expected_block {
            r.check(
                &format!("{}: correct block_number {}", prefix, block),
                call.body["block_number"].as_u64() == Some(block),
            );
        } else {
            r.check(
                &format!("{}: no block_number (off-chain)", prefix),
                call.body.get("block_number").is_none(),
            );
        }
        if let Some(ts) = expected_ts {
            r.check(
                &format!("{}: correct timestamp {}", prefix, ts),
                call.body["timestamp"].as_u64() == Some(ts),
            );
        }
    };

    // --- proposal_created ---
    if let Some(call) = calls_3e
        .iter()
        .find(|c| c.body["event_type"].as_str() == Some("proposal_created"))
    {
        let prefix = "3e proposal_created";
        let pid = Uuid::from_bytes(PROP_3E_CREATED_BYTES).to_string();
        check_common(
            &mut r,
            prefix,
            call,
            &space_3e,
            Some(12345),
            Some(1700000000),
        );
        r.check(
            &format!("{}: correct proposal_id", prefix),
            call.body["proposal_id"].as_str() == Some(pid.as_str()),
        );
        r.check(
            &format!("{}: has proposer_id", prefix),
            call.body["proposer_id"].as_str() == Some(proposer.as_str()),
        );
        r.check(
            &format!("{}: voting_mode is 'fast'", prefix),
            call.body["voting_mode"].as_str() == Some("fast"),
        );
        r.check(
            &format!("{}: has actions array", prefix),
            call.body["actions"].is_array(),
        );
        r.check(
            &format!("{}: has settings object", prefix),
            call.body["settings"].is_object(),
        );
        if call.body["settings"].is_object() {
            r.check(
                &format!("{}: settings.start_date", prefix),
                call.body["settings"]["start_date"].as_u64() == Some(1700000000),
            );
            r.check(
                &format!("{}: settings.end_date", prefix),
                call.body["settings"]["end_date"].as_u64() == Some(1700086400),
            );
            r.check(
                &format!("{}: settings.voting_mode", prefix),
                call.body["settings"]["voting_mode"].as_str() == Some("fast"),
            );
            r.check(
                &format!("{}: settings.quorum", prefix),
                call.body["settings"]["quorum"].as_u64() == Some(1),
            );
        }
        r.check(
            &format!("{}: no voter_id", prefix),
            call.body.get("voter_id").is_none(),
        );
        r.check(
            &format!("{}: no vote", prefix),
            call.body.get("vote").is_none(),
        );
    }

    // --- proposal_updated ---
    if let Some(call) = calls_3e
        .iter()
        .find(|c| c.body["event_type"].as_str() == Some("proposal_updated"))
    {
        let prefix = "3e proposal_updated";
        let pid = Uuid::from_bytes(PROP_3E_UPDATED_BYTES).to_string();
        check_common(
            &mut r,
            prefix,
            call,
            &space_3e,
            Some(12347),
            Some(1700002000),
        );
        r.check(
            &format!("{}: correct proposal_id", prefix),
            call.body["proposal_id"].as_str() == Some(pid.as_str()),
        );
        r.check(
            &format!("{}: has proposer_id", prefix),
            call.body["proposer_id"].as_str() == Some(proposer.as_str()),
        );
        r.check(
            &format!("{}: voting_mode is 'fast'", prefix),
            call.body["voting_mode"].as_str() == Some("fast"),
        );
        r.check(
            &format!("{}: has actions array", prefix),
            call.body["actions"].is_array(),
        );
        r.check(
            &format!("{}: has settings object", prefix),
            call.body["settings"].is_object(),
        );
        r.check(
            &format!("{}: no voter_id", prefix),
            call.body.get("voter_id").is_none(),
        );
        r.check(
            &format!("{}: no vote", prefix),
            call.body.get("vote").is_none(),
        );
    }

    // --- proposal_voted ---
    if let Some(call) = calls_3e
        .iter()
        .find(|c| c.body["event_type"].as_str() == Some("proposal_voted"))
    {
        let prefix = "3e proposal_voted";
        let pid = Uuid::from_bytes(PROP_3E_VOTED_BYTES).to_string();
        check_common(
            &mut r,
            prefix,
            call,
            &space_3e,
            Some(12348),
            Some(1700003000),
        );
        r.check(
            &format!("{}: correct proposal_id", prefix),
            call.body["proposal_id"].as_str() == Some(pid.as_str()),
        );
        r.check(
            &format!("{}: correct voter_id", prefix),
            call.body["voter_id"].as_str() == Some(voter.as_str()),
        );
        r.check(
            &format!("{}: vote is 'yes'", prefix),
            call.body["vote"].as_str() == Some("yes"),
        );
        r.check(
            &format!("{}: no proposer_id", prefix),
            call.body.get("proposer_id").is_none(),
        );
        r.check(
            &format!("{}: no voting_mode", prefix),
            call.body.get("voting_mode").is_none(),
        );
        r.check(
            &format!("{}: no actions", prefix),
            call.body.get("actions").is_none(),
        );
        r.check(
            &format!("{}: no settings", prefix),
            call.body.get("settings").is_none(),
        );
    }

    // --- proposal_executed ---
    if let Some(call) = calls_3e
        .iter()
        .find(|c| c.body["event_type"].as_str() == Some("proposal_executed"))
    {
        let prefix = "3e proposal_executed";
        let pid = Uuid::from_bytes(PROP_3E_EXECUTED_BYTES).to_string();
        check_common(
            &mut r,
            prefix,
            call,
            &space_3e,
            Some(12346),
            Some(1700001000),
        );
        r.check(
            &format!("{}: correct proposal_id", prefix),
            call.body["proposal_id"].as_str() == Some(pid.as_str()),
        );
        r.check(
            &format!("{}: no proposer_id", prefix),
            call.body.get("proposer_id").is_none(),
        );
        r.check(
            &format!("{}: no voter_id", prefix),
            call.body.get("voter_id").is_none(),
        );
        r.check(
            &format!("{}: no vote", prefix),
            call.body.get("vote").is_none(),
        );
        r.check(
            &format!("{}: no voting_mode", prefix),
            call.body.get("voting_mode").is_none(),
        );
        r.check(
            &format!("{}: no actions", prefix),
            call.body.get("actions").is_none(),
        );
        r.check(
            &format!("{}: no settings", prefix),
            call.body.get("settings").is_none(),
        );
    }

    // --- proposal_settings_updated ---
    if let Some(call) = calls_3e
        .iter()
        .find(|c| c.body["event_type"].as_str() == Some("proposal_settings_updated"))
    {
        let prefix = "3e proposal_settings_updated";
        let pid = Uuid::from_bytes(PROP_3E_SETTINGS_BYTES).to_string();
        check_common(
            &mut r,
            prefix,
            call,
            &space_3e,
            Some(12349),
            Some(1700004000),
        );
        r.check(
            &format!("{}: correct proposal_id", prefix),
            call.body["proposal_id"].as_str() == Some(pid.as_str()),
        );
        r.check(
            &format!("{}: voting_mode is 'slow'", prefix),
            call.body["voting_mode"].as_str() == Some("slow"),
        );
        r.check(
            &format!("{}: has settings object", prefix),
            call.body["settings"].is_object(),
        );
        if call.body["settings"].is_object() {
            r.check(
                &format!("{}: settings.end_date", prefix),
                call.body["settings"]["end_date"].as_u64() == Some(1700172800),
            );
            r.check(
                &format!("{}: settings.voting_mode 'slow'", prefix),
                call.body["settings"]["voting_mode"].as_str() == Some("slow"),
            );
            r.check(
                &format!("{}: settings.quorum 5", prefix),
                call.body["settings"]["quorum"].as_u64() == Some(5),
            );
            r.check(
                &format!("{}: settings.percentage_threshold 5000000", prefix),
                call.body["settings"]["percentage_threshold"].as_u64() == Some(5000000),
            );
        }
        r.check(
            &format!("{}: no proposer_id", prefix),
            call.body.get("proposer_id").is_none(),
        );
        r.check(
            &format!("{}: no voter_id", prefix),
            call.body.get("voter_id").is_none(),
        );
        r.check(
            &format!("{}: no actions", prefix),
            call.body.get("actions").is_none(),
        );
    }

    // --- proposal_rejected ---
    if let Some(call) = calls_3e
        .iter()
        .find(|c| c.body["event_type"].as_str() == Some("proposal_rejected"))
    {
        let prefix = "3e proposal_rejected";
        let pid = Uuid::from_bytes(PROP_3E_REJECTED_BYTES).to_string();
        let proposed_by = Uuid::from_bytes(PROPOSER_ID_BYTES).to_string();
        check_common(&mut r, prefix, call, &space_3e, None, None);
        r.check(
            &format!("{}: correct proposal_id", prefix),
            call.body["proposal_id"].as_str() == Some(pid.as_str()),
        );
        r.check(
            &format!("{}: has proposer_id", prefix),
            call.body["proposer_id"].as_str() == Some(proposed_by.as_str()),
        );
        r.check(
            &format!("{}: has timestamp", prefix),
            call.body["timestamp"].is_number(),
        );
        r.check(
            &format!("{}: no voter_id", prefix),
            call.body.get("voter_id").is_none(),
        );
        r.check(
            &format!("{}: no vote", prefix),
            call.body.get("vote").is_none(),
        );
        r.check(
            &format!("{}: no voting_mode", prefix),
            call.body.get("voting_mode").is_none(),
        );
        r.check(
            &format!("{}: no actions", prefix),
            call.body.get("actions").is_none(),
        );
        r.check(
            &format!("{}: no settings", prefix),
            call.body.get("settings").is_none(),
        );
    }

    // --- 1-editor space: proposal_created ---
    if let Some(call) = calls.iter().find(|c| {
        c.body["space_id"].as_str() == Some(space_1e.as_str())
            && c.body["event_type"].as_str() == Some("proposal_created")
    }) {
        let prefix = "1e proposal_created";
        let pid = Uuid::from_bytes(PROP_1E_CREATED_BYTES).to_string();
        check_common(
            &mut r,
            prefix,
            call,
            &space_1e,
            Some(20001),
            Some(1700010000),
        );
        r.check(
            &format!("{}: correct proposal_id", prefix),
            call.body["proposal_id"].as_str() == Some(pid.as_str()),
        );
        r.check(
            &format!("{}: correct user_space_id", prefix),
            call.body["user_space_id"].as_str() == Some(editor_solo.as_str()),
        );
    }

    // ===================================================================
    // Cross-event isolation
    // ===================================================================
    let created_id = Uuid::from_bytes(PROP_3E_CREATED_BYTES).to_string();
    r.check(
        "3e created proposal_id only in proposal_created",
        !calls.iter().any(|c| {
            c.body["proposal_id"].as_str() == Some(created_id.as_str())
                && c.body["event_type"].as_str() != Some("proposal_created")
        }),
    );
    let executed_id = Uuid::from_bytes(PROP_3E_EXECUTED_BYTES).to_string();
    r.check(
        "3e executed proposal_id only in proposal_executed",
        !calls.iter().any(|c| {
            c.body["proposal_id"].as_str() == Some(executed_id.as_str())
                && c.body["event_type"].as_str() != Some("proposal_executed")
        }),
    );

    // ===================================================================
    // Bounty event verification
    // ===================================================================

    let bounty_space = Uuid::from_bytes(BOUNTY_SPACE_BYTES).to_string();
    let bounty_entity = Uuid::from_bytes(BOUNTY_ENTITY_BYTES).to_string();
    let interest_rel = Uuid::from_bytes(INTEREST_RELATION_BYTES).to_string();
    let allocated_rel = Uuid::from_bytes(ALLOCATED_RELATION_BYTES).to_string();
    let payout_rel = Uuid::from_bytes(PAYOUT_RELATION_BYTES).to_string();

    // --- bounty_interest: 2 editors × 3 webhooks = 6 calls ---
    let interest_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.body["event_type"].as_str() == Some("bounty_interest"))
        .collect();
    r.check(
        "bounty_interest: 6 calls (2 editors x 3 webhooks)",
        interest_calls.len() == 6,
    );
    if let Some(call) = interest_calls.first() {
        r.check(
            "bounty_interest: category is 'bounty'",
            call.body["category"].as_str() == Some("bounty"),
        );
        r.check(
            "bounty_interest: correct bounty_entity_id",
            call.body["bounty_entity_id"].as_str() == Some(bounty_entity.as_str()),
        );
        r.check(
            "bounty_interest: correct relation_id",
            call.body["relation_id"].as_str() == Some(interest_rel.as_str()),
        );
        r.check(
            "bounty_interest: has bounty_space_id",
            call.body["bounty_space_id"].as_str() == Some(bounty_space.as_str()),
        );
        r.check(
            "bounty_interest: no governance fields",
            call.body.get("proposal_id").is_none()
                && call.body.get("voter_id").is_none()
                && call.body.get("proposer_id").is_none(),
        );
    }
    // Each bounty editor gets exactly 3 deliveries
    for editor_str in [&bounty_editor_1, &bounty_editor_2] {
        let n = interest_calls
            .iter()
            .filter(|c| c.body["user_space_id"].as_str() == Some(editor_str.as_str()))
            .count();
        r.check(
            &format!(
                "bounty_interest: editor {}.. got 3 deliveries",
                &editor_str[..8]
            ),
            n == 3,
        );
    }

    // --- bounty_allocated: 1 curator × 3 webhooks = 3 calls ---
    let allocated_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.body["event_type"].as_str() == Some("bounty_allocated"))
        .collect();
    r.check(
        "bounty_allocated: 3 calls (1 curator x 3 webhooks)",
        allocated_calls.len() == 3,
    );
    if let Some(call) = allocated_calls.first() {
        r.check(
            "bounty_allocated: category is 'bounty'",
            call.body["category"].as_str() == Some("bounty"),
        );
        r.check(
            "bounty_allocated: correct relation_id",
            call.body["relation_id"].as_str() == Some(allocated_rel.as_str()),
        );
        r.check(
            "bounty_allocated: curator is recipient",
            call.body["user_space_id"].as_str() == Some(curator_space.as_str()),
        );
    }

    // --- bounty_payout: 1 curator × 3 webhooks = 3 calls ---
    let payout_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.body["event_type"].as_str() == Some("bounty_payout"))
        .collect();
    r.check(
        "bounty_payout: 3 calls (1 curator x 3 webhooks)",
        payout_calls.len() == 3,
    );
    if let Some(call) = payout_calls.first() {
        r.check(
            "bounty_payout: category is 'bounty'",
            call.body["category"].as_str() == Some("bounty"),
        );
        r.check(
            "bounty_payout: correct relation_id",
            call.body["relation_id"].as_str() == Some(payout_rel.as_str()),
        );
        r.check(
            "bounty_payout: curator is recipient",
            call.body["user_space_id"].as_str() == Some(curator_space.as_str()),
        );
    }

    // --- Phase 3a: bounty_created — 2 bounty-space editors × 3 webhooks = 6 ---
    let new_bounty_entity = Uuid::from_bytes(NEW_BOUNTY_ENTITY_BYTES).to_string();
    let bounty_space = Uuid::from_bytes(BOUNTY_SPACE_BYTES).to_string();
    let created_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.body["event_type"].as_str() == Some("bounty_created"))
        .collect();
    r.check(
        "bounty_created: 6 calls (2 bounty editors x 3 webhooks)",
        created_calls.len() == 6,
    );
    if let Some(call) = created_calls.first() {
        r.check(
            "bounty_created: category is 'bounty'",
            call.body["category"].as_str() == Some("bounty"),
        );
        r.check(
            "bounty_created: correct bounty_entity_id",
            call.body["bounty_entity_id"].as_str() == Some(new_bounty_entity.as_str()),
        );
        r.check(
            "bounty_created: correct bounty_space_id",
            call.body["bounty_space_id"].as_str() == Some(bounty_space.as_str()),
        );
    }
    for editor in [&bounty_editor_1, &bounty_editor_2] {
        let n = created_calls
            .iter()
            .filter(|c| c.body["user_space_id"].as_str() == Some(editor.as_str()))
            .count();
        r.check(
            &format!("bounty_created: editor {}.. got 3 deliveries", &editor[..8]),
            n == 3,
        );
    }

    // --- Phase 2a: proposal_comment — proposer × 3 webhooks = 3, and the
    //     non-member comment is filtered out (exactly 3 total proves it). ---
    let comment_proposal = Uuid::from_bytes(COMMENT_PROPOSAL_BYTES).to_string();
    let comment_member = Uuid::from_bytes(COMMENT_MEMBER_BYTES).to_string();
    let comment_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.body["event_type"].as_str() == Some("proposal_comment"))
        .collect();
    r.check(
        "proposal_comment: exactly 3 calls (proposer x 3; non-member comment filtered out)",
        comment_calls.len() == 3,
    );
    r.check(
        "proposal_comment: all delivered to the proposer",
        comment_calls
            .iter()
            .all(|c| c.body["user_space_id"].as_str() == Some(comment_proposer.as_str())),
    );
    if let Some(call) = comment_calls.first() {
        r.check(
            "proposal_comment: category is 'comment'",
            call.body["category"].as_str() == Some("comment"),
        );
        r.check(
            "proposal_comment: correct proposal_id",
            call.body["proposal_id"].as_str() == Some(comment_proposal.as_str()),
        );
        r.check(
            "proposal_comment: commenter_space_id is the member commenter",
            call.body["commenter_space_id"].as_str() == Some(comment_member.as_str()),
        );
    }

    // --- Phase 2b: comment thread — a reply notifies the prior participant and
    //     the root's creator (home space), but NOT the reply's author. ---
    let thread_root = Uuid::from_bytes(THREAD_ROOT_BYTES).to_string();
    let reply_author = Uuid::from_bytes(REPLY_AUTHOR_SPACE_BYTES).to_string();
    let comment_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.body["event_type"].as_str() == Some("comment"))
        .collect();
    r.check(
        "comment: 6 calls (prior participant + root creator) x 3 webhooks",
        comment_calls.len() == 6,
    );
    if let Some(call) = comment_calls.first() {
        r.check(
            "comment: category is 'comment'",
            call.body["category"].as_str() == Some("comment"),
        );
        r.check(
            "comment: correct thread root_id",
            call.body["root_id"].as_str() == Some(thread_root.as_str()),
        );
    }
    // The prior participant and the root's creator each get 3 (one per webhook).
    for (label, who) in [
        ("prior participant", &thread_participant),
        ("root creator", &thread_home),
    ] {
        let n = comment_calls
            .iter()
            .filter(|c| c.body["user_space_id"].as_str() == Some(who.as_str()))
            .count();
        r.check(
            &format!("comment: {} notified on 3 webhooks", label),
            n == 3,
        );
    }
    // The reply's own author must NOT be notified.
    r.check(
        "comment: reply author is not notified (self-exclusion)",
        !comment_calls
            .iter()
            .any(|c| c.body["user_space_id"].as_str() == Some(reply_author.as_str())),
    );

    r
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let webhook_port: u16 = env::var("WEBHOOK_PORT")
        .unwrap_or_else(|_| "8765".to_string())
        .parse()
        .expect("WEBHOOK_PORT must be a valid port number");
    let webhook_secret =
        env::var("WEBHOOK_SECRET").unwrap_or_else(|_| "test-e2e-secret".to_string());
    let timeout_secs: u64 = env::var("E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);

    println!("=== Notification Service E2E Tests ===");
    println!();
    println!("  database:       {}", database_url);
    println!("  kafka:          {}", kafka_broker);
    println!("  webhook port:   {}", webhook_port);
    println!("  timeout:        {}s", timeout_secs);
    println!("  spaces:         3 (0 editors, 1 editor, 3 editors)");
    println!("  webhooks:       {}", NUM_WEBHOOKS);
    println!("  expected calls: {}", EXPECTED_CALLS);
    println!();

    // 1. Start webhook server
    let (tx, mut rx) = mpsc::unbounded_channel::<WebhookCall>();
    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(tx);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", webhook_port))
        .await
        .unwrap_or_else(|e| panic!("Failed to bind on port {}: {}", webhook_port, e));
    println!("[1/5] Webhook server listening on port {}", webhook_port);
    let _webhook_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("\nERROR: Webhook server failed: {}", e);
            std::process::exit(1);
        }
    });

    // 2. Seed database
    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to database");
    seed_database(&pool, webhook_port, &webhook_secret)
        .await
        .expect("Failed to seed database");
    println!("[2/5] Database seeded");

    // 3. Produce Kafka events
    produce_test_events(&kafka_broker)
        .await
        .expect("Failed to produce Kafka events");
    println!("[3/5] Kafka events produced (7 governance + 3 bounty events)");

    // 4. Wait for webhook calls
    // We wait for EXPECTED_CALLS, plus an extra grace period to catch false positives.
    println!(
        "[4/5] Waiting for {} webhook calls (timeout: {}s)...",
        EXPECTED_CALLS, timeout_secs
    );

    let mut calls: Vec<WebhookCall> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    // First, collect the expected calls
    while calls.len() < EXPECTED_CALLS {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            eprintln!(
                "\nERROR: Timeout. Received {}/{} calls.",
                calls.len(),
                EXPECTED_CALLS
            );
            let mut counts: HashMap<String, usize> = HashMap::new();
            for c in &calls {
                *counts
                    .entry(c.body["event_type"].as_str().unwrap_or("?").to_string())
                    .or_default() += 1;
            }
            for (et, n) in &counts {
                eprintln!("  {}: {}", et, n);
            }
            std::process::exit(1);
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(call)) => {
                let n = calls.len() + 1;
                if n <= 6 || n.is_multiple_of(10) || n == EXPECTED_CALLS {
                    let et = call.body["event_type"].as_str().unwrap_or("?");
                    let uid = call.body["user_space_id"]
                        .as_str()
                        .map(|s| &s[..8])
                        .unwrap_or("?");
                    println!("  -> {}/{}: {} (editor: {}...)", n, EXPECTED_CALLS, et, uid);
                }
                calls.push(call);
            }
            Ok(None) => {
                eprintln!("\nERROR: Channel closed");
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!(
                    "\nERROR: Timeout. Received {}/{}.",
                    calls.len(),
                    EXPECTED_CALLS
                );
                std::process::exit(1);
            }
        }
    }

    // Brief grace period: drain any unexpected extra calls (false positives)
    println!("  Draining extras for 3s...");
    let grace_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = grace_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(call)) => {
                eprintln!(
                    "  !! extra call: {} (space: {}, editor: {})",
                    call.body["event_type"].as_str().unwrap_or("?"),
                    call.body["space_id"].as_str().unwrap_or("?"),
                    call.body["user_space_id"].as_str().unwrap_or("?"),
                );
                calls.push(call);
            }
            _ => break,
        }
    }

    // 5. Verify
    println!();
    println!("[5/5] Verifying {} webhook calls...", calls.len());
    println!();

    let results = verify_calls(&calls, &webhook_secret);

    println!();
    println!(
        "=== Results: {} passed, {} failed ===",
        results.passed, results.failed
    );
    if results.failed > 0 {
        std::process::exit(1);
    }
    println!();
    println!("All e2e tests passed!");
}
