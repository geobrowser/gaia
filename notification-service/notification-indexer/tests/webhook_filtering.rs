//! Integration test for per-webhook notification filtering at fan-out.
//!
//! Runs only when `NOTIF_TEST_DATABASE_URL` points at a throwaway Postgres
//! (it DROPs/CREATEs the three tables it needs, so don't aim it at a real DB).
//! Verifies that `insert_notifications_for_users` creates `notification_deliveries`
//! rows only for webhooks whose `notification_types` / `space_ids` filters match.

use notification_indexer::models::{
    ActionSummary, GovernanceData, NotificationData, NotificationEvent, NotificationEventType,
    NotificationPayload, PAYLOAD_VERSION,
};
use notification_indexer::storage::Storage;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const SCHEMA: &str = r#"
DROP TABLE IF EXISTS notification_deliveries;
DROP TABLE IF EXISTS notification_outbox;
DROP TABLE IF EXISTS app_webhooks;
CREATE TABLE app_webhooks (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    app_name text NOT NULL UNIQUE,
    url text NOT NULL,
    secret text NOT NULL,
    notification_types text[],
    space_ids uuid[],
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE notification_outbox (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    idempotency_key text NOT NULL UNIQUE,
    event_type text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE notification_deliveries (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    outbox_id uuid NOT NULL,
    webhook_id uuid NOT NULL,
    status text NOT NULL DEFAULT 'pending',
    attempts smallint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(outbox_id, webhook_id)
);
"#;

/// Build a `proposal_created` event in `space_id` with the given action types.
fn proposal_event(idem: &str, space_id: Uuid, actions: &[&str]) -> NotificationEvent {
    NotificationEvent {
        event_type: NotificationEventType::ProposalCreated,
        idempotency_key: idem.to_string(),
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: "proposal_created".to_string(),
            category: "governance".to_string(),
            space_id: space_id.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: None,
            timestamp: None,
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: Uuid::nil().to_string(),
                proposer_id: None,
                voter_id: None,
                vote: None,
                voting_mode: None,
                actions: Some(
                    actions
                        .iter()
                        .map(|t| ActionSummary {
                            action_type: t.to_string(),
                            ..Default::default()
                        })
                        .collect(),
                ),
                settings: None,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    }
}

/// App names that received a delivery, sorted. (Truncate between events to reset.)
async fn delivered_apps(pool: &PgPool) -> Vec<String> {
    let rows = sqlx::query(
        "SELECT w.app_name FROM notification_deliveries d
         JOIN app_webhooks w ON w.id = d.webhook_id
         ORDER BY w.app_name",
    )
    .fetch_all(pool)
    .await
    .expect("query deliveries");
    rows.iter()
        .map(|r| r.get::<String, _>("app_name"))
        .collect()
}

#[tokio::test]
async fn fan_out_respects_webhook_filters() {
    let Ok(url) = std::env::var("NOTIF_TEST_DATABASE_URL") else {
        eprintln!("skipping fan_out_respects_webhook_filters: set NOTIF_TEST_DATABASE_URL to run");
        return;
    };

    let storage = Storage::connect(&url).await.expect("connect");
    let pool = storage.pool().clone();

    for stmt in SCHEMA.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(stmt)
            .execute(&pool)
            .await
            .expect("apply schema");
    }

    let space_x = Uuid::from_u128(0xA);
    let space_y = Uuid::from_u128(0xB);

    // all → no filters; member_only → add_member; space_x_only → space X
    sqlx::query(
        "INSERT INTO app_webhooks (app_name, url, secret, notification_types, space_ids) VALUES
            ('all',          'http://a', 's', NULL,                NULL),
            ('member_only',  'http://b', 's', ARRAY['add_member'], NULL),
            ('space_x_only', 'http://c', 's', NULL,                ARRAY[$1]::uuid[])",
    )
    .bind(space_x)
    .execute(&pool)
    .await
    .expect("seed webhooks");

    let recipient = Uuid::from_u128(0xE);

    // Event 1: add_member proposal in space X → all (no filter) + member_only (type) + space_x_only (space).
    let e1 = proposal_event("e1", space_x, &["add_member"]);
    storage
        .insert_notifications_for_users(&e1, &[recipient])
        .await
        .expect("insert e1");
    assert_eq!(
        delivered_apps(&pool).await,
        vec!["all", "member_only", "space_x_only"],
        "add_member proposal in space X reaches all three"
    );

    sqlx::query("TRUNCATE notification_deliveries, notification_outbox")
        .execute(&pool)
        .await
        .expect("truncate after e1");

    // Event 2: plain (publish) proposal in space Y → only `all`
    //   member_only: no add_member token; space_x_only: wrong space.
    let e2 = proposal_event("e2", space_y, &["publish"]);
    storage
        .insert_notifications_for_users(&e2, &[recipient])
        .await
        .expect("insert e2");
    assert_eq!(
        delivered_apps(&pool).await,
        vec!["all"],
        "plain proposal in space Y reaches only the unfiltered webhook"
    );

    sqlx::query("TRUNCATE notification_deliveries, notification_outbox")
        .execute(&pool)
        .await
        .expect("truncate after e2");

    // Event 3: a vote in space X → only `all` (member_only filters type; space_x_only would
    // match the space but a vote isn't add_member... it has no type filter, so space matches).
    // Vote token = {proposal_voted}: all (no filter) + space_x_only (space X, no type filter).
    let e3 = NotificationEvent {
        event_type: NotificationEventType::ProposalVoted,
        idempotency_key: "e3".to_string(),
        payload: NotificationPayload {
            version: PAYLOAD_VERSION,
            event_type: "proposal_voted".to_string(),
            category: "governance".to_string(),
            space_id: space_x.to_string(),
            user_space_id: None,
            idempotency_key: None,
            block_number: None,
            timestamp: None,
            space_name: None,
            data: NotificationData::Governance(GovernanceData {
                proposal_id: Uuid::nil().to_string(),
                proposer_id: None,
                voter_id: None,
                vote: None,
                voting_mode: None,
                actions: None,
                settings: None,
                proposal_name: None,
                proposer_name: None,
                voter_name: None,
                yes_count: None,
                no_count: None,
                abstain_count: None,
            }),
        },
    };
    storage
        .insert_notifications_for_users(&e3, &[recipient])
        .await
        .expect("insert e3");
    assert_eq!(
        delivered_apps(&pool).await,
        vec!["all", "space_x_only"],
        "a vote in space X reaches the unfiltered + space-X webhooks, not the add_member one"
    );

    sqlx::query("TRUNCATE notification_deliveries, notification_outbox")
        .execute(&pool)
        .await
        .expect("truncate after e3");

    // Event 4: webhook deleted while the cache is still warm. The cache (warmed by
    // e1-e3) still holds space_x_only's id, but the row is gone — the fan-out insert
    // must skip it (join against app_webhooks) rather than hit the FK and abort.
    sqlx::query("DELETE FROM app_webhooks WHERE app_name = 'space_x_only'")
        .execute(&pool)
        .await
        .expect("delete space_x_only");

    let e4 = proposal_event("e4", space_x, &["add_member"]);
    storage
        .insert_notifications_for_users(&e4, &[recipient])
        .await
        .expect("insert e4 must not FK-error on the stale cached webhook id");
    assert_eq!(
        delivered_apps(&pool).await,
        vec!["all", "member_only"],
        "a deleted webhook still in the cache is skipped, not delivered or errored"
    );
}
