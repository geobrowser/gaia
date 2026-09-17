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
//! TWO SOURCES, AND THE GAP BETWEEN THEM IS THE POINT (GEO-2950).
//!
//!   * `window`   — the rows the ranked walk returns. Measured in SQL.
//!   * `rendered` — what the app's own feed endpoint serves, after geogenesis applies
//!     `applyTargetMix`/`applyDiversityCap` and `applyPerSpaceQuota`. Only observable over
//!     HTTP, so it is measured here.
//!
//! Sampling only the window is what made this canary useless the first time it mattered.
//! On 2026-09-17 it was recording Claim 24 / Debate 8 / News 34 while readers were served
//! 1.8 / 5.9 / 2.3 per 10 — true, and about a feed nobody sees. A change in both sources is
//! a ranking change; a change in `rendered` alone is read-time reordering.
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

/// One item as the app's feed endpoint returns it. Only the fields the histogram needs.
#[derive(serde::Deserialize)]
struct FeedItem {
    #[serde(default)]
    types: Vec<FeedItemType>,
    #[serde(rename = "createdAtSec", default)]
    created_at_sec: Option<i64>,
}

#[derive(serde::Deserialize)]
struct FeedItemType {
    id: String,
}

#[derive(serde::Deserialize)]
struct FeedResponse {
    #[serde(default)]
    items: Vec<FeedItem>,
}

fn hyphenless(id: &Uuid) -> String {
    id.simple().to_string()
}

/// Samples the RENDERED feed — what the app actually serves — and logs its drift.
///
/// Classifies each item by the first watched type it carries, in the caller's own type order.
/// That mirrors geogenesis's `exploreItemTypeKey` without reproducing it: the frontend has a
/// separate priority list so an entity that is both a Claim and something more specific reads
/// as the latter. Close enough for a composition signal, and the histogram sums to the page
/// size either way; the alternative is duplicating a list that lives in another repo and would
/// drift silently.
async fn sample_rendered(
    pool: &sqlx::PgPool,
    type_ids: &[Uuid],
    feed_url: &str,
    alert_delta: f64,
) -> Result<(), IndexerError> {
    let type_param = type_ids
        .iter()
        .map(hyphenless)
        .collect::<Vec<_>>()
        .join(",");
    let url = format!("{feed_url}?sort=best&typeIds={type_param}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| IndexerError::Config(format!("building the http client: {e}")))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| IndexerError::Config(format!("fetching the rendered feed: {e}")))?;

    if !response.status().is_success() {
        return Err(IndexerError::Config(format!(
            "rendered feed returned {} for {url}",
            response.status()
        )));
    }

    let feed: FeedResponse = response
        .json()
        .await
        .map_err(|e| IndexerError::Config(format!("parsing the rendered feed: {e}")))?;

    // An empty page is a real signal, not a reason to skip: GEO-2853 was exactly that.
    let now = chrono::Utc::now().timestamp();
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    let mut ages: Vec<i64> = Vec::new();

    for item in &feed.items {
        let present: std::collections::HashSet<&str> =
            item.types.iter().map(|t| t.id.as_str()).collect();
        if let Some(matched) = type_ids
            .iter()
            .find(|t| present.contains(hyphenless(t).as_str()))
        {
            *counts.entry(matched.to_string()).or_insert(0) += 1;
        }
        if let Some(created) = item.created_at_sec {
            if created > 0 {
                ages.push(now - created);
            }
        }
    }

    ages.sort_unstable();
    let median = (!ages.is_empty()).then(|| ages[ages.len() / 2]);
    let oldest = ages.last().copied();

    let composition = serde_json::to_value(&counts)
        .map_err(|e| IndexerError::Config(format!("serialising the composition: {e}")))?;

    let sample_id: i64 = sqlx::query_scalar(
        "SELECT public.record_feed_composition_sample('rendered', $1, $2, $3, NULL, $4, $5)",
    )
    .bind(type_ids)
    .bind(&composition)
    .bind(feed.items.len() as i32)
    .bind(median)
    .bind(oldest)
    .fetch_one(pool)
    .await
    .map_err(IndexerError::Database)?;

    info!(
        sample_id,
        items = feed.items.len(),
        classified = counts.values().sum::<i64>(),
        "rendered feed sampled"
    );

    report_drift(pool, sample_id, None, "rendered", alert_delta).await
}

/// Reads `feed_composition_drift` for one (scope, source) and logs the move since its
/// previous sample. Shared by both sources so they cannot report differently.
async fn report_drift(
    pool: &sqlx::PgPool,
    sample_id: i64,
    space: Option<Uuid>,
    source: &str,
    alert_delta: f64,
) -> Result<(), IndexerError> {
    // share_delta is NUMERIC; cast in the query rather than enabling sqlx's `bigdecimal`
    // feature for one log field. The precision lost is far below the alert threshold.
    let drift = sqlx::query(
        "SELECT type_id, previous_count, current_count, share_delta::float8 AS share_delta \
         FROM public.feed_composition_drift($1, $2)",
    )
    .bind(space)
    .bind(source)
    .fetch_all(pool)
    .await
    .map_err(IndexerError::Database)?;

    if drift.is_empty() {
        // The first sample for this scope and source, or its only one.
        info!(sample_id, space = ?space, source, "no previous sample to compare against");
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
                source,
                previous_count,
                current_count,
                share_delta = delta,
                threshold = alert_delta,
                "feed composition moved sharply — if no ranking change was deployed, something \
                 else changed what people see"
            );
        } else {
            info!(sample_id, %type_id, space = ?space, source, previous_count, current_count, share_delta = delta, "feed composition drift");
        }
    }

    Ok(())
}

/// Samples one scope of the ranked WINDOW and logs its drift. `space` is None for the global feed.
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

    report_drift(pool, sample_id, space, "window", alert_delta).await
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

    // The rendered feed, if we are told where it lives. Optional so the job still runs
    // without it — a missing URL means no rendered history, not a failed run.
    match env::var("FEED_COMPOSITION_FEED_URL") {
        Err(_) => info!("FEED_COMPOSITION_FEED_URL unset — sampling the ranked window only"),
        Ok(feed_url) => {
            if let Err(e) = sample_rendered(&pool, &type_ids, feed_url.trim(), alert_delta).await {
                failures += 1;
                error!(error = %e, "failed to sample the rendered feed");
            }
        }
    }

    if failures > 0 {
        return Err(IndexerError::Config(format!(
            "{failures} of {} sample(s) failed",
            2 + space_ids.len()
        )));
    }

    Ok(())
}
