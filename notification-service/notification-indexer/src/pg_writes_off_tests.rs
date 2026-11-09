//! With `NOTIFICATION_INDEXER_PG_WRITES` off, every Postgres write site issues zero statements.
//!
//! No database is needed: the pool is lazy and points at a local listener that counts TCP
//! connections. A statement needs a connection, so zero accepts means zero statements. A control
//! run with the flag on proves the listener would have seen one.

use crate::models::{
    ActionSummary, GovernanceData, NotificationData, NotificationEvent, NotificationEventType,
    NotificationPayload, PAYLOAD_VERSION,
};
use crate::storage::{pg_writes_enabled_from, Storage};
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

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

/// A listener that counts connection attempts and drops each one.
async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let seen = accepted.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            seen.fetch_add(1, Ordering::SeqCst);
            drop(sock);
        }
    });
    (port, accepted)
}

fn storage_on(port: u16, pg_writes: bool) -> Storage {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(2))
        .connect_lazy(&format!("postgres://u:p@127.0.0.1:{port}/db"))
        .unwrap();
    Storage::new(pool).with_pg_writes(pg_writes)
}

#[test]
fn flag_parsing_defaults_on() {
    assert!(pg_writes_enabled_from(None));
    assert!(pg_writes_enabled_from(Some("")));
    assert!(pg_writes_enabled_from(Some("1")));
    assert!(pg_writes_enabled_from(Some("garbage")));
    for off in ["0", "false", "OFF", " no "] {
        assert!(!pg_writes_enabled_from(Some(off)), "{off}");
    }
}

#[test]
fn pg_writes_off_issues_no_statements_and_counts_skips() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (port, accepted) = counting_listener().await;
                let off = storage_on(port, false);
                let event = proposal_event("k", Uuid::new_v4(), &["add_member"]);

                let n = off
                    .insert_notifications_for_users(&event, &[Uuid::new_v4(), Uuid::new_v4()])
                    .await
                    .unwrap();
                assert_eq!(n, 0);
                assert_eq!(
                    off.delete_expired_notifications(30, 100).await.unwrap(),
                    (0, 0)
                );
                off.set_poll_cursor("c", chrono::Utc::now(), 7)
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert_eq!(
                    accepted.load(Ordering::SeqCst),
                    0,
                    "a statement reached Postgres"
                );

                // Control: with the flag on the same call does try to connect.
                let on = storage_on(port, true);
                assert!(on
                    .set_poll_cursor("c", chrono::Utc::now(), 7)
                    .await
                    .is_err());
                assert!(
                    accepted.load(Ordering::SeqCst) > 0,
                    "control saw no connection"
                );
            });
    });
    let rendered = handle.render();
    for line in [
        "notification_indexer_pg_writes_skipped_total{site=\"outbox_insert\"} 1",
        "notification_indexer_pg_writes_skipped_total{site=\"retention_delete\"} 1",
        "notification_indexer_pg_writes_skipped_total{site=\"poll_cursor_upsert\"} 1",
    ] {
        assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
    }
}
