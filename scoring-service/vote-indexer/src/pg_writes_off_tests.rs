//! With `VOTE_INDEXER_PG_WRITES` off, the vote batch and the ranking refresh issue zero statements.
//!
//! No database is needed: the pool is lazy and points at a local listener that counts TCP
//! connections. A statement needs a connection, so zero accepts means zero statements. A control
//! run with the flag on proves the listener would have seen one.

use crate::batch::process_vote_batch;
use crate::models::voting::{ResponseKind, VoteItem, VoteObjectType, VoteValue};
use crate::storage::{pg_writes_enabled_from, Storage};
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

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

fn vote() -> VoteItem {
    VoteItem {
        voter_id: Uuid::new_v4(),
        object_id: Uuid::new_v4(),
        object_type: VoteObjectType::Entity,
        space_id: Uuid::new_v4(),
        vote: VoteValue::Up,
        kind: ResponseKind::Curation,
        block_number: 1,
        block_timestamp: 1,
    }
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
                let votes = [vote(), vote()];

                assert_eq!(process_vote_batch(&votes, &off).await.unwrap(), 2);
                assert_eq!(
                    off.refresh_ranking_scores_for(&[Uuid::new_v4()])
                        .await
                        .unwrap(),
                    0
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert_eq!(
                    accepted.load(Ordering::SeqCst),
                    0,
                    "a statement reached Postgres"
                );

                // Control: with the flag on the same batch does try to connect.
                let on = storage_on(port, true);
                assert!(process_vote_batch(&votes, &on).await.is_err());
                assert!(
                    accepted.load(Ordering::SeqCst) > 0,
                    "control saw no connection"
                );
            });
    });
    let rendered = handle.render();
    for site in ["vote_batch", "ranking_refresh"] {
        let line = format!("vote_indexer_pg_writes_skipped_total{{site=\"{site}\"}} 1");
        assert!(rendered.contains(&line), "missing `{line}` in:\n{rendered}");
    }
}
