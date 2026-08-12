//! Substream stall detection, measured against the chain tip.
//!
//! The substream can die without the process noticing. Observed 2026-08-10:
//! Pinax dropped the HTTP/2 body (`h2 protocol error: error reading a body from
//! connection`), the client logged `Blockstreams disconnected` then
//! `Blockstreams connected`, and no `BlockScopedData` ever arrived again. The
//! process stayed healthy — 0 restarts over 8 days — so nothing restarted it,
//! and the canonical graph sat frozen at block 27694 for two days while the
//! chain advanced to 29448. Personal spaces silently stopped becoming canonical.
//!
//! A plain restart was the entire remedy: atlas resumes from its persisted
//! cursor and recomputes canonical state from full graph state, so re-processing
//! is idempotent. Recovery took ~20 seconds. This module makes that automatic —
//! on a confirmed stall it exits non-zero and lets the orchestrator restart.
//!
//! ## Why this compares against the chain tip
//!
//! The obvious detector — "no block arrived for N minutes" — is **wrong on this
//! chain**, and I shipped it before measuring. Chain 55516 is transaction-driven,
//! not a fixed-cadence chain: ~894 blocks/day, median gap between blocks 2
//! minutes, p90 8 minutes, and the longest genuinely-idle stretch over a
//! representative week was **130 minutes** (confirmed against two independent
//! metric series, so it was real chain idleness and not a broken exporter). A
//! 10-minute silence timeout would have restarted atlas 402 times in that week.
//!
//! Lag against the tip is cadence-independent and states the actual failure
//! condition: *blocks exist that we have not processed*. An idle chain yields
//! `tip == processed`, so it is silent by construction, while the real wedge
//! shows lag growing without bound.
//!
//! ## Why these thresholds
//!
//! A healthy substream consumer on this chain sits at lag **0** — measured over
//! the same week from `hermes-pipeline`: p50 0, p90 0, p99 1, max 8, and the
//! longest continuous stretch above lag 1 was 2 minutes. Substreams delivers at
//! head here, with no finality-depth offset that would leave a healthy consumer
//! permanently behind. So `lag > 10` sustained for 20 minutes has roughly two
//! orders of magnitude of headroom over anything healthy, while the real outage
//! (lag reaching 1,754) trips it inside an hour.
//!
//! Everything here fails **open**: an unreadable tip, or no processed block yet,
//! never causes a restart. A detector that guesses wrong costs availability, so
//! it only ever acts on a lag it has positively confirmed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hermes_instrumentation::{error, info, warn};

/// How far behind the tip atlas may fall before the clock starts. Chosen from
/// measured healthy lag (max 8 over a week), not from chain cadence.
pub const DEFAULT_LAG_THRESHOLD_BLOCKS: u64 = 10;

/// How long the lag must persist before atlas is treated as wedged. Must
/// comfortably exceed a cold-start replay, which took 19s for ~1,750 blocks.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(1200);

/// How often the watchdog polls the tip and re-checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Per-request timeout for the tip poll. Short: a slow RPC should read as
/// "unknown tip" (fail open), never as a stall.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared record of the last block atlas processed.
///
/// Cheap enough to touch on every block: one relaxed atomic store, so the hot
/// path is unaffected.
#[derive(Debug, Clone, Default)]
pub struct StreamProgress {
    last_block: Arc<AtomicU64>,
}

impl StreamProgress {
    /// Progress with nothing processed yet. Reads as "unknown", which fails open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Progress seeded from a restored checkpoint.
    ///
    /// This matters: without it, a restart during an idle chain would leave
    /// `last_block` at 0 while the tip sat at ~29,536, computing a lag of 29,536
    /// out of a perfectly healthy process and restarting it every 20 minutes
    /// until the chain happened to produce a block.
    pub fn seeded(block_number: u64) -> Self {
        Self {
            last_block: Arc::new(AtomicU64::new(block_number)),
        }
    }

    /// Record that a block was processed. Called from the block handler.
    pub fn record_block(&self, block_number: u64) {
        self.last_block.store(block_number, Ordering::Relaxed);
    }

    /// Last processed block; `0` means nothing processed and no checkpoint.
    pub fn last_block(&self) -> u64 {
        self.last_block.load(Ordering::Relaxed)
    }
}

/// What the watchdog decided on one tick. Separated from the exiting so the
/// decision is unit-testable without killing the test process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallVerdict {
    /// Caught up, or within the lag threshold.
    Healthy { lag: u64 },
    /// Behind the threshold, but not yet for long enough to act on.
    Lagging { lag: u64, lagging_secs: u64 },
    /// Behind the threshold for longer than the timeout — restart.
    Stalled {
        lag: u64,
        last_block: u64,
        lagging_secs: u64,
    },
    /// Cannot compute a lag. Fails open: never restarts.
    Unknown,
}

/// Tracks how long atlas has been behind the tip.
#[derive(Debug)]
pub struct LagTracker {
    threshold_blocks: u64,
    timeout: Duration,
    /// When the lag first exceeded the threshold, in unix seconds.
    lagging_since: Option<u64>,
}

impl LagTracker {
    pub fn new(threshold_blocks: u64, timeout: Duration) -> Self {
        Self {
            threshold_blocks,
            timeout,
            lagging_since: None,
        }
    }

    /// Fold one observation into the verdict.
    ///
    /// `tip` is `None` when the tip could not be read.
    pub fn observe(&mut self, processed: u64, tip: Option<u64>, now_secs: u64) -> StallVerdict {
        // No tip to compare against: hold the clock but never act. Holding
        // rather than clearing means a flaky RPC cannot indefinitely mask a real
        // stall — a trip still requires a positively confirmed lag either side.
        let Some(tip) = tip else {
            return StallVerdict::Unknown;
        };

        // Nothing processed and no checkpoint to seed from. `tip - 0` would be a
        // meaningless lag, so refuse to judge.
        if processed == 0 {
            return StallVerdict::Unknown;
        }

        // Saturating: atlas legitimately observes a block slightly before the
        // tip poll does, and measured skew reached 12 blocks ahead. That is not
        // negative lag, it is zero lag.
        let lag = tip.saturating_sub(processed);

        if lag <= self.threshold_blocks {
            self.lagging_since = None;
            return StallVerdict::Healthy { lag };
        }

        let since = *self.lagging_since.get_or_insert(now_secs);
        let lagging_secs = now_secs.saturating_sub(since);
        if lagging_secs >= self.timeout.as_secs() {
            StallVerdict::Stalled {
                lag,
                last_block: processed,
                lagging_secs,
            }
        } else {
            StallVerdict::Lagging { lag, lagging_secs }
        }
    }
}

/// Stall-detector configuration, resolved from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StallConfig {
    pub rpc_url: String,
    pub threshold_blocks: u64,
    pub timeout: Duration,
}

/// Resolve config from the environment, or `None` if detection should be off.
///
/// - `RPC_URL` — required; without it there is no tip to compare against, so the
///   detector stays off rather than falling back to a cadence guess.
/// - `ATLAS_STALL_LAG_BLOCKS` — lag threshold in blocks.
/// - `ATLAS_STALL_TIMEOUT_SECS` — how long the lag must persist; `0` disables.
pub fn config_from_env() -> Option<StallConfig> {
    let timeout = match std::env::var("ATLAS_STALL_TIMEOUT_SECS") {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(0) => {
                warn!("ATLAS_STALL_TIMEOUT_SECS=0 — substream stall detection disabled");
                return None;
            }
            Ok(secs) => Duration::from_secs(secs),
            Err(err) => {
                warn!(
                    value = %raw,
                    error = %err,
                    default_secs = DEFAULT_STALL_TIMEOUT.as_secs(),
                    "Invalid ATLAS_STALL_TIMEOUT_SECS; using default"
                );
                DEFAULT_STALL_TIMEOUT
            }
        },
        Err(_) => DEFAULT_STALL_TIMEOUT,
    };

    let threshold_blocks = match std::env::var("ATLAS_STALL_LAG_BLOCKS") {
        Ok(raw) => raw.trim().parse::<u64>().unwrap_or_else(|err| {
            warn!(
                value = %raw,
                error = %err,
                default_blocks = DEFAULT_LAG_THRESHOLD_BLOCKS,
                "Invalid ATLAS_STALL_LAG_BLOCKS; using default"
            );
            DEFAULT_LAG_THRESHOLD_BLOCKS
        }),
        Err(_) => DEFAULT_LAG_THRESHOLD_BLOCKS,
    };

    match std::env::var("RPC_URL") {
        Ok(rpc_url) if !rpc_url.trim().is_empty() => Some(StallConfig {
            rpc_url,
            threshold_blocks,
            timeout,
        }),
        _ => {
            warn!(
                "RPC_URL is not set — substream stall detection disabled. Atlas cannot tell a \
                 wedged stream from an idle chain without a chain tip to compare against."
            );
            None
        }
    }
}

/// Fetch the chain tip. `None` on any failure, so callers fail open.
async fn fetch_tip(client: &reqwest::Client, url: &str) -> Option<u64> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1,
    });

    let resp = match client.post(url).json(&body).send().await {
        Ok(resp) => resp,
        Err(err) => {
            warn!(error = %err, "Stall detector could not reach the chain RPC");
            return None;
        }
    };

    let payload: serde_json::Value = match resp.json().await {
        Ok(payload) => payload,
        Err(err) => {
            warn!(error = %err, "Chain RPC returned an unreadable tip response");
            return None;
        }
    };

    let raw = payload.get("result").and_then(|r| r.as_str());
    match raw.and_then(|r| u64::from_str_radix(r.trim_start_matches("0x"), 16).ok()) {
        Some(tip) => Some(tip),
        None => {
            warn!(response = %payload, "Chain RPC response had no usable block number");
            None
        }
    }
}

/// Spawn the watchdog. Exits the process on a confirmed stall so the
/// orchestrator restarts it; safe because atlas resumes from its persisted
/// cursor.
pub fn spawn(progress: StreamProgress, config: StallConfig) {
    info!(
        lag_threshold_blocks = config.threshold_blocks,
        stall_timeout_secs = config.timeout.as_secs(),
        check_interval_secs = CHECK_INTERVAL.as_secs(),
        "Substream stall detector armed (tip-relative)"
    );

    tokio::spawn(async move {
        let client = match reqwest::Client::builder().timeout(RPC_TIMEOUT).build() {
            Ok(client) => client,
            Err(err) => {
                // Fail open: no detector is better than a detector that cannot
                // read the tip and might act on a bad comparison.
                error!(error = %err, "Could not build the stall detector's RPC client; stall detection is off");
                return;
            }
        };

        let mut tracker = LagTracker::new(config.threshold_blocks, config.timeout);
        let mut ticker = tokio::time::interval(CHECK_INTERVAL);
        loop {
            ticker.tick().await;
            let tip = fetch_tip(&client, &config.rpc_url).await;
            let processed = progress.last_block();

            match tracker.observe(processed, tip, unix_secs()) {
                StallVerdict::Healthy { .. } | StallVerdict::Unknown => {}
                StallVerdict::Lagging { lag, lagging_secs } => {
                    warn!(
                        lag,
                        lagging_secs,
                        last_block = processed,
                        stall_timeout_secs = config.timeout.as_secs(),
                        "Atlas is behind the chain tip; will restart if this persists"
                    );
                }
                StallVerdict::Stalled {
                    lag,
                    last_block,
                    lagging_secs,
                } => {
                    // Exit rather than trying to rebuild the stream in place: a
                    // restart is the remedy already known to work, and it also
                    // clears any other state the dropped connection left behind.
                    error!(
                        lag,
                        last_block,
                        lagging_secs,
                        stall_timeout_secs = config.timeout.as_secs(),
                        "Atlas has been behind the chain tip past the stall timeout; exiting so \
                         the orchestrator restarts us. Atlas resumes from its persisted cursor."
                    );
                    std::process::exit(1);
                }
            }
        }
    });
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(1200);
    const THRESHOLD: u64 = 10;

    fn tracker() -> LagTracker {
        LagTracker::new(THRESHOLD, TIMEOUT)
    }

    /// The regression test for the bug this module shipped with. Chain 55516 is
    /// transaction-driven; the longest genuinely-idle window measured over a
    /// representative week was 130 minutes, verified against two independent
    /// metric series. A silence-based detector restarted atlas 402 times a week
    /// on exactly this pattern. Lag-based detection must stay silent throughout.
    #[test]
    fn the_longest_observed_idle_window_is_not_a_stall() {
        let mut t = tracker();
        let mut now = 1_000;
        // 130 minutes of a completely idle chain: tip and processed both frozen.
        for _ in 0..130 {
            now += 60;
            assert_eq!(
                t.observe(25_748, Some(25_748), now),
                StallVerdict::Healthy { lag: 0 },
                "an idle chain must never read as a stall"
            );
        }
    }

    #[test]
    fn a_wedged_stream_trips_once_the_lag_persists() {
        // The real outage: processed frozen at 27694 while the tip ran to 29448.
        let mut t = tracker();
        assert_eq!(
            t.observe(27_694, Some(27_700), 1_000),
            StallVerdict::Healthy { lag: 6 },
            "6 blocks behind is within measured healthy range"
        );
        // Tip pulls away past the threshold.
        assert!(matches!(
            t.observe(27_694, Some(27_750), 1_060),
            StallVerdict::Lagging { .. }
        ));
        // Still lagging when the timeout elapses.
        assert_eq!(
            t.observe(27_694, Some(29_448), 1_060 + 1_200),
            StallVerdict::Stalled {
                lag: 1_754,
                last_block: 27_694,
                lagging_secs: 1_200
            }
        );
    }

    #[test]
    fn a_brief_lag_spike_does_not_trip() {
        let mut t = tracker();
        assert!(matches!(
            t.observe(29_000, Some(29_100), 1_000),
            StallVerdict::Lagging { lag: 100, .. }
        ));
        // Caught up well inside the timeout.
        assert_eq!(
            t.observe(29_100, Some(29_100), 1_300),
            StallVerdict::Healthy { lag: 0 }
        );
        // And the clock was cleared, so a later spike starts fresh rather than
        // inheriting the earlier one and tripping immediately.
        assert!(matches!(
            t.observe(29_100, Some(29_300), 1_360),
            StallVerdict::Lagging {
                lagging_secs: 0,
                ..
            }
        ));
    }

    #[test]
    fn the_cold_start_replay_does_not_trip() {
        // 19s for ~1,750 blocks measured in production. Lag starts huge and
        // collapses long before the timeout.
        let mut t = tracker();
        let mut now = 1_000;
        for processed in [27_694, 28_300, 28_900, 29_400, 29_438] {
            let v = t.observe(processed, Some(29_438), now);
            assert!(
                !matches!(v, StallVerdict::Stalled { .. }),
                "cold-start replay must not trip: {v:?}"
            );
            now += 5;
        }
        assert_eq!(
            t.observe(29_438, Some(29_438), now),
            StallVerdict::Healthy { lag: 0 }
        );
    }

    #[test]
    fn head_skew_is_clamped_rather_than_read_as_lag() {
        // Measured skew reached 12 blocks ahead of the exporter's tip.
        let mut t = tracker();
        assert_eq!(
            t.observe(29_548, Some(29_536), 1_000),
            StallVerdict::Healthy { lag: 0 }
        );
    }

    #[test]
    fn an_unreadable_tip_never_trips() {
        let mut t = tracker();
        // Establish a real lag first, then lose the RPC for a long time.
        assert!(matches!(
            t.observe(27_694, Some(29_448), 1_000),
            StallVerdict::Lagging { .. }
        ));
        for i in 1..100 {
            assert_eq!(
                t.observe(27_694, None, 1_000 + i * 60),
                StallVerdict::Unknown,
                "an unreadable tip must fail open"
            );
        }
    }

    #[test]
    fn an_unreadable_tip_holds_the_clock_rather_than_resetting_it() {
        let mut t = tracker();
        assert!(matches!(
            t.observe(27_694, Some(29_448), 1_000),
            StallVerdict::Lagging {
                lagging_secs: 0,
                ..
            }
        ));
        // RPC unavailable across the whole timeout window...
        assert_eq!(t.observe(27_694, None, 1_600), StallVerdict::Unknown);
        // ...and once it returns, the still-present lag is acted on rather than
        // starting a fresh 20 minutes.
        assert!(matches!(
            t.observe(27_694, Some(29_448), 2_200),
            StallVerdict::Stalled {
                lagging_secs: 1_200,
                ..
            }
        ));
    }

    #[test]
    fn nothing_processed_yet_fails_open() {
        // A fresh process with no checkpoint: `tip - 0` is not a lag.
        let mut t = tracker();
        assert_eq!(t.observe(0, Some(29_536), 1_000), StallVerdict::Unknown);
        assert_eq!(
            t.observe(0, Some(29_536), 1_000_000),
            StallVerdict::Unknown,
            "must never trip on an unseeded process, however long it waits"
        );
    }

    #[test]
    fn progress_seeded_from_a_checkpoint_reports_that_block() {
        // Without seeding, a restart during an idle chain computes a lag of the
        // entire chain height and restarts a healthy process on a loop.
        let p = StreamProgress::seeded(29_438);
        assert_eq!(p.last_block(), 29_438);
        let mut t = tracker();
        assert_eq!(
            t.observe(p.last_block(), Some(29_438), 1_000),
            StallVerdict::Healthy { lag: 0 }
        );
    }

    #[test]
    fn unseeded_progress_reads_as_nothing_processed() {
        assert_eq!(StreamProgress::new().last_block(), 0);
    }

    #[test]
    fn progress_tracks_the_latest_block() {
        let p = StreamProgress::new();
        p.record_block(10);
        p.record_block(11);
        assert_eq!(p.last_block(), 11);
    }

    #[test]
    fn progress_is_shared_across_clones() {
        // The watchdog holds a clone; it must observe the sink's writes.
        let p = StreamProgress::new();
        let watcher = p.clone();
        p.record_block(29_500);
        assert_eq!(watcher.last_block(), 29_500);
    }

    #[test]
    fn a_lag_exactly_at_the_threshold_is_healthy() {
        let mut t = tracker();
        assert_eq!(
            t.observe(29_000, Some(29_010), 1_000),
            StallVerdict::Healthy { lag: 10 }
        );
    }
}
