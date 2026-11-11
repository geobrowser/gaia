//! With `RANKING_INDEXER_PG_WRITES` off, the periodic binaries' Postgres writes are never started.
//!
//! Same method as `pg_writes_off_tests`: a lazy pool pointed at a local listener that counts TCP
//! accepts, so zero accepts means zero statements, with a control run (flag on) that must connect.
//! The library-level writers (`feed_signals`, `not_interested`, `topic_interest`) are called for
//! real. The plpgsql calls written inline in a binary cannot be called from here, so for those the
//! generic gate is proven on a representative statement and a source check pins that each binary
//! still routes its call through `pg_bin_writes::run` under its own site label.

use crate::feed_signals::{self, Cutoff, Window};
use crate::metrics::PgWriteSite;
use crate::pg_bin_writes::run_if;
use crate::{not_interested, topic_interest};
use chrono::Utc;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

fn lazy_pool(port: u16) -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(2))
        .connect_lazy(&format!("postgres://u:p@127.0.0.1:{port}/db"))
        .unwrap()
}

fn window() -> Window {
    let now = Utc::now();
    Window {
        window_start: now,
        window_end: now,
        last_data_through: None,
        unchanged: false,
    }
}

fn cutoff() -> Cutoff {
    Cutoff {
        generation: "g".into(),
        data_through: Utc::now().to_rfc3339(),
    }
}

/// Every library-level writer, once, through the gate. Returns how many came back skipped.
async fn run_library_writers(pool: &PgPool, on: bool) -> usize {
    let mut skipped = 0;
    let c = cutoff();
    let w = window();
    skipped += run_if(
        on,
        PgWriteSite::FeedSignalsApply,
        feed_signals::apply(pool, "g", &w, &[], &[], false, false),
    )
    .await
    .map_or(0, |r| r.is_none() as usize);
    skipped += run_if(
        on,
        PgWriteSite::FeedSignalsRecord,
        feed_signals::record(pool, "unchanged", Some(&c), None),
    )
    .await
    .map_or(0, |r| r.is_none() as usize);
    skipped += run_if(
        on,
        PgWriteSite::NotInterestedApply,
        not_interested::apply(pool, &[], false, false, None),
    )
    .await
    .map_or(0, |r| r.is_none() as usize);
    skipped += run_if(
        on,
        PgWriteSite::TopicInterestSweep,
        topic_interest::sweep(pool, None),
    )
    .await
    .map_or(0, |r| r.is_none() as usize);
    skipped += run_if(
        on,
        PgWriteSite::TopicInterestRefit,
        topic_interest::refit(pool, None),
    )
    .await
    .map_or(0, |r| r.is_none() as usize);
    skipped
}

/// A statement of the kind the binaries write inline: it needs a connection from the pool.
async fn inline_statement(pool: &PgPool) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar("SELECT public.refresh_account_weights()")
        .fetch_one(pool)
        .await
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn off_starts_no_statement_for_any_library_writer_and_counts_each_site() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        runtime().block_on(async {
            let (port, accepted) = counting_listener().await;
            let pool = lazy_pool(port);
            assert_eq!(run_library_writers(&pool, false).await, 5);
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                accepted.load(Ordering::SeqCst),
                0,
                "a statement reached Postgres"
            );

            // Control: with the flag on the same writers do try to connect.
            assert_eq!(run_library_writers(&pool, true).await, 0);
            assert!(
                accepted.load(Ordering::SeqCst) > 0,
                "control saw no connection"
            );
        });
    });
    let rendered = handle.render();
    for site in [
        "feed_signals_apply",
        "feed_signals_record",
        "not_interested_apply",
        "topic_interest_sweep",
        "topic_interest_refit",
    ] {
        let line = format!("ranking_indexer_pg_writes_skipped_total{{site=\"{site}\"}} 1");
        assert!(rendered.contains(&line), "missing `{line}` in:\n{rendered}");
    }
}

#[test]
fn off_drops_an_inline_statement_unpolled_and_on_runs_it() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        runtime().block_on(async {
            let (port, accepted) = counting_listener().await;
            let pool = lazy_pool(port);
            let off = run_if(
                false,
                PgWriteSite::AccountWeightsRefresh,
                inline_statement(&pool),
            )
            .await
            .expect("a skipped write is Ok");
            assert_eq!(off, None);
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(accepted.load(Ordering::SeqCst), 0);

            let on = run_if(
                true,
                PgWriteSite::AccountWeightsRefresh,
                inline_statement(&pool),
            )
            .await;
            assert!(on.is_err(), "the fake server is not Postgres");
            assert!(
                accepted.load(Ordering::SeqCst) > 0,
                "control saw no connection"
            );
        });
    });
    assert!(handle
        .render()
        .contains("ranking_indexer_pg_writes_skipped_total{site=\"account_weights_refresh\"} 1"));
}

#[test]
fn every_site_is_registered_at_zero() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, crate::metrics::register);
    let rendered = handle.render();
    for site in PgWriteSite::ALL {
        let line = format!(
            "ranking_indexer_pg_writes_skipped_total{{site=\"{}\"}} 0",
            site.as_str()
        );
        assert!(rendered.contains(&line), "missing `{line}`");
    }
}

/// Each binary that calls a writing plpgsql function routes it through `pg_bin_writes::run` under
/// its own site. A new unguarded call has to be added here by hand, which is the point.
#[test]
fn every_binary_routes_its_write_through_the_gate() {
    let bins: [(&str, &str, &[&str]); 12] = [
        (
            include_str!("bin/account_exclusions_sync.rs"),
            "replace_account_exclusions",
            &["AccountExclusionsReplace"],
        ),
        (
            include_str!("bin/account_weights_refresh.rs"),
            "refresh_account_weights",
            &["AccountWeightsRefresh"],
        ),
        (
            include_str!("bin/agenda_listings_refresh.rs"),
            "refresh_agenda_listings",
            &["AgendaListingsRefresh"],
        ),
        (
            include_str!("bin/feed_composition_sample.rs"),
            "sample_feed_composition",
            &[
                "FeedCompositionWindowSample",
                "FeedCompositionRenderedSample",
            ],
        ),
        (
            include_str!("bin/feed_signals_sync.rs"),
            "feed_signals::apply",
            &["FeedSignalsApply", "FeedSignalsRecord"],
        ),
        (
            include_str!("bin/not_interested_sync.rs"),
            "not_interested::apply",
            &["NotInterestedApply"],
        ),
        (
            include_str!("bin/primer_claims_refresh.rs"),
            "refresh_primer_claim_stats",
            &["PrimerClaimsRefresh", "ClaimOverlapSample"],
        ),
        (
            include_str!("bin/stance_map_shadow.rs"),
            "record_stance_map_run",
            &["StanceMapRun"],
        ),
        (
            include_str!("bin/topic_interest.rs"),
            "topic_interest::sweep",
            &["TopicInterestSweep", "TopicInterestRefit"],
        ),
        (
            include_str!("bin/topic_ranking_reconcile.rs"),
            "reconcile_entity_topic_ranking",
            &["TopicRankingReconcile"],
        ),
        (
            include_str!("bin/topic_suggestions_refresh.rs"),
            "refresh_space_topic_suggestions",
            &["TopicSuggestionsRefresh"],
        ),
        (
            include_str!("bin/topic_interest.rs"),
            "topic_interest::refit",
            &["TopicInterestRefit"],
        ),
    ];
    for (src, call, sites) in bins {
        assert!(src.contains(call), "{call} moved; update this test");
        assert!(src.contains("pg_bin_writes::run("), "{call} is not gated");
        for site in sites {
            assert!(
                src.contains(&format!("PgWriteSite::{site}")),
                "{call} lacks site {site}"
            );
        }
    }
}
