//! Feed-composition canary (GEO-2926 follow-up).
//!
//! Records what the ranked feed is actually made of, on a schedule, so a ranking
//! change cannot quietly change what people see.
//!
//! WHY THIS EXISTS. On 2026-09-16 two ranking retunes shipped in one night. The first
//! cut `participation_weight` 7 -> 2.5 with a correct measurement, a passing test suite
//! and a reviewed migration, and it handed the default Explore feed to News stories —
//! the first page went from 15 Claims to 5, and from 3 News stories to 16. Nothing
//! caught it, and nothing could have: a ranking parameter has no wrong value that
//! errors, and the scores are stored rather than computed on read, so the feed does not
//! even move until a re-score runs. It was caught by a person opening the app.
//!
//! The before/after numbers above had to be reconstructed after the fact from the
//! arithmetic relating the two parameter sets. That worked only because the change was
//! confined to a single term. This binary means the next one does not need luck.
//!
//! WHAT IT DOES. One `sample_feed_composition` call per scope, then reads
//! `feed_composition_drift` for that scope and logs the move since its previous sample.
//! Both live in migrations 0091/0092 — the logic is SQL so that it is covered by the
//! suites in `api/drizzle/tests`, which run in CI, rather than by a Rust test nobody runs.
//!
//! SCOPES. Always the global feed, plus one sample per space in
//! `FEED_COMPOSITION_SPACE_IDS`. Drift is per scope: a space that moved must not be
//! reported against the global history. The original complaint behind all of this was
//! about the top of a SINGLE space, which a global-only canary would not have caught.
//!
//! DRIFT IS REPORTED, NOT ENFORCED. A large move is logged at WARN and the job still
//! succeeds. Failing the job on drift would be an alert that cries wolf: composition
//! legitimately moves when content is published, when a space goes quiet, and every
//! time someone deliberately retunes. Wiring the WARN to a real alert channel is the
//! obvious next step and deliberately not done here — Sentry's quota is exhausted until
//! 2026-09-24, so anything routed through it would be invisible on arrival.
//!
//! Usage: `DATABASE_URL=... FEED_COMPOSITION_TYPE_IDS=<uuid,uuid> cargo run -p ranking-indexer --bin feed_composition_sample`

use std::env;

use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use tracing::{error, info, warn};
use uuid::Uuid;

use ranking_indexer::error::IndexerError;

/// Samples one scope and logs its drift. `space` is None for the global feed.
async fn sample_scope(
    pool: &sqlx::PgPool,
    type_ids: &[Uuid],
    window: i32,
    space: Option<Uuid>,
    alert_delta: f64,
) -> Result<(), IndexerError> {
    let sample_id: i64 = sqlx::query_scalar("SELECT public.sample_feed_composition($1, $2, $3)")
        .bind(type_ids)
        .bind(window)
        .bind(space)
        .fetch_one(pool)
        .await
        .map_err(IndexerError::Database)?;

    info!(
        sample_id,
        window,
        types = type_ids.len(),
        space = ?space,
        "feed composition sampled"
    );

    // share_delta is NUMERIC; cast in the query rather than enabling sqlx's `bigdecimal`
    // feature for one log field. The precision lost is far below the alert threshold.
    let drift = sqlx::query(
        "SELECT type_id, previous_count, current_count, share_delta::float8 AS share_delta \
         FROM public.feed_composition_drift($1)",
    )
    .bind(space)
    .fetch_all(pool)
    .await
    .map_err(IndexerError::Database)?;

    if drift.is_empty() {
        // The first sample for this scope, or its only one. Not a problem and not an alarm.
        info!(sample_id, space = ?space, "no previous sample to compare against");
        return Ok(());
    }

    for row in drift {
        let type_id: Uuid = row.try_get("type_id").map_err(IndexerError::Database)?;
        let previous_count: i32 = row
            .try_get("previous_count")
            .map_err(IndexerError::Database)?;
        let current_count: i32 = row
            .try_get("current_count")
            .map_err(IndexerError::Database)?;
        let delta: f64 = row.try_get("share_delta").map_err(IndexerError::Database)?;

        if delta.abs() >= alert_delta {
            warn!(
                sample_id,
                %type_id,
                space = ?space,
                previous_count,
                current_count,
                share_delta = delta,
                threshold = alert_delta,
                "feed composition moved sharply — if no ranking change was deployed, something \
                 else changed what people see"
            );
        } else {
            info!(sample_id, %type_id, space = ?space, previous_count, current_count, share_delta = delta, "feed composition drift");
        }
    }

    Ok(())
}

/// `EXPLORE_DIVERSITY_WINDOW_SIZE` in geogenesis: three pages of 22. The window Explore
/// fetches before its diversity cap and per-space quota reorder it.
const DEFAULT_WINDOW: i32 = 66;

/// Share-point move that is worth a WARN. 0.15 of a 66-row window is ten rows — well
/// above the churn a normal publishing day produces, well below the 0.68 that #948 moved
/// News stories by.
const DEFAULT_ALERT_DELTA: f64 = 0.15;

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> Result<T, IndexerError> {
    match env::var(key) {
        Err(_) => Ok(default),
        Ok(raw) => raw
            .trim()
            .parse::<T>()
            .map_err(|_| IndexerError::Config(format!("{key} is not parseable: {raw:?}"))),
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;

    // Required, with no default on purpose. The type set mirrors geogenesis's
    // DEFAULT_EXPLORE_TYPE_IDS, which this repo cannot see; a compiled-in default would
    // drift from it silently and leave the canary watching a feed nobody looks at. Better
    // to refuse to run than to watch the wrong thing.
    let raw_types = env::var("FEED_COMPOSITION_TYPE_IDS").map_err(|_| {
        IndexerError::Config(
            "FEED_COMPOSITION_TYPE_IDS not set — comma-separated type ids to watch, \
             mirroring geogenesis DEFAULT_EXPLORE_TYPE_IDS"
                .into(),
        )
    })?;

    let type_ids = raw_types
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Uuid::parse_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| IndexerError::Config(format!("FEED_COMPOSITION_TYPE_IDS: {e}")))?;

    if type_ids.is_empty() {
        return Err(IndexerError::Config(
            "FEED_COMPOSITION_TYPE_IDS held no ids".into(),
        ));
    }

    let window: i32 = env_or("FEED_COMPOSITION_WINDOW", DEFAULT_WINDOW)?;
    let alert_delta: f64 = env_or("FEED_COMPOSITION_ALERT_SHARE_DELTA", DEFAULT_ALERT_DELTA)?;

    // Optional, unlike the type list: with none set the canary watches the global feed
    // only, which is the behaviour before per-space sampling existed.
    let space_ids = match env::var("FEED_COMPOSITION_SPACE_IDS") {
        Err(_) => Vec::new(),
        Ok(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Uuid::parse_str)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| IndexerError::Config(format!("FEED_COMPOSITION_SPACE_IDS: {e}")))?,
    };

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .map_err(IndexerError::Database)?;

    // Global first, then each configured space. One failure must not cost the others:
    // a space id that no longer exists should not stop the global sample being recorded.
    let mut failures = 0usize;
    for scope in std::iter::once(None).chain(space_ids.iter().copied().map(Some)) {
        if let Err(e) = sample_scope(&pool, &type_ids, window, scope, alert_delta).await {
            failures += 1;
            error!(space = ?scope, error = %e, "failed to sample this scope");
        }
    }

    if failures > 0 {
        return Err(IndexerError::Config(format!(
            "{failures} of {} scopes failed to sample",
            1 + space_ids.len()
        )));
    }

    Ok(())
}
