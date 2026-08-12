//! Substream stall detection.
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
//! on a stall it exits non-zero and lets the orchestrator restart the process.
//!
//! ## Why this keys on block advance
//!
//! **Not** on checkpoint writes or canonical changes. Those are legitimately
//! sparse — real checkpoints landed hours apart (25278 → 25652 → 25791 → …)
//! because atlas only persists when the graph actually changes. A detector
//! watching for state changes would fire constantly on a healthy but quiet
//! stream.
//!
//! Block arrival is the honest signal: chain 55516 produces blocks every couple
//! of seconds, so a healthy stream is never idle for long. It also stays quiet
//! during a legitimate long catch-up — replaying 1,750 blocks still advances
//! blocks the whole way — which is the trap that made a latency-based liveness
//! probe kill busy-but-alive api pods (#880).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hermes_instrumentation::{error, info, warn};

/// How long the stream may deliver nothing before it is treated as wedged.
///
/// Generous on purpose: a healthy stream sees blocks every few seconds, so this
/// is ~two orders of magnitude above normal, and it must never fire on a slow
/// block or a brief reconnect.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(600);

/// How often the watchdog re-checks. Bounds how long a real stall persists
/// beyond the timeout.
const CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Shared record of the last block the stream delivered.
///
/// Cheap enough to touch on every block: two atomics, no lock, so the hot path
/// is unaffected.
#[derive(Debug, Clone, Default)]
pub struct StreamProgress {
    inner: Arc<ProgressInner>,
}

#[derive(Debug, Default)]
struct ProgressInner {
    /// Highest block seen. `0` means "nothing yet".
    last_block: AtomicU64,
    /// Monotonic-ish marker for when that block arrived, in seconds since the
    /// watchdog's epoch. Written by the stream, read by the watchdog.
    last_advance_secs: AtomicU64,
}

impl StreamProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a block arrived. Called from the block handler.
    pub fn record_block(&self, block_number: u64, now_secs: u64) {
        self.inner.last_block.store(block_number, Ordering::Relaxed);
        self.inner
            .last_advance_secs
            .store(now_secs, Ordering::Relaxed);
    }

    pub fn last_block(&self) -> u64 {
        self.inner.last_block.load(Ordering::Relaxed)
    }

    pub fn last_advance_secs(&self) -> u64 {
        self.inner.last_advance_secs.load(Ordering::Relaxed)
    }
}

/// What the watchdog decided on one tick. Separated from the exiting so the
/// decision is unit-testable without killing the test process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallVerdict {
    /// Blocks are arriving, or not enough time has passed to judge.
    Healthy,
    /// Nothing has arrived for longer than the timeout — restart.
    Stalled { idle_secs: u64, last_block: u64 },
}

/// Decide whether the stream is wedged.
///
/// `started_secs` is when the watchdog began, used as the baseline before the
/// first block arrives — otherwise a slow startup (restore, Kafka connect, the
/// initial substream handshake) would look like an instant stall.
pub fn classify(
    progress: &StreamProgress,
    started_secs: u64,
    now_secs: u64,
    timeout: Duration,
) -> StallVerdict {
    let last_advance = progress.last_advance_secs();
    // No block yet: judge against startup, so a stream that never delivers its
    // first block is still caught rather than waiting forever.
    let reference = if last_advance == 0 {
        started_secs
    } else {
        last_advance
    };

    let idle_secs = now_secs.saturating_sub(reference);
    if idle_secs >= timeout.as_secs() {
        StallVerdict::Stalled {
            idle_secs,
            last_block: progress.last_block(),
        }
    } else {
        StallVerdict::Healthy
    }
}

/// Read the stall timeout from `ATLAS_STALL_TIMEOUT_SECS`.
///
/// `0` disables detection — an escape hatch for local runs against a mock or a
/// deliberately paused stream, where no blocks arriving is expected.
pub fn timeout_from_env() -> Option<Duration> {
    match std::env::var("ATLAS_STALL_TIMEOUT_SECS") {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(0) => {
                warn!("ATLAS_STALL_TIMEOUT_SECS=0 — substream stall detection disabled");
                None
            }
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(err) => {
                warn!(
                    value = %raw,
                    error = %err,
                    default_secs = DEFAULT_STALL_TIMEOUT.as_secs(),
                    "Invalid ATLAS_STALL_TIMEOUT_SECS; using default"
                );
                Some(DEFAULT_STALL_TIMEOUT)
            }
        },
        Err(_) => Some(DEFAULT_STALL_TIMEOUT),
    }
}

/// Spawn the watchdog. Exits the process on a stall so the orchestrator restarts
/// it; safe because atlas resumes from its persisted cursor.
pub fn spawn(progress: StreamProgress, timeout: Duration) {
    let started = unix_secs();
    info!(
        stall_timeout_secs = timeout.as_secs(),
        check_interval_secs = CHECK_INTERVAL.as_secs(),
        "Substream stall detector armed"
    );

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CHECK_INTERVAL);
        loop {
            ticker.tick().await;
            match classify(&progress, started, unix_secs(), timeout) {
                StallVerdict::Healthy => {}
                StallVerdict::Stalled {
                    idle_secs,
                    last_block,
                } => {
                    // Exit rather than trying to rebuild the stream in place: a
                    // restart is the remedy already known to work, and it also
                    // clears any other state the dropped connection left behind.
                    error!(
                        idle_secs,
                        last_block,
                        stall_timeout_secs = timeout.as_secs(),
                        "Substream delivered no blocks past the stall timeout; exiting so the \
                         orchestrator restarts us. Atlas resumes from its persisted cursor."
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

    const TIMEOUT: Duration = Duration::from_secs(600);

    #[test]
    fn healthy_while_blocks_keep_arriving() {
        let p = StreamProgress::new();
        p.record_block(29_250, 1_000);
        // 5 minutes idle, under the 10-minute timeout.
        assert_eq!(classify(&p, 0, 1_300, TIMEOUT), StallVerdict::Healthy);
    }

    #[test]
    fn stalls_once_the_timeout_elapses_with_no_new_block() {
        let p = StreamProgress::new();
        p.record_block(29_250, 1_000);
        assert_eq!(
            classify(&p, 0, 1_600, TIMEOUT),
            StallVerdict::Stalled {
                idle_secs: 600,
                last_block: 29_250
            }
        );
    }

    #[test]
    fn a_slow_catch_up_is_not_a_stall() {
        // Replaying ~1,750 blocks takes a while but blocks keep advancing, which
        // is exactly the case a latency-based probe gets wrong (#880).
        let p = StreamProgress::new();
        let mut now = 1_000;
        for block in 27_694..27_894 {
            p.record_block(block, now);
            now += 5; // 5s per block: slow, but progressing
            assert_eq!(classify(&p, 0, now, TIMEOUT), StallVerdict::Healthy);
        }
    }

    #[test]
    fn startup_is_measured_from_launch_not_from_zero() {
        // No block yet. Without the startup baseline, `last_advance == 0` would
        // make every fresh process look instantly stalled.
        let p = StreamProgress::new();
        assert_eq!(classify(&p, 1_000, 1_100, TIMEOUT), StallVerdict::Healthy);
    }

    #[test]
    fn a_stream_that_never_delivers_a_first_block_still_trips() {
        let p = StreamProgress::new();
        assert_eq!(
            classify(&p, 1_000, 1_600, TIMEOUT),
            StallVerdict::Stalled {
                idle_secs: 600,
                last_block: 0
            }
        );
    }

    #[test]
    fn recording_a_block_clears_a_pending_stall() {
        let p = StreamProgress::new();
        p.record_block(1, 1_000);
        assert!(matches!(
            classify(&p, 0, 1_700, TIMEOUT),
            StallVerdict::Stalled { .. }
        ));
        // A reconnect that starts delivering again must un-stall it.
        p.record_block(2, 1_700);
        assert_eq!(classify(&p, 0, 1_700, TIMEOUT), StallVerdict::Healthy);
    }

    #[test]
    fn progress_tracks_the_latest_block_and_time() {
        let p = StreamProgress::new();
        p.record_block(10, 500);
        p.record_block(11, 505);
        assert_eq!(p.last_block(), 11);
        assert_eq!(p.last_advance_secs(), 505);
    }
}
