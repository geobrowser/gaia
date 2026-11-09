//! With `RANKING_INDEXER_PG_WRITES` off, every `Storage` write issues zero Postgres statements.
//!
//! No database is needed: the pool is lazy and points at a local listener that counts TCP
//! connections. A statement needs a connection, so zero accepts means zero statements. A control
//! run with the flag on proves the listener would have seen one.

use crate::models::{BlockMeta, Ranking, RankingBlock, RankingItem};
use crate::publish::RankPositionRow;
use crate::scoring::ScoreRow;
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
    Storage::from_pool(pool).with_pg_writes(pg_writes)
}

fn id() -> Uuid {
    Uuid::new_v4()
}

/// Calls every `Storage` write once.
async fn call_every_write(s: &Storage) -> Vec<Result<(), crate::error::IndexerError>> {
    let (a, b) = (id(), id());
    let block = RankingBlock {
        id: a,
        space_id: b,
        name: None,
        filter: None,
        start_date: None,
        end_date: None,
        restriction_id: None,
        ranking_type: None,
        submission_frequency: None,
    };
    let ranking = Ranking {
        id: a,
        block_id: Some(b),
        space_id: b,
        author_address: None,
        rank_type: None,
        submitted_at: None,
        updated_at_block: 1,
        update_index: 0,
    };
    let item = RankingItem {
        ranking_id: a,
        entity_id: b,
        space_id: b,
        position: None,
        weight: None,
    };
    let score = ScoreRow {
        entity_id: a,
        space_id: b,
        score: 1.0,
        position: 1,
    };
    let proj = RankPositionRow {
        entity_id: a,
        space_id: b,
        value: 1,
        position: "a".into(),
        relation_id: id(),
        reified_entity_id: id(),
        value_row_id: id(),
    };
    vec![
        s.add_member(a, b).await,
        s.remove_member(a, b).await,
        s.add_editor(a, b).await,
        s.remove_editor(a, b).await,
        s.upsert_ranking_block(&block).await,
        s.upsert_ranking(&ranking).await,
        s.set_ranking_block(a, b, b).await,
        s.replace_ranking_items(a, &[item]).await,
        s.replace_ranking_scores(a, &[score]).await,
        s.replace_rank_position_projection(
            a,
            b,
            BlockMeta {
                number: 1,
                timestamp: 1,
            },
            &[proj],
            &[id()],
        )
        .await,
    ]
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
                for r in call_every_write(&off).await {
                    r.expect("a skipped write returns Ok");
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert_eq!(
                    accepted.load(Ordering::SeqCst),
                    0,
                    "a statement reached Postgres"
                );

                // Control: with the flag on a write does try to connect.
                let on = storage_on(port, true);
                assert!(on.add_member(id(), id()).await.is_err());
                assert!(
                    accepted.load(Ordering::SeqCst) > 0,
                    "control saw no connection"
                );
            });
    });
    let rendered = handle.render();
    for site in [
        "add_member",
        "remove_member",
        "add_editor",
        "remove_editor",
        "upsert_ranking_block",
        "upsert_ranking",
        "set_ranking_block",
        "replace_ranking_items",
        "replace_ranking_scores",
        "replace_rank_position_projection",
    ] {
        let line = format!("ranking_indexer_pg_writes_skipped_total{{site=\"{site}\"}} 1");
        assert!(rendered.contains(&line), "missing `{line}` in:\n{rendered}");
    }
}
