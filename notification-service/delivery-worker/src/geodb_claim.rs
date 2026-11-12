//! Claiming deliveries from geo-db's work class instead of Postgres (plan 10.16, GAP-15).
//!
//! Off unless `DELIVERY_WORKER_CLAIM_SOURCE=geodb`. Unset (or any other value) leaves Postgres
//! the source and the Postgres path in `main.rs` and `storage.rs` untouched.
//!
//! With the flag on, the poll loop claims a batch over the held `geodb-client work pipe`
//! ([`geodb_outbox::reader::OutboxReader`]) under a lease, delivers each unit exactly as the
//! Postgres path does (same signature, same retry policy, same backoff), and settles it with one
//! pipe verb:
//!
//! - delivered: `complete` (not epoch-fenced, so a late ack after a lapsed lease is harmless);
//! - retry later: `retry` with the backoff as the delay (epoch-fenced);
//! - give up: `fail` (epoch-fenced).
//!
//! There is no reaper: a unit whose worker died is claimed again once its lease lapses, with a
//! new epoch, and the dead worker's `retry` or `fail` then answers `lease_lost` and changes
//! nothing. The webhook url and secret still come from `app_webhooks` (or the mounted file) by
//! the webhook id in the unit's key; a webhook that is gone fails the unit. `last_error` is not
//! stored by the work class (W6) and goes to the log line only.
//!
//! The shadow reader (`GEODB_OUTBOX_SHADOW_READ`) completes units from the same class, so the
//! two must not run together: `main` turns the shadow off when this is on.

use std::future::Future;
use std::time::Duration;

use geodb_outbox::reader::{is_marker, Ack, Claimed, OutboxReader};
use geodb_outbox::{Config, MirrorError};
use tokio::sync::Mutex;

use crate::deliver::{backoff_seconds, is_success, should_retry};

pub const ENV_VAR: &str = "DELIVERY_WORKER_CLAIM_SOURCE";
pub const LEASE_ENV_VAR: &str = "DELIVERY_WORKER_CLAIM_LEASE_SECS";

/// The plan's default lease, the same as the Postgres reaper's stale threshold.
pub const DEFAULT_LEASE_SECS: u64 = 300;

pub const CLAIMED_TOTAL: &str = "delivery_worker_geodb_claimed_total";
pub const ACKS_TOTAL: &str = "delivery_worker_geodb_acks_total";

/// Where `claim` takes deliveries from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimSource {
    Postgres,
    GeoDb,
}

/// Parse `DELIVERY_WORKER_CLAIM_SOURCE`. Only an explicit `geodb` selects geo-db; unset, empty
/// or anything else is Postgres, so a typo never moves the queue.
pub fn claim_source_from(value: Option<&str>) -> ClaimSource {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("geodb") => ClaimSource::GeoDb,
        _ => ClaimSource::Postgres,
    }
}

/// The pipe config for the claimer, from the shared `GEODB_CLIENT_BIN`, `GEODB_OUTBOX_KEY_PATH`
/// and `GEODB_BINDING_PATH`. `None` when the key or binding is unset.
pub fn config_from(get: impl Fn(&str) -> Option<String>) -> Option<Config> {
    Config::from_vars_flag(
        |k| {
            if k == ENV_VAR {
                Some("1".into())
            } else {
                get(k)
            }
        },
        ENV_VAR,
    )
}

/// The lease, from `DELIVERY_WORKER_CLAIM_LEASE_SECS` (default 300, never 0).
pub fn lease_secs_from(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_LEASE_SECS)
}

/// The four pipe verbs the worker needs. [`OutboxReader`] is the real one; tests use a fake.
pub trait WorkPipe: Send {
    fn claim(
        &mut self,
        limit: u16,
        lease_ms: u64,
    ) -> impl Future<Output = Result<Vec<Claimed>, MirrorError>> + Send;
    fn complete(&mut self, c: &Claimed) -> impl Future<Output = Result<Ack, MirrorError>> + Send;
    fn retry(
        &mut self,
        c: &Claimed,
        delay_ms: u64,
    ) -> impl Future<Output = Result<Ack, MirrorError>> + Send;
    fn fail(&mut self, c: &Claimed) -> impl Future<Output = Result<Ack, MirrorError>> + Send;
}

impl WorkPipe for OutboxReader {
    async fn claim(&mut self, limit: u16, lease_ms: u64) -> Result<Vec<Claimed>, MirrorError> {
        OutboxReader::claim(self, limit, lease_ms).await
    }
    async fn complete(&mut self, c: &Claimed) -> Result<Ack, MirrorError> {
        OutboxReader::complete(self, c).await
    }
    async fn retry(&mut self, c: &Claimed, delay_ms: u64) -> Result<Ack, MirrorError> {
        OutboxReader::retry(self, c, delay_ms).await
    }
    async fn fail(&mut self, c: &Claimed) -> Result<Ack, MirrorError> {
        OutboxReader::fail(self, c).await
    }
}

/// What to do with a unit after its delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Delivered,
    Retry { delay_ms: u64 },
    Fail,
}

/// The Postgres path's policy (`deliver_one`), as a pure function. `result` is the HTTP status,
/// or `Err` for a network failure (which retries regardless of status class).
pub fn decide(attempts: u32, max_retries: i32, result: &Result<u16, ()>) -> Decision {
    let attempt = i32::try_from(attempts)
        .unwrap_or(i32::MAX)
        .saturating_add(1);
    match result {
        Ok(status) if is_success(*status) => Decision::Delivered,
        Ok(status) if attempt >= max_retries || !should_retry(*status) => Decision::Fail,
        Err(()) if attempt >= max_retries => Decision::Fail,
        _ => Decision::Retry {
            delay_ms: u64::try_from(backoff_seconds(attempt))
                .unwrap_or(0)
                .saturating_mul(1000),
        },
    }
}

/// How a settle ended, for the caller's counters.
#[derive(Debug, PartialEq, Eq)]
pub enum Settled {
    Acked,
    /// Another claim owns the unit now; this worker's retry or fail changed nothing.
    LeaseLost,
    /// The pipe failed or answered something unexpected. The unit stays leased and comes back
    /// when the lease lapses.
    PipeError,
}

/// Claims and settles units over one pipe shared by the concurrent delivery tasks.
pub struct GeoDbClaimer<P: WorkPipe> {
    pipe: Mutex<P>,
    lease_ms: u64,
}

impl GeoDbClaimer<OutboxReader> {
    pub fn from_config(cfg: Config, lease: Duration) -> Self {
        Self::new(OutboxReader::new(cfg), lease)
    }
}

impl<P: WorkPipe> GeoDbClaimer<P> {
    pub fn new(pipe: P, lease: Duration) -> Self {
        Self {
            pipe: Mutex::new(pipe),
            lease_ms: u64::try_from(lease.as_millis()).unwrap_or(u64::MAX),
        }
    }

    pub fn lease_ms(&self) -> u64 {
        self.lease_ms
    }

    /// Claims up to `limit` deliveries. Start markers a shadow reader may have left in the class
    /// are not deliveries: they are completed here and dropped from the batch.
    pub async fn claim(&self, limit: u16) -> Result<Vec<Claimed>, MirrorError> {
        let mut pipe = self.pipe.lock().await;
        let units = pipe.claim(limit, self.lease_ms).await?;
        let mut out = Vec::with_capacity(units.len());
        for u in units {
            if is_marker(&u.key) {
                let _ = pipe.complete(&u).await;
            } else {
                out.push(u);
            }
        }
        metrics::counter!(CLAIMED_TOTAL).increment(out.len() as u64);
        Ok(out)
    }

    /// Settles a unit with the verb its decision names.
    pub async fn settle(&self, c: &Claimed, d: Decision) -> Settled {
        let mut pipe = self.pipe.lock().await;
        let ack = match d {
            Decision::Delivered => pipe.complete(c).await,
            Decision::Retry { delay_ms } => pipe.retry(c, delay_ms).await,
            Decision::Fail => pipe.fail(c).await,
        };
        let (settled, label) = match ack {
            Ok(Ack::Ok | Ack::AlreadyTerminal) => (Settled::Acked, "ok"),
            Ok(Ack::LeaseLost) => (Settled::LeaseLost, "lease_lost"),
            Ok(Ack::Unknown) | Err(_) => (Settled::PipeError, "error"),
        };
        metrics::counter!(ACKS_TOTAL, "result" => label).increment(1);
        settled
    }
}

/// Registers the counters at zero, only when the claimer is on, so the default worker's series
/// are unchanged.
pub fn register() {
    metrics::counter!(CLAIMED_TOTAL).absolute(0);
    for l in ["ok", "lease_lost", "error"] {
        metrics::counter!(ACKS_TOTAL, "result" => l).absolute(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geodb_outbox::reader::{KEY_BYTES, MARK};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    type Rows = BTreeMap<[u8; KEY_BYTES], (State, u32)>;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum State {
        Pending { due: u64 },
        InProgress { lease_until: u64, epoch: u64 },
        Delivered,
        Failed,
    }

    /// An in-memory work class with the pipe's semantics (plan 10.16 section 3): claims take
    /// `pending` rows that are due and `in_progress` rows whose lease lapsed, oldest first, with
    /// a fresh epoch; `complete` is not fenced; `retry` and `fail` are.
    #[derive(Clone)]
    struct FakePipe {
        now: Arc<AtomicU64>,
        rows: Arc<std::sync::Mutex<Rows>>,
        epoch: Arc<AtomicU64>,
    }

    impl FakePipe {
        fn new() -> Self {
            Self {
                now: Arc::new(AtomicU64::new(1_000)),
                rows: Arc::default(),
                epoch: Arc::new(AtomicU64::new(0)),
            }
        }
        fn enter(&self, tag: u8) -> [u8; KEY_BYTES] {
            let k = [tag; KEY_BYTES];
            self.rows
                .lock()
                .unwrap()
                .insert(k, (State::Pending { due: 0 }, 0));
            k
        }
        fn advance(&self, ms: u64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }
        fn state(&self, k: &[u8; KEY_BYTES]) -> (State, u32) {
            self.rows.lock().unwrap()[k]
        }
    }

    impl WorkPipe for FakePipe {
        async fn claim(&mut self, limit: u16, lease_ms: u64) -> Result<Vec<Claimed>, MirrorError> {
            let now = self.now.load(Ordering::SeqCst);
            let mut rows = self.rows.lock().unwrap();
            let mut out = Vec::new();
            for (k, (st, attempts)) in rows.iter_mut() {
                if out.len() >= usize::from(limit) {
                    break;
                }
                let claimable = match *st {
                    State::Pending { due } => due <= now,
                    State::InProgress { lease_until, .. } => lease_until <= now,
                    _ => false,
                };
                if claimable {
                    let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
                    *st = State::InProgress {
                        lease_until: now + lease_ms,
                        epoch,
                    };
                    out.push(Claimed {
                        key: *k,
                        attempts: *attempts,
                        epoch,
                        payload: b"{}".to_vec(),
                    });
                }
            }
            Ok(out)
        }
        async fn complete(&mut self, c: &Claimed) -> Result<Ack, MirrorError> {
            let mut rows = self.rows.lock().unwrap();
            Ok(match rows.get_mut(&c.key) {
                None => Ack::Unknown,
                Some((State::Delivered | State::Failed, _)) => Ack::AlreadyTerminal,
                Some((st, _)) => {
                    *st = State::Delivered;
                    Ack::Ok
                }
            })
        }
        async fn retry(&mut self, c: &Claimed, delay_ms: u64) -> Result<Ack, MirrorError> {
            let now = self.now.load(Ordering::SeqCst);
            let mut rows = self.rows.lock().unwrap();
            Ok(match rows.get_mut(&c.key) {
                None => Ack::Unknown,
                Some((State::Delivered | State::Failed, _)) => Ack::AlreadyTerminal,
                Some((st @ State::InProgress { .. }, attempts)) if matches!(*st, State::InProgress { epoch, .. } if epoch == c.epoch) =>
                {
                    *st = State::Pending {
                        due: now + delay_ms,
                    };
                    *attempts += 1;
                    Ack::Ok
                }
                Some(_) => Ack::LeaseLost,
            })
        }
        async fn fail(&mut self, c: &Claimed) -> Result<Ack, MirrorError> {
            let mut rows = self.rows.lock().unwrap();
            Ok(match rows.get_mut(&c.key) {
                None => Ack::Unknown,
                Some((State::Delivered | State::Failed, _)) => Ack::AlreadyTerminal,
                Some((st @ State::InProgress { .. }, attempts)) if matches!(*st, State::InProgress { epoch, .. } if epoch == c.epoch) =>
                {
                    *st = State::Failed;
                    *attempts += 1;
                    Ack::Ok
                }
                Some(_) => Ack::LeaseLost,
            })
        }
    }

    fn claimer(p: &FakePipe) -> GeoDbClaimer<FakePipe> {
        GeoDbClaimer::new(p.clone(), Duration::from_secs(300))
    }

    #[test]
    fn the_flag_selects_geodb_only_when_explicit() {
        assert_eq!(claim_source_from(None), ClaimSource::Postgres);
        assert_eq!(claim_source_from(Some("")), ClaimSource::Postgres);
        assert_eq!(claim_source_from(Some("postgres")), ClaimSource::Postgres);
        assert_eq!(claim_source_from(Some("geo-db")), ClaimSource::Postgres);
        assert_eq!(claim_source_from(Some("1")), ClaimSource::Postgres);
        assert_eq!(claim_source_from(Some(" GeoDB ")), ClaimSource::GeoDb);
        assert_eq!(lease_secs_from(None), 300);
        assert_eq!(lease_secs_from(Some("0")), 300);
        assert_eq!(lease_secs_from(Some("45")), 45);
    }

    #[test]
    fn the_pipe_config_needs_the_key_and_binding() {
        let none = |_: &str| None;
        assert!(config_from(none).is_none());
        let cfg = config_from(|k| match k {
            "GEODB_OUTBOX_KEY_PATH" => Some("/k".into()),
            "GEODB_BINDING_PATH" => Some("/b".into()),
            _ => None,
        })
        .unwrap();
        assert!(cfg.args.iter().any(|a| a == "pipe"));
    }

    #[test]
    fn the_policy_matches_the_postgres_path() {
        assert_eq!(decide(0, 100, &Ok(200)), Decision::Delivered);
        assert_eq!(decide(0, 100, &Ok(409)), Decision::Delivered);
        assert_eq!(
            decide(0, 100, &Ok(500)),
            Decision::Retry { delay_ms: 30_000 }
        );
        assert_eq!(
            decide(1, 100, &Err(())),
            Decision::Retry { delay_ms: 60_000 }
        );
        assert_eq!(decide(99, 100, &Ok(500)), Decision::Fail);
        assert_eq!(decide(99, 100, &Err(())), Decision::Fail);
        // The backoff caps at two days.
        assert_eq!(
            decide(40, 100, &Err(())),
            Decision::Retry {
                delay_ms: 172_800_000
            }
        );
    }

    #[tokio::test]
    async fn claim_then_complete_delivers_the_unit() {
        let p = FakePipe::new();
        let k = p.enter(1);
        let c = claimer(&p);
        let got = c.claim(10).await.unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(p.state(&k).0, State::InProgress { .. }));
        assert_eq!(c.settle(&got[0], Decision::Delivered).await, Settled::Acked);
        assert_eq!(p.state(&k), (State::Delivered, 0));
        // Nothing is left to claim.
        assert!(c.claim(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn retry_reschedules_after_the_backoff_and_counts_the_try() {
        let p = FakePipe::new();
        let k = p.enter(1);
        let c = claimer(&p);
        let got = c.claim(10).await.unwrap();
        assert_eq!(
            c.settle(&got[0], Decision::Retry { delay_ms: 30_000 })
                .await,
            Settled::Acked
        );
        assert_eq!(p.state(&k).1, 1);
        assert!(c.claim(10).await.unwrap().is_empty(), "not due yet");
        p.advance(30_000);
        let again = c.claim(10).await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].attempts, 1);
    }

    #[tokio::test]
    async fn fail_ends_the_unit_for_good() {
        let p = FakePipe::new();
        let k = p.enter(1);
        let c = claimer(&p);
        let got = c.claim(10).await.unwrap();
        assert_eq!(c.settle(&got[0], Decision::Fail).await, Settled::Acked);
        assert_eq!(p.state(&k), (State::Failed, 1));
        p.advance(10_000_000);
        assert!(c.claim(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_expired_lease_is_claimed_again_and_the_old_claim_is_fenced() {
        let p = FakePipe::new();
        let k = p.enter(1);
        let c = claimer(&p);
        let first = c.claim(10).await.unwrap().remove(0);
        // Within the lease nobody else gets it.
        p.advance(299_000);
        assert!(c.claim(10).await.unwrap().is_empty());
        // After the lease lapses a new claim takes it with a new epoch.
        p.advance(1_000);
        let second = c.claim(10).await.unwrap().remove(0);
        assert_eq!(second.key, k);
        assert_ne!(second.epoch, first.epoch);
        // The first worker's retry and fail answer lease_lost and change nothing.
        assert_eq!(
            c.settle(&first, Decision::Retry { delay_ms: 1 }).await,
            Settled::LeaseLost
        );
        assert_eq!(c.settle(&first, Decision::Fail).await, Settled::LeaseLost);
        assert!(matches!(p.state(&k).0, State::InProgress { epoch, .. } if epoch == second.epoch));
        assert_eq!(p.state(&k).1, 0, "a fenced ack does not count a try");
        // The live claim still settles.
        assert_eq!(c.settle(&second, Decision::Delivered).await, Settled::Acked);
    }

    #[tokio::test]
    async fn two_workers_never_hold_the_same_unit_at_once() {
        let p = FakePipe::new();
        for t in 1..=5 {
            p.enter(t);
        }
        let (a, b) = (claimer(&p), claimer(&p));
        let (ga, gb) = tokio::join!(a.claim(3), b.claim(3));
        let (ga, gb) = (ga.unwrap(), gb.unwrap());
        assert_eq!(ga.len() + gb.len(), 5, "every unit claimed once");
        for x in &ga {
            assert!(gb.iter().all(|y| y.key != x.key), "double claim");
        }
        // A second claim finds nothing while every lease is live.
        assert!(a.claim(10).await.unwrap().is_empty());
        assert!(b.claim(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_start_marker_is_completed_and_not_delivered() {
        let p = FakePipe::new();
        let m = [MARK; KEY_BYTES];
        p.rows
            .lock()
            .unwrap()
            .insert(m, (State::Pending { due: 0 }, 0));
        let k = p.enter(1);
        let got = claimer(&p).claim(10).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, k);
        assert_eq!(p.state(&m).0, State::Delivered);
    }
}
