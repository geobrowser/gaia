//! Replays `notification_outbox` rows into geo-db's work class: the operator tool behind
//! `outbox-replay` (RUNBOOK section 11).
//!
//! The live mirror ([`geodb_outbox::OutboxMirror`]) is fire-and-forget, so a unit it dropped (serve
//! down, queue full) is a gap nothing fills. This module reads the rows Postgres holds for a time
//! window and enters each one through the same [`geodb_outbox::enter_line`] the mirror uses: the
//! ledger key is the row's `idempotency_key`, the event-type byte, the `hex:` payload and the
//! webhook list are encoded by that one function and not re-implemented here.
//!
//! Idempotence is geo-db's. An enter is put-if-absent on the ledger key, so a unit the mirror
//! already entered (or a previous replay did) is answered `duplicate` and writes nothing. Running the
//! same window twice therefore enters nothing the second time. geo-db has no read-only per-key probe
//! (its only reads are `claim`, which leases and so disturbs the delivery-side shadow, and `counts`),
//! so "present" is learned from the enter's own answer and a dry run, which never touches geo-db,
//! cannot report it.
//!
//! The source and the sink are traits so the loop is tested against fixtures with no Postgres and no
//! geo-db.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use geodb_outbox::{enter_line, Entered, Enterer, MirrorError, Unit};
use uuid::Uuid;

/// One outbox row as read from Postgres.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    /// `idempotency_key`: 64 lowercase hex digits.
    pub idempotency_key: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    /// `webhook_id` of the row's delivery rows.
    pub webhooks: Vec<Uuid>,
}

/// Where a page resumes: strictly after `(created_at, id)`.
pub type Cursor = (DateTime<Utc>, Uuid);

/// The unit the live mirror would have submitted for `row`. The payload is the JSON value's compact
/// text, as in `Storage::insert_outbox`; webhooks are the delivery rows' ids, sorted so a replay is
/// deterministic.
pub fn to_unit(row: &Row) -> Unit {
    let hooks: BTreeSet<[u8; 16]> = row.webhooks.iter().map(|w| *w.as_bytes()).collect();
    Unit {
        ledger_key_hex: row.idempotency_key.clone(),
        event_type: row.event_type.clone(),
        payload: row.payload.to_string(),
        webhooks: hooks.into_iter().collect(),
    }
}

/// Rows of the window, oldest first.
pub trait RowSource {
    /// Up to `limit` rows after `after`, in `(created_at, id)` order. Empty means done.
    fn page(
        &mut self,
        after: Option<Cursor>,
        limit: usize,
    ) -> impl std::future::Future<Output = Result<Vec<Row>, String>>;
}

/// A pool of one connection whose sessions are read-only (`default_transaction_read_only = on`), so
/// the tool cannot write to Postgres even by mistake.
pub async fn read_only_pool(url: &str) -> Result<sqlx::PgPool, sqlx::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only = on")
                    .execute(c)
                    .await
                    .map(|_| ())
            })
        })
        .connect(url)
        .await
}

/// The Postgres row source: outbox rows with `from <= created_at < to` (`to` exclusive), each with
/// the `webhook_id`s of its delivery rows.
pub struct PgSource {
    pub pool: sqlx::PgPool,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl RowSource for PgSource {
    async fn page(&mut self, after: Option<Cursor>, limit: usize) -> Result<Vec<Row>, String> {
        use sqlx::Row as _;
        let (at, id) = after.unzip();
        let rows = sqlx::query(
            r#"
            SELECT o.created_at, o.id, o.idempotency_key, o.event_type, o.payload,
                   ARRAY(SELECT d.webhook_id FROM notification_deliveries d
                         WHERE d.outbox_id = o.id ORDER BY d.webhook_id) AS webhooks
            FROM notification_outbox o
            WHERE o.created_at >= $1 AND o.created_at < $2
              AND ($3::timestamptz IS NULL OR (o.created_at, o.id) > ($3, $4))
            ORDER BY o.created_at, o.id
            LIMIT $5
            "#,
        )
        .bind(self.from)
        .bind(self.to)
        .bind(at)
        .bind(id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter()
            .map(|r| {
                Ok(Row {
                    created_at: r.try_get("created_at")?,
                    id: r.try_get("id")?,
                    idempotency_key: r.try_get("idempotency_key")?,
                    event_type: r.try_get("event_type")?,
                    payload: r.try_get("payload")?,
                    webhooks: r.try_get::<Vec<Uuid>, _>("webhooks")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(|e| e.to_string())
    }
}

/// Where units go.
pub trait Sink {
    fn enter(
        &mut self,
        unit: &Unit,
    ) -> impl std::future::Future<Output = Result<Entered, MirrorError>>;
}

impl Sink for Enterer {
    async fn enter(&mut self, unit: &Unit) -> Result<Entered, MirrorError> {
        Enterer::enter(self, unit).await
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Rows read from Postgres per query.
    pub batch: usize,
    /// Pause after each enter that reached geo-db (the rate bound).
    pub interval: Duration,
    /// Stop after this many rows scanned.
    pub max_units: Option<u64>,
    /// False (the default) classifies rows and writes nothing.
    pub apply: bool,
}

pub const BATCH_DEFAULT: usize = 500;
pub const BATCH_MAX: usize = 5000;
pub const RATE_DEFAULT: u32 = 50;
pub const RATE_MAX: u32 = 1000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Rows read from Postgres.
    pub scanned: u64,
    /// Rows the mirror's encoding accepts: what an apply would send.
    pub would_enter: u64,
    /// Rows the encoding refuses (unknown event type, payload over the bound, too many webhooks, bad
    /// key). The live mirror skips these too.
    pub unencodable: u64,
    /// Apply only: enters answered `entered`.
    pub entered: u64,
    /// Apply only: enters answered `duplicate`, the units already present.
    pub present: u64,
    /// The last row handled, to resume a stopped run from.
    pub last: Option<Cursor>,
}

#[derive(Debug)]
pub struct Stopped {
    pub report: Report,
    pub reason: String,
}

/// Walks the window. In a dry run nothing is sent; in an apply a geo-db error stops the run (the
/// report so far comes back with it) because a held pipe that is failing will fail every row. A rerun
/// is safe: the rows already entered answer `duplicate`.
pub async fn run<S: RowSource, K: Sink>(
    source: &mut S,
    sink: &mut K,
    opts: &Options,
) -> Result<Report, Stopped> {
    let mut rep = Report::default();
    let mut after: Option<Cursor> = None;
    loop {
        let want = match opts.max_units {
            Some(m) if rep.scanned >= m => break,
            Some(m) => opts.batch.min((m - rep.scanned) as usize),
            None => opts.batch,
        };
        let rows = source.page(after, want).await.map_err(|e| Stopped {
            report: rep.clone(),
            reason: format!("postgres: {e}"),
        })?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            rep.scanned += 1;
            after = Some((row.created_at, row.id));
            rep.last = after;
            let unit = to_unit(row);
            if enter_line(&unit).is_err() {
                rep.unencodable += 1;
                continue;
            }
            rep.would_enter += 1;
            if !opts.apply {
                continue;
            }
            match sink.enter(&unit).await {
                Ok(Entered::New) => rep.entered += 1,
                Ok(Entered::Duplicate) => rep.present += 1,
                Err(e) => {
                    return Err(Stopped {
                        report: rep,
                        reason: format!("geo-db enter of {}: {e}", unit.ledger_key_hex),
                    })
                }
            }
            if !opts.interval.is_zero() {
                tokio::time::sleep(opts.interval).await;
            }
        }
    }
    Ok(rep)
}

/// The enter interval for `rate` units a second, clamped to `1..=RATE_MAX`.
pub fn interval_for(rate: u32) -> Duration {
    Duration::from_micros(1_000_000 / u64::from(rate.clamp(1, RATE_MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;

    fn row(n: u32, event_type: &str) -> Row {
        let raw = format!("blk:{n}:{event_type}:user");
        Row {
            created_at: Utc.timestamp_opt(1_000 + i64::from(n), 0).unwrap(),
            id: Uuid::from_u128(u128::from(n)),
            idempotency_key: hex::encode(Sha256::digest(raw.as_bytes())),
            event_type: event_type.to_string(),
            payload: serde_json::json!({"n": n, "idempotency_key": raw}),
            webhooks: vec![Uuid::from_u128(900 + u128::from(n))],
        }
    }

    struct Rows(Vec<Row>);
    impl RowSource for Rows {
        async fn page(&mut self, after: Option<Cursor>, limit: usize) -> Result<Vec<Row>, String> {
            Ok(self
                .0
                .iter()
                .filter(|r| after.is_none_or(|a| (r.created_at, r.id) > a))
                .take(limit)
                .cloned()
                .collect())
        }
    }

    /// A put-if-absent store: what geo-db's enter does with the ledger key.
    #[derive(Default)]
    struct Store {
        held: HashSet<String>,
        writes: u64,
        calls: u64,
        fail_after: Option<u64>,
    }
    impl Sink for Store {
        async fn enter(&mut self, unit: &Unit) -> Result<Entered, MirrorError> {
            self.calls += 1;
            if self.fail_after.is_some_and(|n| self.calls > n) {
                return Err(MirrorError::Closed);
            }
            if self.held.insert(unit.ledger_key_hex.clone()) {
                self.writes += 1;
                Ok(Entered::New)
            } else {
                Ok(Entered::Duplicate)
            }
        }
    }

    fn opts(apply: bool) -> Options {
        Options {
            batch: 2,
            interval: Duration::ZERO,
            max_units: None,
            apply,
        }
    }

    fn rows(n: u32) -> Vec<Row> {
        (1..=n).map(|i| row(i, "proposal_created")).collect()
    }

    #[tokio::test]
    async fn dry_run_counts_and_never_touches_the_sink() {
        let mut store = Store::default();
        let rep = run(&mut Rows(rows(5)), &mut store, &opts(false))
            .await
            .unwrap();
        assert_eq!((rep.scanned, rep.would_enter), (5, 5));
        assert_eq!((rep.entered, rep.present), (0, 0));
        assert_eq!(store.calls, 0);
    }

    #[tokio::test]
    async fn apply_enters_the_missing_and_skips_the_present() {
        let all = rows(5);
        let mut store = Store::default();
        for r in &all[..2] {
            store.held.insert(r.idempotency_key.clone());
        }
        let rep = run(&mut Rows(all), &mut store, &opts(true)).await.unwrap();
        assert_eq!((rep.scanned, rep.entered, rep.present), (5, 3, 2));
        assert_eq!(store.writes, 3);
        assert_eq!(store.held.len(), 5);
    }

    #[tokio::test]
    async fn a_second_run_over_the_same_window_writes_nothing() {
        let all = rows(7);
        let mut store = Store::default();
        let first = run(&mut Rows(all.clone()), &mut store, &opts(true))
            .await
            .unwrap();
        assert_eq!((first.entered, first.present), (7, 0));
        let second = run(&mut Rows(all), &mut store, &opts(true)).await.unwrap();
        assert_eq!((second.entered, second.present), (0, 7));
        assert_eq!((store.writes, store.held.len()), (7, 7));
    }

    #[tokio::test]
    async fn rows_the_encoding_refuses_are_counted_not_sent() {
        let mut all = rows(2);
        all.push(row(3, "not_a_type"));
        let mut big = row(4, "comment");
        big.payload = serde_json::json!({"p": "x".repeat(geodb_outbox::PAYLOAD_MAX)});
        all.push(big);
        let mut store = Store::default();
        let rep = run(&mut Rows(all), &mut store, &opts(true)).await.unwrap();
        assert_eq!((rep.scanned, rep.unencodable, rep.would_enter), (4, 2, 2));
        assert_eq!(store.calls, 2);
    }

    #[tokio::test]
    async fn max_units_bounds_the_scan() {
        let mut store = Store::default();
        let mut o = opts(true);
        o.max_units = Some(3);
        let rep = run(&mut Rows(rows(10)), &mut store, &o).await.unwrap();
        assert_eq!((rep.scanned, rep.entered), (3, 3));
        assert_eq!(rep.last.unwrap().1, Uuid::from_u128(3));
    }

    #[tokio::test]
    async fn a_sink_failure_stops_the_run_and_a_rerun_finishes_it() {
        let all = rows(6);
        let mut store = Store {
            fail_after: Some(2),
            ..Store::default()
        };
        let stopped = run(&mut Rows(all.clone()), &mut store, &opts(true))
            .await
            .unwrap_err();
        assert_eq!(stopped.report.entered, 2);
        store.fail_after = None;
        let rep = run(&mut Rows(all), &mut store, &opts(true)).await.unwrap();
        assert_eq!((rep.entered, rep.present), (4, 2));
        assert_eq!(store.held.len(), 6);
    }

    #[test]
    fn the_unit_encodes_exactly_as_the_live_mirror_does() {
        let r = row(1, "proposal_created");
        let mut r2 = r.clone();
        r2.webhooks = vec![Uuid::from_u128(5), Uuid::from_u128(3), Uuid::from_u128(5)];
        let u = to_unit(&r2);
        // sorted, deduplicated
        assert_eq!(
            u.webhooks,
            vec![
                *Uuid::from_u128(3).as_bytes(),
                *Uuid::from_u128(5).as_bytes()
            ]
        );
        let line = enter_line(&to_unit(&r)).unwrap();
        let payload: String = r
            .payload
            .to_string()
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        let hook = hex::encode(Uuid::from_u128(901).as_bytes());
        assert_eq!(
            line,
            format!("enter\t{}\t0\thex:{payload}\t{hook}\n", r.idempotency_key)
        );
    }

    #[test]
    fn rate_maps_to_an_interval() {
        assert_eq!(interval_for(50), Duration::from_millis(20));
        assert_eq!(interval_for(0), Duration::from_secs(1));
        assert_eq!(interval_for(1_000_000), Duration::from_millis(1));
    }
}
