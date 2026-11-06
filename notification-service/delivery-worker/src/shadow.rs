//! The delivery-side shadow of geo-db's work class (plan 10.16, GAP-15). Measurement only.
//!
//! Postgres stays the queue and the only thing that delivers. When a delivery reaches a terminal
//! state in Postgres (`delivered`, or `failed` for good) the delivery loop hands the shadow
//! that row's identity and payload with a `try_send`: it never waits and never fails the loop.
//! A background task claims the units the indexer mirrored into geo-db, pairs each with the
//! Postgres row delivered for the same (ledger key, webhook id), and completes the unit. It
//! delivers nothing and writes nothing to Postgres.
//!
//! Results, counted on `delivery_worker_shadow_total{result}`:
//! - `match`: the unit and the delivered row agree on the payload.
//! - `mismatch`: both exist for the key but the payload differs (JSON-equal comparison, since
//!   Postgres `jsonb` reorders keys).
//! - `miss`: Postgres delivered a row geo-db never had (not mirrored, dropped, or not yet
//!   claimed) within `miss_grace` of the delivery.
//! - `extra`: geo-db holds a unit whose Postgres delivery never reached a terminal state within
//!   `extra_grace` of the claim. That grace must outlast the retry backoff (capped at two days,
//!   [`crate::deliver::backoff_seconds`]), so the default is three days.
//!
//! Rows delivered before the mirror was switched on show as `miss`; read the series from the
//! mirror's start. The shadow is OFF unless `GEODB_OUTBOX_SHADOW_READ` is `1` or `true`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use geodb_outbox::reader::{Ack, Claimed, OutboxReader, KEY_BYTES};
use geodb_outbox::MirrorError;
use hermes_instrumentation::{debug, warn};
use tokio::sync::mpsc;
use uuid::Uuid;

pub const RESULTS: &str = "delivery_worker_shadow_total";
pub const ERRORS: &str = "delivery_worker_shadow_errors_total";
pub const DROPPED: &str = "delivery_worker_shadow_dropped_total";
pub const PENDING_UNITS: &str = "delivery_worker_shadow_pending_units";
pub const PENDING_ROWS: &str = "delivery_worker_shadow_pending_rows";

/// Observations waiting for the shadow task. A burst past this is dropped, counted.
pub const QUEUE_MAX: usize = 4096;

/// Most unresolved units, and most unresolved Postgres rows, held at once. Past it the shadow
/// stops claiming and drops observations, counted: memory stays bounded.
pub const HELD_MAX: usize = 20_000;

/// A delivery's identity: the ledger key (the outbox `idempotency_key` decoded) then the
/// webhook id, the key geo-db's `delivery` collection uses.
pub type Key = [u8; KEY_BYTES];

/// The key for a Postgres delivery, or `None` when `idempotency_key` is not 64 hex digits.
pub fn key_of(idempotency_key: &str, webhook_id: Uuid) -> Option<Key> {
    let ledger = geodb_outbox::reader::unhex(idempotency_key)?;
    if ledger.len() != 32 {
        return None;
    }
    let mut k = [0u8; KEY_BYTES];
    k[..32].copy_from_slice(&ledger);
    k[32..].copy_from_slice(webhook_id.as_bytes());
    Some(k)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Match,
    Mismatch,
    Miss,
    Extra,
}

impl Verdict {
    const ALL: [Verdict; 4] = [
        Verdict::Match,
        Verdict::Mismatch,
        Verdict::Miss,
        Verdict::Extra,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Verdict::Match => "match",
            Verdict::Mismatch => "mismatch",
            Verdict::Miss => "miss",
            Verdict::Extra => "extra",
        }
    }
}

/// A terminal Postgres delivery, as the loop saw it.
pub struct Observation {
    pub key: Key,
    pub payload: serde_json::Value,
}

/// The handle the delivery loop holds. Cheap to clone; `observe` never blocks.
#[derive(Clone)]
pub struct Shadow {
    tx: mpsc::Sender<Observation>,
}

impl Shadow {
    /// Reports a delivery that reached a terminal state in Postgres. A row whose key cannot be
    /// formed or a full queue is counted and dropped; the caller is never affected.
    pub fn observe(&self, idempotency_key: &str, webhook_id: Uuid, payload: &serde_json::Value) {
        let Some(key) = key_of(idempotency_key, webhook_id) else {
            metrics::counter!(DROPPED, "reason" => "bad_key").increment(1);
            return;
        };
        let obs = Observation {
            key,
            payload: payload.clone(),
        };
        if self.tx.try_send(obs).is_err() {
            metrics::counter!(DROPPED, "reason" => "queue_full").increment(1);
        }
    }
}

/// What a Postgres delivery's observation came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Observed {
    Resolved(Verdict, Claimed),
    Waiting,
    Full,
}

/// Pairs claimed units with observed deliveries. Pure: time is passed in.
pub struct Matcher {
    miss_grace: Duration,
    extra_grace: Duration,
    /// Claimed units whose Postgres delivery has not been seen, with when first claimed.
    units: HashMap<Key, (Claimed, Instant)>,
    /// Terminal Postgres rows whose unit has not been claimed, with when observed.
    rows: HashMap<Key, (serde_json::Value, Instant)>,
}

fn compare(unit: &Claimed, row: &serde_json::Value) -> Verdict {
    match serde_json::from_slice::<serde_json::Value>(&unit.payload) {
        Ok(v) if &v == row => Verdict::Match,
        _ => Verdict::Mismatch,
    }
}

impl Matcher {
    pub fn new(miss_grace: Duration, extra_grace: Duration) -> Self {
        Self {
            miss_grace,
            extra_grace,
            units: HashMap::new(),
            rows: HashMap::new(),
        }
    }

    pub fn units_held(&self) -> usize {
        self.units.len()
    }

    pub fn rows_held(&self) -> usize {
        self.rows.len()
    }

    /// A unit was claimed. Resolves at once if its row was already observed. A unit claimed
    /// again after its lease lapsed keeps its first-claimed time and takes the new epoch.
    pub fn claimed(&mut self, unit: Claimed, now: Instant) -> Option<(Verdict, Claimed)> {
        if let Some((row, _)) = self.rows.remove(&unit.key) {
            return Some((compare(&unit, &row), unit));
        }
        let since = self.units.get(&unit.key).map_or(now, |(_, t)| *t);
        self.units.insert(unit.key, (unit, since));
        None
    }

    /// A Postgres delivery reached a terminal state. Resolves at once if its unit is held;
    /// otherwise it waits for the unit, unless the wait list is full.
    pub fn observed(&mut self, obs: Observation, now: Instant) -> Observed {
        if let Some((unit, _)) = self.units.remove(&obs.key) {
            return Observed::Resolved(compare(&unit, &obs.payload), unit);
        }
        if self.rows.len() >= HELD_MAX && !self.rows.contains_key(&obs.key) {
            return Observed::Full;
        }
        self.rows.insert(obs.key, (obs.payload, now));
        Observed::Waiting
    }

    /// Ages out what waited too long: rows past `miss_grace` are misses (nothing to complete),
    /// units past `extra_grace` are extras (to complete). Returns the miss count and the extras.
    pub fn sweep(&mut self, now: Instant) -> (u64, Vec<Claimed>) {
        let before = self.rows.len();
        let grace = self.miss_grace;
        self.rows.retain(|_, (_, t)| now.duration_since(*t) < grace);
        let misses = (before - self.rows.len()) as u64;
        let grace = self.extra_grace;
        let stale: Vec<Key> = self
            .units
            .iter()
            .filter(|(_, (_, t))| now.duration_since(*t) >= grace)
            .map(|(k, _)| *k)
            .collect();
        let extras = stale
            .into_iter()
            .filter_map(|k| self.units.remove(&k).map(|(c, _)| c))
            .collect();
        (misses, extras)
    }
}

/// What claiming and completing units needs; `OutboxReader` in production, a fake in tests.
pub trait Source {
    fn claim(
        &mut self,
        limit: u16,
        lease_ms: u64,
    ) -> impl std::future::Future<Output = Result<Vec<Claimed>, MirrorError>> + Send;
    fn complete(
        &mut self,
        unit: &Claimed,
    ) -> impl std::future::Future<Output = Result<Ack, MirrorError>> + Send;
}

impl Source for OutboxReader {
    async fn claim(&mut self, limit: u16, lease_ms: u64) -> Result<Vec<Claimed>, MirrorError> {
        OutboxReader::claim(self, limit, lease_ms).await
    }
    async fn complete(&mut self, unit: &Claimed) -> Result<Ack, MirrorError> {
        OutboxReader::complete(self, unit).await
    }
}

/// What one pass resolved and what failed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub matches: u64,
    pub mismatches: u64,
    pub misses: u64,
    pub extras: u64,
    pub claim_errors: u64,
    pub complete_errors: u64,
}

impl Tally {
    fn add(&mut self, v: Verdict) {
        match v {
            Verdict::Match => self.matches += 1,
            Verdict::Mismatch => self.mismatches += 1,
            Verdict::Miss => self.misses += 1,
            Verdict::Extra => self.extras += 1,
        }
    }

    fn emit(&self) {
        for (v, n) in [
            (Verdict::Match, self.matches),
            (Verdict::Mismatch, self.mismatches),
            (Verdict::Miss, self.misses),
            (Verdict::Extra, self.extras),
        ] {
            if n > 0 {
                metrics::counter!(RESULTS, "result" => v.as_str()).increment(n);
            }
        }
        if self.claim_errors > 0 {
            metrics::counter!(ERRORS, "op" => "claim").increment(self.claim_errors);
        }
        if self.complete_errors > 0 {
            metrics::counter!(ERRORS, "op" => "complete").increment(self.complete_errors);
        }
    }
}

async fn finish<S: Source>(src: &mut S, unit: &Claimed, v: Verdict, t: &mut Tally) {
    t.add(v);
    // `complete` takes the unit out of geo-db's queue; a failure leaves it to be claimed again
    // after its lease, so it is counted and not retried here.
    match src.complete(unit).await {
        Ok(Ack::Ok | Ack::AlreadyTerminal | Ack::Unknown) => {}
        Ok(Ack::LeaseLost) => {}
        Err(e) => {
            t.complete_errors += 1;
            debug!(error = %e, "shadow complete failed");
        }
    }
}

/// One pass: drain what the loop observed, claim a batch, age out the old, complete what
/// resolved. Postgres is not touched.
pub async fn pass<S: Source>(
    src: &mut S,
    m: &mut Matcher,
    observed: Vec<Observation>,
    batch: u16,
    lease_ms: u64,
    now: Instant,
) -> Tally {
    let mut t = Tally::default();
    let mut done: Vec<(Verdict, Claimed)> = Vec::new();
    for obs in observed {
        match m.observed(obs, now) {
            Observed::Resolved(v, u) => done.push((v, u)),
            Observed::Waiting => {}
            Observed::Full => {
                metrics::counter!(DROPPED, "reason" => "rows_full").increment(1);
            }
        }
    }
    if m.units_held() < HELD_MAX {
        match src.claim(batch, lease_ms).await {
            Ok(units) => {
                for u in units {
                    if let Some(r) = m.claimed(u, now) {
                        done.push(r);
                    }
                }
            }
            Err(MirrorError::CoolingDown) => t.claim_errors += 1,
            Err(e) => {
                t.claim_errors += 1;
                warn!(error = %e, "geo-db shadow claim failed; Postgres delivery is unaffected");
            }
        }
    }
    let (misses, extras) = m.sweep(now);
    t.misses += misses;
    for (v, unit) in done {
        finish(src, &unit, v, &mut t).await;
    }
    for unit in extras {
        finish(src, &unit, Verdict::Extra, &mut t).await;
    }
    t
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub interval: Duration,
    pub batch: u16,
    pub lease: Duration,
    pub miss_grace: Duration,
    pub extra_grace: Duration,
}

impl Settings {
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        let secs =
            |k: &str, d: u64| Duration::from_secs(get(k).and_then(|s| s.parse().ok()).unwrap_or(d));
        Settings {
            interval: secs("GEODB_SHADOW_INTERVAL_SECS", 5),
            batch: get("GEODB_SHADOW_BATCH")
                .and_then(|s| s.parse().ok())
                .unwrap_or(16),
            lease: secs("GEODB_SHADOW_LEASE_SECS", 3600),
            miss_grace: secs("GEODB_SHADOW_MISS_GRACE_SECS", 600),
            extra_grace: secs("GEODB_SHADOW_EXTRA_GRACE_SECS", 3 * 24 * 3600),
        }
    }
}

/// Starts the shadow when `GEODB_OUTBOX_SHADOW_READ` is on and configured; `None` otherwise (the
/// default). Panics outside a tokio runtime.
pub fn start() -> Option<Shadow> {
    let reader = OutboxReader::from_env()?;
    let settings = Settings::from_vars(|k| std::env::var(k).ok());
    register();
    Some(spawn(reader, settings))
}

/// Starts the task over `src`.
pub fn spawn<S: Source + Send + 'static>(src: S, settings: Settings) -> Shadow {
    let (tx, rx) = mpsc::channel(QUEUE_MAX);
    tokio::spawn(run(src, settings, rx));
    Shadow { tx }
}

async fn run<S: Source>(mut src: S, s: Settings, mut rx: mpsc::Receiver<Observation>) {
    let mut m = Matcher::new(s.miss_grace, s.extra_grace);
    let mut tick = tokio::time::interval(s.interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let mut observed = Vec::new();
        while let Ok(o) = rx.try_recv() {
            observed.push(o);
        }
        let t = pass(
            &mut src,
            &mut m,
            observed,
            s.batch,
            s.lease.as_millis() as u64,
            Instant::now(),
        )
        .await;
        t.emit();
        metrics::gauge!(PENDING_UNITS).set(m.units_held() as f64);
        metrics::gauge!(PENDING_ROWS).set(m.rows_held() as f64);
    }
}

/// Describes the series and registers the counters at zero, so the first `miss` or `mismatch` is
/// visible to `increase()`.
pub fn register() {
    metrics::describe_counter!(
        RESULTS,
        "geo-db shadow comparison of each terminal Postgres delivery: match, mismatch (payload \
         differs), miss (delivered in Postgres, no unit in geo-db) or extra (unit in geo-db, \
         never delivered in Postgres)"
    );
    metrics::describe_counter!(ERRORS, "geo-db shadow claim or complete calls that failed");
    metrics::describe_counter!(
        DROPPED,
        "Terminal deliveries the shadow did not keep: queue_full, rows_full or bad_key"
    );
    metrics::describe_gauge!(
        PENDING_UNITS,
        "geo-db units claimed and waiting for their Postgres delivery"
    );
    metrics::describe_gauge!(
        PENDING_ROWS,
        "Postgres deliveries waiting for their geo-db unit"
    );
    for v in Verdict::ALL {
        metrics::counter!(RESULTS, "result" => v.as_str()).absolute(0);
    }
    for op in ["claim", "complete"] {
        metrics::counter!(ERRORS, "op" => op).absolute(0);
    }
    for reason in ["queue_full", "rows_full", "bad_key"] {
        metrics::counter!(DROPPED, "reason" => reason).absolute(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn key(n: u8) -> Key {
        [n; KEY_BYTES]
    }

    fn unit(n: u8, json: &str) -> Claimed {
        Claimed {
            key: key(n),
            attempts: 0,
            epoch: u64::from(n),
            payload: json.as_bytes().to_vec(),
        }
    }

    fn obs(n: u8, v: serde_json::Value) -> Observation {
        Observation {
            key: key(n),
            payload: v,
        }
    }

    #[derive(Default)]
    struct Fake {
        claims: VecDeque<Result<Vec<Claimed>, MirrorError>>,
        completed: Vec<(Key, u64)>,
        claim_calls: u32,
    }

    impl Source for Fake {
        async fn claim(&mut self, _: u16, _: u64) -> Result<Vec<Claimed>, MirrorError> {
            self.claim_calls += 1;
            self.claims.pop_front().unwrap_or(Ok(Vec::new()))
        }
        async fn complete(&mut self, u: &Claimed) -> Result<Ack, MirrorError> {
            self.completed.push((u.key, u.epoch));
            Ok(Ack::Ok)
        }
    }

    fn matcher() -> Matcher {
        Matcher::new(Duration::from_secs(10), Duration::from_secs(100))
    }

    #[test]
    fn key_is_ledger_then_webhook() {
        let id = Uuid::from_bytes([9; 16]);
        let k = key_of(&"07".repeat(32), id).unwrap();
        assert_eq!(&k[..32], &[7u8; 32]);
        assert_eq!(&k[32..], &[9u8; 16]);
        assert!(key_of("zz", id).is_none());
        assert!(key_of(&"07".repeat(31), id).is_none());
    }

    #[test]
    fn unit_then_row_matches_and_row_then_unit_matches() {
        let now = Instant::now();
        let mut m = matcher();
        assert!(m.claimed(unit(1, "{\"a\":1,\"b\":2}"), now).is_none());
        let Observed::Resolved(v, u) = m.observed(obs(1, serde_json::json!({"b":2,"a":1})), now)
        else {
            panic!("not resolved")
        };
        assert_eq!((v, u.epoch), (Verdict::Match, 1));
        assert_eq!((m.units_held(), m.rows_held()), (0, 0));

        assert_eq!(
            m.observed(obs(2, serde_json::json!({"a":1})), now),
            Observed::Waiting
        );
        let (v, _) = m.claimed(unit(2, "{\"a\":1}"), now).unwrap();
        assert_eq!(v, Verdict::Match);
    }

    #[test]
    fn a_differing_or_unparsable_payload_is_a_mismatch() {
        let now = Instant::now();
        let mut m = matcher();
        m.claimed(unit(1, "{\"a\":1}"), now);
        assert!(matches!(
            m.observed(obs(1, serde_json::json!({"a":2})), now),
            Observed::Resolved(Verdict::Mismatch, _)
        ));
        m.claimed(unit(2, "not json"), now);
        assert!(matches!(
            m.observed(obs(2, serde_json::json!({})), now),
            Observed::Resolved(Verdict::Mismatch, _)
        ));
    }

    #[test]
    fn sweep_ages_rows_into_misses_and_units_into_extras() {
        let t0 = Instant::now();
        let mut m = matcher();
        m.observed(obs(1, serde_json::json!({})), t0);
        m.claimed(unit(2, "{}"), t0);
        // Nothing is old enough yet.
        assert_eq!(m.sweep(t0 + Duration::from_secs(9)), (0, vec![]));
        // The row passes its miss grace; the unit has not passed its extra grace.
        assert_eq!(m.sweep(t0 + Duration::from_secs(10)), (1, vec![]));
        let (misses, extras) = m.sweep(t0 + Duration::from_secs(100));
        assert_eq!(misses, 0);
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].key, key(2));
        assert_eq!((m.units_held(), m.rows_held()), (0, 0));
    }

    #[test]
    fn a_reclaimed_unit_keeps_its_first_claimed_time_and_takes_the_new_epoch() {
        let t0 = Instant::now();
        let mut m = matcher();
        m.claimed(unit(1, "{}"), t0);
        let mut again = unit(1, "{}");
        again.epoch = 77;
        m.claimed(again, t0 + Duration::from_secs(60));
        let (_, extras) = m.sweep(t0 + Duration::from_secs(100));
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].epoch, 77);
    }

    #[test]
    fn the_wait_list_is_bounded() {
        let now = Instant::now();
        let mut m = matcher();
        for i in 0..HELD_MAX {
            let mut k = [0u8; KEY_BYTES];
            k[..8].copy_from_slice(&(i as u64).to_be_bytes());
            let o = Observation {
                key: k,
                payload: serde_json::json!({}),
            };
            assert_eq!(m.observed(o, now), Observed::Waiting);
        }
        assert_eq!(
            m.observed(obs(255, serde_json::json!({})), now),
            Observed::Full
        );
    }

    #[tokio::test]
    async fn a_pass_completes_matched_missed_and_extra_and_tallies_them() {
        let t0 = Instant::now();
        let mut m = matcher();
        let mut src = Fake::default();
        src.claims
            .push_back(Ok(vec![unit(1, "{\"a\":1}"), unit(2, "{\"a\":1}")]));
        // Row 1 agrees, row 2 differs, row 3 has no unit.
        let rows = vec![
            obs(1, serde_json::json!({"a":1})),
            obs(2, serde_json::json!({"a":9})),
            obs(3, serde_json::json!({})),
        ];
        let t = pass(&mut src, &mut m, rows, 16, 1000, t0).await;
        assert_eq!((t.matches, t.mismatches, t.misses, t.extras), (1, 1, 0, 0));
        assert_eq!(src.completed.len(), 2);

        // Ten seconds on, row 3 is a miss; a unit nobody delivers is an extra after 100 s.
        src.claims.push_back(Ok(vec![unit(4, "{}")]));
        let t = pass(
            &mut src,
            &mut m,
            vec![],
            16,
            1000,
            t0 + Duration::from_secs(10),
        )
        .await;
        assert_eq!((t.matches, t.misses, t.extras), (0, 1, 0));
        let t = pass(
            &mut src,
            &mut m,
            vec![],
            16,
            1000,
            t0 + Duration::from_secs(110),
        )
        .await;
        assert_eq!((t.misses, t.extras), (0, 1));
        assert_eq!(src.completed.last().unwrap().0, key(4));
    }

    #[tokio::test]
    async fn a_failed_claim_is_counted_and_nothing_else_changes() {
        let mut m = matcher();
        let mut src = Fake::default();
        src.claims.push_back(Err(MirrorError::CoolingDown));
        let t = pass(&mut src, &mut m, vec![], 16, 1000, Instant::now()).await;
        assert_eq!(
            t,
            Tally {
                claim_errors: 1,
                ..Tally::default()
            }
        );
        assert!(src.completed.is_empty());
    }

    #[tokio::test]
    async fn a_full_unit_list_stops_claiming() {
        let now = Instant::now();
        let mut m = matcher();
        for i in 0..HELD_MAX {
            let mut k = [0u8; KEY_BYTES];
            k[..8].copy_from_slice(&(i as u64).to_be_bytes());
            m.claimed(
                Claimed {
                    key: k,
                    attempts: 0,
                    epoch: 1,
                    payload: vec![],
                },
                now,
            );
        }
        let mut src = Fake::default();
        pass(&mut src, &mut m, vec![], 16, 1000, now).await;
        assert_eq!(src.claim_calls, 0);
    }

    #[test]
    fn results_register_at_zero_and_emit_by_label() {
        use metrics_exporter_prometheus::PrometheusBuilder;
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            register();
            Tally {
                matches: 3,
                misses: 1,
                ..Tally::default()
            }
            .emit();
        });
        let out = handle.render();
        for line in [
            "delivery_worker_shadow_total{result=\"match\"} 3",
            "delivery_worker_shadow_total{result=\"miss\"} 1",
            "delivery_worker_shadow_total{result=\"mismatch\"} 0",
            "delivery_worker_shadow_total{result=\"extra\"} 0",
            "delivery_worker_shadow_errors_total{op=\"claim\"} 0",
        ] {
            assert!(out.contains(line), "missing `{line}` in:\n{out}");
        }
    }

    #[test]
    fn settings_default_and_override() {
        let d = Settings::from_vars(|_| None);
        assert_eq!(d.extra_grace, Duration::from_secs(3 * 24 * 3600));
        assert_eq!(d.batch, 16);
        let o = Settings::from_vars(|k| (k == "GEODB_SHADOW_BATCH").then(|| "4".to_string()));
        assert_eq!(o.batch, 4);
    }
}
