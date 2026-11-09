//! With `DELIVERY_WORKER_PG_WRITES` off, every Postgres write site issues zero statements.
//!
//! No database is needed: the pool is lazy and points at a local listener that counts TCP
//! connections. A statement needs a connection, so zero accepts means zero statements. A control
//! run with the flag on proves the listener would have seen one.

use delivery_worker::storage::{pg_writes_enabled_from, Storage};
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

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
    Storage::new(pool)
        .with_webhooks_file(None)
        .with_pg_writes(pg_writes)
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
                let id = Uuid::new_v4();

                assert!(off.claim_pending(10).await.unwrap().is_empty());
                off.mark_delivered(id).await.unwrap();
                off.mark_retry(id, 30, "e").await.unwrap();
                off.mark_failed(id, "e").await.unwrap();
                assert_eq!(off.reset_stale_claims(300).await.unwrap(), 0);
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert_eq!(
                    accepted.load(Ordering::SeqCst),
                    0,
                    "a statement reached Postgres"
                );

                // Control: with the flag on the same call does try to connect.
                let on = storage_on(port, true);
                assert!(on.mark_delivered(id).await.is_err());
                assert!(
                    accepted.load(Ordering::SeqCst) > 0,
                    "control saw no connection"
                );
            });
    });
    let rendered = handle.render();
    for site in [
        "claim",
        "mark_delivered",
        "mark_retry",
        "mark_failed",
        "reset_stale_claims",
    ] {
        let line = format!("delivery_worker_pg_writes_skipped_total{{site=\"{site}\"}} 1");
        assert!(rendered.contains(&line), "missing `{line}` in:\n{rendered}");
    }
}
