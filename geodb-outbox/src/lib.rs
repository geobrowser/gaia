//! Best-effort shadow of `notification_outbox` rows into geo-db's work class (plan 10.16).
//!
//! Postgres stays the source of truth and every reader stays on it. After the indexer's Postgres
//! transaction commits, the caller hands each *newly inserted* outbox row to
//! [`OutboxMirror::submit`], which only does a `try_send` into a bounded queue: it never waits,
//! never fails and never allocates past the bound. A background task owns one held
//! `geodb-client work pipe` child and sends one `enter` line a unit (geo-db
//! `docs/api/10-embedded-and-wire.md`, the `work pipe` row). An enter is put-if-absent on the
//! ledger key, so a Kafka replay that re-submits a row is answered `duplicate`, not written twice.
//!
//! Every failure is logged and counted, never raised: a full queue drops the unit
//! (`geodb_outbox_mirror_dropped_total`), a payload over [`PAYLOAD_MAX`] is skipped
//! (`geodb_outbox_mirror_skipped_total`), a pipe error drops the child and nothing respawns for
//! the cool-down (`geodb_outbox_mirror_errors_total`). The exception is a pipe that was already
//! open when the unit arrived: geo-db closes idle pipes, so that unit is replayed once over a
//! fresh pipe (`geodb_outbox_mirror_reconnects_total`) rather than lost. The mirror is a measurement, so a dropped
//! unit is a gap a later comparison finds, not a fault.
//!
//! The ledger key is the outbox row's `idempotency_key` decoded from hex (a SHA-256, 32 bytes).
//! The event type is [`event_type_code`] of the row's `event_type`. The payload is the row's JSON
//! text, sent as `hex:` so the tab-separated line cannot be broken by its content. The webhooks
//! are the delivery rows' `webhook_id` UUIDs (16 bytes each).
//!
//! The mirror is OFF unless `GEODB_OUTBOX_MIRROR` is `1` or `true`.
//!
//! Backlog (second flag, off by default): with `GEODB_OUTBOX_MIRROR_BACKLOG` on, a unit whose
//! enter fails is kept instead of dropped, in a bounded in-memory backlog
//! (`GEODB_OUTBOX_MIRROR_BACKLOG_MAX`, default [`BACKLOG_MAX_DEFAULT`]). Units are retried oldest
//! first, in submit order, every [`RETRY_INTERVAL`] and whenever a new unit arrives, so a serve
//! roll's 20-40 s gap loses nothing. A retry is safe because an enter is put-if-absent on the
//! ledger key: a unit whose first attempt actually landed (a timed-out answer) is answered
//! `duplicate`. When the bound is hit the oldest unit is dropped
//! (`geodb_outbox_mirror_backlog_dropped_total`). A unit that can never be sent (bad key, bad
//! type, payload too long) is skipped, never kept.
//!
//! Head expiry: the unit at the head blocks the ones behind it, so a unit geo-db keeps refusing
//! (a per-unit `Refused` answer, not a down pipe) must not sit there until overflow evicts it.
//! The head is dropped, counted in `geodb_outbox_mirror_backlog_expired_total`, after
//! [`EXPIRE_ATTEMPTS_DEFAULT`] refused attempts or [`EXPIRE_SECS_DEFAULT`] seconds since its first
//! refusal (`GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_ATTEMPTS`, `GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_SECS`).
//! Transport failures (closed pipe, timeout, cooling down) never expire a unit, so an outage still
//! keeps everything up to the bound. The units behind the expired one keep their order.
//!
//! The delivery side reads the same work class through [`reader::OutboxReader`], over the same
//! pipe client, behind its own flag `GEODB_OUTBOX_SHADOW_READ` (also off by default).

pub mod reader;

use std::collections::VecDeque;
use std::ffi::OsString;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// The longest payload a work unit carries (geo-db `work_payload_max`'s default, 16 KiB).
pub const PAYLOAD_MAX: usize = 16 * 1024;

/// The most webhooks a unit names (geo-db `work.zig` bounds a unit's `hook_count` to a byte; the
/// frame bounds it further, so the mirror keeps well inside).
pub const HOOKS_MAX: usize = 32;

/// Units waiting for the pipe. A burst past this drops units, counted.
pub const QUEUE_MAX: usize = 4096;

/// Failed units kept when the backlog flag is on: about 70 s at the highest per-minute insert
/// rate seen across 7 days of production (42 units/s, 2026-11-11), with a unit's usual size a
/// few hundred bytes (worst case 16 KiB payload, 3000 of them is 48 MiB).
pub const BACKLOG_MAX_DEFAULT: usize = 3000;

/// How often a non-empty backlog is retried when no new unit arrives.
pub const RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Refused attempts after which the head is dropped (about 30 s of 1 s retries).
pub const EXPIRE_ATTEMPTS_DEFAULT: u32 = 30;

/// Seconds after a unit's first refusal after which it is dropped, even with attempts left (a
/// busy stream retries the head on every arrival, so attempts alone can be quick).
pub const EXPIRE_SECS_DEFAULT: u64 = 120;

/// When the head of the backlog is dropped for being refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expiry {
    pub attempts: u32,
    pub age: Duration,
}

impl Default for Expiry {
    fn default() -> Self {
        Expiry {
            attempts: EXPIRE_ATTEMPTS_DEFAULT,
            age: Duration::from_secs(EXPIRE_SECS_DEFAULT),
        }
    }
}

/// The expiry from the environment: each knob a positive integer, else its default.
pub fn expiry_from_vars(get: impl Fn(&str) -> Option<String>) -> Expiry {
    let num = |k: &str| {
        get(k)
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|n| *n > 0)
    };
    let d = Expiry::default();
    Expiry {
        attempts: num("GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_ATTEMPTS")
            .map_or(d.attempts, |n| n.min(u64::from(u32::MAX)) as u32),
        age: num("GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_SECS").map_or(d.age, Duration::from_secs),
    }
}

/// The work class's event-type byte for an outbox `event_type`, stable by this table (geo-db's
/// `work_expire` keeps by a bitmask over types below 32). `None` for a type the table lacks.
pub fn event_type_code(event_type: &str) -> Option<u8> {
    Some(match event_type {
        "proposal_created" => 0,
        "proposal_updated" => 1,
        "proposal_voted" => 2,
        "proposal_executed" => 3,
        "proposal_settings_updated" => 4,
        "proposal_rejected" => 5,
        "bounty_interest" => 6,
        "bounty_allocated" => 7,
        "bounty_payout" => 8,
        "bounty_created" => 9,
        "proposal_comment" => 10,
        "comment" => 11,
        "entity_votes_threshold" => 12,
        _ => return None,
    })
}

/// One outbox row to shadow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// The row's `idempotency_key`: 64 lowercase hex digits (a SHA-256).
    pub ledger_key_hex: String,
    pub event_type: String,
    /// The row's payload as JSON text.
    pub payload: String,
    /// `webhook_id`s of the row's delivery rows, 16 bytes each.
    pub webhooks: Vec<[u8; 16]>,
}

#[derive(Debug, Error)]
pub enum MirrorError {
    #[error("spawn failed: {0}")]
    Spawn(std::io::Error),
    #[error("child did not print `ready`, got {0:?}")]
    NotReady(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("child closed its output")]
    Closed,
    #[error("bad frame header {0:?}")]
    BadFrame(String),
    #[error("refused: {0}")]
    Refused(String),
    #[error("payload of {0} bytes is over the {PAYLOAD_MAX} byte bound")]
    TooLong(usize),
    #[error("unit not representable: {0}")]
    BadUnit(String),
    #[error("cooling down after a failure")]
    CoolingDown,
}

/// The most an enter's answer (`entered` or `duplicate`) can be.
const ENTER_ANSWER_MAX: usize = 4096;

/// What an enter was answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Entered {
    New,
    Duplicate,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Bound on one request, and on the spawn handshake.
    pub timeout: Duration,
    /// After a failure, no respawn for this long.
    pub cooldown: Duration,
}

impl Config {
    /// Reads the environment. `None` when the mirror is off (the default) or misconfigured.
    /// `GEODB_OUTBOX_MIRROR`, `GEODB_CLIENT_BIN` (default `geodb-client`), `GEODB_OUTBOX_KEY_PATH`
    /// and `GEODB_BINDING_PATH` (both required).
    pub fn from_env() -> Option<Config> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Config> {
        Self::from_vars_flag(get, "GEODB_OUTBOX_MIRROR")
    }

    /// As [`Config::from_env`], for the feature switched by `flag` (the mirror's is
    /// `GEODB_OUTBOX_MIRROR`, the reader's `GEODB_OUTBOX_SHADOW_READ`). The client, key and
    /// binding are shared.
    pub fn from_env_flag(flag: &str) -> Option<Config> {
        Self::from_vars_flag(|k| std::env::var(k).ok(), flag)
    }

    pub fn from_vars_flag(get: impl Fn(&str) -> Option<String>, flag: &str) -> Option<Config> {
        let on = matches!(
            get(flag).as_deref().map(str::trim),
            Some("1") | Some("true")
        );
        if !on {
            return None;
        }
        let (Some(key), Some(binding)) = (get("GEODB_OUTBOX_KEY_PATH"), get("GEODB_BINDING_PATH"))
        else {
            warn!(
                event = "geodb_outbox.misconfigured",
                flag,
                "flag is on but GEODB_OUTBOX_KEY_PATH or GEODB_BINDING_PATH is unset; disabled"
            );
            return None;
        };
        Some(Config {
            program: get("GEODB_CLIENT_BIN")
                .unwrap_or_else(|| "geodb-client".into())
                .into(),
            args: ["--key", &key, "--binding", &binding, "work", "pipe"]
                .iter()
                .map(OsString::from)
                .collect(),
            timeout: Duration::from_secs(2),
            cooldown: Duration::from_secs(10),
        })
    }
}

/// The line `work pipe` reads for `unit`, or why it cannot be sent.
pub fn enter_line(unit: &Unit) -> Result<String, MirrorError> {
    let key = &unit.ledger_key_hex;
    if key.len() != 64
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(MirrorError::BadUnit(format!("ledger key {key:?}")));
    }
    let code = event_type_code(&unit.event_type)
        .ok_or_else(|| MirrorError::BadUnit(format!("event type {:?}", unit.event_type)))?;
    if unit.payload.len() > PAYLOAD_MAX {
        return Err(MirrorError::TooLong(unit.payload.len()));
    }
    if unit.webhooks.len() > HOOKS_MAX {
        return Err(MirrorError::BadUnit(format!(
            "{} webhooks",
            unit.webhooks.len()
        )));
    }
    let payload: String = unit.payload.bytes().map(|b| format!("{b:02x}")).collect();
    let hooks: Vec<String> = unit
        .webhooks
        .iter()
        .map(|h| h.iter().map(|b| format!("{b:02x}")).collect())
        .collect();
    Ok(format!(
        "enter\t{key}\t{code}\thex:{payload}\t{}\n",
        hooks.join(",")
    ))
}

pub(crate) struct Pipe {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for Pipe {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Owns the `work pipe` child (started on first use). Used from one task.
pub(crate) struct Worker {
    pub(crate) cfg: Config,
    pipe: Option<Pipe>,
    failed_at: Option<Instant>,
}

impl Worker {
    pub(crate) fn new(cfg: Config) -> Self {
        Worker {
            cfg,
            pipe: None,
            failed_at: None,
        }
    }

    async fn enter(&mut self, unit: &Unit) -> Result<Entered, MirrorError> {
        let line = enter_line(unit)?;
        // An enter is put-if-absent on the ledger key, so replaying it on a fresh pipe is safe.
        let body = self.call_replaying(&line, ENTER_ANSWER_MAX).await?;
        match body.trim_end() {
            "entered" => Ok(Entered::New),
            "duplicate" => Ok(Entered::Duplicate),
            other => Err(MirrorError::BadFrame(other.to_string())),
        }
    }

    /// As [`Worker::call`], for a request that is safe to send twice. geo-db closes a pipe that
    /// sat idle past its idle bound (60 s) and ends it at grant expiry (`grant_ttl_s`), and the
    /// held client does not ping while it waits on stdin, so a pipe that has been open a while
    /// is often already dead when the next unit arrives. When a pipe opened by an earlier call
    /// fails, the request is sent once more over a fresh pipe, ignoring the cool-down (the
    /// failure was the old pipe's, not the server's). A pipe opened by this call that fails
    /// is a real fault and is not retried.
    pub(crate) async fn call_replaying(
        &mut self,
        line: &str,
        max_body: usize,
    ) -> Result<String, MirrorError> {
        let reused = self.pipe.is_some();
        match self.call(line, max_body).await {
            Err(e) if reused && !matches!(e, MirrorError::CoolingDown) => {
                metrics::counter!("geodb_outbox_mirror_reconnects_total").increment(1);
                info!(
                    event = "geodb_outbox.pipe_lost",
                    error = %e,
                    "held work pipe was dead; reopening and replaying the unit"
                );
                self.failed_at = None;
                self.call(line, max_body).await
            }
            other => other,
        }
    }

    /// One request line (ending in `\n`) and its `+` frame's body, at most `max_body` bytes.
    /// Starts the child on first use; a failure drops it and cools down.
    pub(crate) async fn call(
        &mut self,
        line: &str,
        max_body: usize,
    ) -> Result<String, MirrorError> {
        if self.pipe.is_none() {
            if self
                .failed_at
                .is_some_and(|t| t.elapsed() < self.cfg.cooldown)
            {
                return Err(MirrorError::CoolingDown);
            }
            match tokio::time::timeout(self.cfg.timeout, spawn(&self.cfg)).await {
                Ok(Ok(p)) => {
                    info!(
                        event = "geodb_outbox.pipe_started",
                        "geodb-client work pipe ready"
                    );
                    self.pipe = Some(p);
                }
                Ok(Err(e)) => {
                    self.failed_at = Some(Instant::now());
                    return Err(e);
                }
                Err(_) => {
                    self.failed_at = Some(Instant::now());
                    return Err(MirrorError::Timeout(self.cfg.timeout));
                }
            }
        }
        let pipe = self.pipe.as_mut().expect("set above");
        let res = match tokio::time::timeout(self.cfg.timeout, request(pipe, line, max_body)).await
        {
            Ok(r) => r,
            Err(_) => Err(MirrorError::Timeout(self.cfg.timeout)),
        };
        match res {
            Ok(body) => {
                self.failed_at = None;
                Ok(body)
            }
            Err(e) => {
                // A refusal or error leaves the tool exiting; drop it (kill on drop) and respawn
                // after the cool-down.
                self.pipe = None;
                self.failed_at = Some(Instant::now());
                Err(e)
            }
        }
    }
}

async fn spawn(cfg: &Config) -> Result<Pipe, MirrorError> {
    let mut child = Command::new(&cfg.program)
        .args(&cfg.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(MirrorError::Spawn)?;
    let stdin = child.stdin.take().ok_or(MirrorError::Closed)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or(MirrorError::Closed)?);
    let mut first = String::new();
    if stdout.read_line(&mut first).await? == 0 {
        return Err(MirrorError::Closed);
    }
    if first.trim_end() != "ready" {
        return Err(MirrorError::NotReady(first));
    }
    Ok(Pipe {
        child,
        stdin,
        stdout,
    })
}

/// One request line, one frame: `+<n>\n<n bytes>` ok (the body returned), `-<n>\n<n bytes>` refused.
async fn request(p: &mut Pipe, line: &str, max_body: usize) -> Result<String, MirrorError> {
    p.stdin.write_all(line.as_bytes()).await?;
    p.stdin.flush().await?;
    let mut head = String::new();
    if p.stdout.read_line(&mut head).await? == 0 {
        return Err(MirrorError::Closed);
    }
    let head = head.trim_end();
    let (sign, n) = head.split_at(head.len().min(1));
    let n: usize = n
        .parse()
        .map_err(|_| MirrorError::BadFrame(head.to_string()))?;
    // Cap what a broken child can make us allocate.
    if n > max_body {
        return Err(MirrorError::BadFrame(head.to_string()));
    }
    let mut body = vec![0u8; n];
    p.stdout.read_exact(&mut body).await?;
    let body = String::from_utf8_lossy(&body).into_owned();
    match sign {
        "+" => Ok(body),
        "-" => Err(MirrorError::Refused(body)),
        _ => Err(MirrorError::BadFrame(head.to_string())),
    }
}

/// The backlog bound from the environment: `None` (off, the default) unless
/// `GEODB_OUTBOX_MIRROR_BACKLOG` is `1` or `true`; the bound is `GEODB_OUTBOX_MIRROR_BACKLOG_MAX`
/// (a positive integer, else [`BACKLOG_MAX_DEFAULT`]).
pub fn backlog_from_vars(get: impl Fn(&str) -> Option<String>) -> Option<usize> {
    let on = matches!(
        get("GEODB_OUTBOX_MIRROR_BACKLOG").as_deref().map(str::trim),
        Some("1") | Some("true")
    );
    if !on {
        return None;
    }
    Some(
        get("GEODB_OUTBOX_MIRROR_BACKLOG_MAX")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(BACKLOG_MAX_DEFAULT),
    )
}

/// What the mirror's task enters units through; the pipe [`Worker`] in production, a fake in
/// tests.
pub(crate) trait Enter {
    fn enter(
        &mut self,
        unit: &Unit,
    ) -> impl std::future::Future<Output = Result<Entered, MirrorError>> + Send;
}

impl Enter for Worker {
    fn enter(
        &mut self,
        unit: &Unit,
    ) -> impl std::future::Future<Output = Result<Entered, MirrorError>> + Send {
        Worker::enter(self, unit)
    }
}

/// The handle the indexer holds. Cheap to clone; `submit` never blocks.
#[derive(Clone)]
pub struct OutboxMirror {
    tx: mpsc::Sender<Unit>,
}

impl OutboxMirror {
    /// Starts the background task on the current tokio runtime. Panics outside one.
    pub fn new(cfg: Config) -> Self {
        Self::with_backlog(cfg, None, Expiry::default())
    }

    /// As [`OutboxMirror::new`], keeping up to `backlog` failed units for retry (`None` or
    /// `Some(0)`: a failed unit is dropped, as before).
    pub fn with_backlog(cfg: Config, backlog: Option<usize>, expiry: Expiry) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_MAX);
        tokio::spawn(run(
            Worker::new(cfg),
            rx,
            backlog.unwrap_or(0),
            expiry,
            RETRY_INTERVAL,
        ));
        Self { tx }
    }

    /// `Some` only when `GEODB_OUTBOX_MIRROR` is on and configured. The backlog follows
    /// `GEODB_OUTBOX_MIRROR_BACKLOG` (off by default).
    pub fn from_env() -> Option<Self> {
        let backlog = backlog_from_vars(|k| std::env::var(k).ok());
        let expiry = expiry_from_vars(|k| std::env::var(k).ok());
        Config::from_env().map(|cfg| Self::with_backlog(cfg, backlog, expiry))
    }

    /// Queues `unit` for the pipe. Never waits and never fails the caller: a full queue drops the
    /// unit and counts it.
    pub fn submit(&self, unit: Unit) {
        if let Err(e) = self.tx.try_send(unit) {
            metrics::counter!("geodb_outbox_mirror_dropped_total").increment(1);
            if matches!(e, mpsc::error::TrySendError::Closed(_)) {
                warn!(
                    event = "geodb_outbox.worker_gone",
                    "mirror task is gone; unit dropped"
                );
            }
        }
    }
}

/// A unit's one enter, with the outcome counted. `Err` is a failure worth keeping the unit for.
async fn attempt<E: Enter>(w: &mut E, unit: &Unit) -> Result<(), MirrorError> {
    match w.enter(unit).await {
        Ok(Entered::New) => {
            metrics::counter!("geodb_outbox_mirror_entered_total").increment(1);
            Ok(())
        }
        Ok(Entered::Duplicate) => {
            metrics::counter!("geodb_outbox_mirror_duplicates_total").increment(1);
            Ok(())
        }
        // Never sendable: skip, whatever the backlog says.
        Err(MirrorError::TooLong(n)) => {
            metrics::counter!("geodb_outbox_mirror_skipped_total").increment(1);
            warn!(
                event = "geodb_outbox.payload_too_long",
                ledger_key = %unit.ledger_key_hex,
                payload_bytes = n,
                limit = PAYLOAD_MAX,
                "payload over the geo-db bound; skipped, not truncated"
            );
            Ok(())
        }
        Err(MirrorError::BadUnit(why)) => {
            metrics::counter!("geodb_outbox_mirror_skipped_total").increment(1);
            warn!(
                event = "geodb_outbox.bad_unit",
                ledger_key = %unit.ledger_key_hex,
                reason = %why,
                "unit cannot be represented in geo-db; skipped"
            );
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn count_failure(unit: &Unit, e: &MirrorError) {
    metrics::counter!("geodb_outbox_mirror_errors_total").increment(1);
    if !matches!(e, MirrorError::CoolingDown) {
        warn!(
            event = "geodb_outbox.mirror_failed",
            ledger_key = %unit.ledger_key_hex,
            error = %e,
            "geo-db outbox mirror failed; Postgres is unaffected"
        );
    }
}

/// Failed units, oldest first. `failed` is true once a unit's first attempt has failed.
struct Backlog {
    max: usize,
    units: VecDeque<(Unit, bool)>,
    expiry: Expiry,
    /// Refused attempts of the current head, and when the first one was. Only the head is ever
    /// attempted, so this is the whole per-unit state; it resets whenever the head changes.
    head_refused: u32,
    head_since: Option<Instant>,
}

impl Backlog {
    fn new(max: usize, expiry: Expiry) -> Self {
        Backlog {
            max,
            units: VecDeque::new(),
            expiry,
            head_refused: 0,
            head_since: None,
        }
    }

    fn pop_head(&mut self) -> Option<(Unit, bool)> {
        self.head_refused = 0;
        self.head_since = None;
        self.units.pop_front()
    }

    fn gauge(&self) {
        metrics::gauge!("geodb_outbox_mirror_backlog").set(self.units.len() as f64);
    }

    /// Keeps `unit` behind the older ones; at the bound the oldest is dropped, counted.
    fn push(&mut self, unit: Unit, failed: bool) {
        if self.units.len() >= self.max
            && let Some((old, _)) = self.pop_head()
        {
            metrics::counter!("geodb_outbox_mirror_backlog_dropped_total").increment(1);
            warn!(
                event = "geodb_outbox.backlog_overflow",
                ledger_key = %old.ledger_key_hex,
                bound = self.max,
                "backlog full; oldest unit dropped"
            );
        }
        self.units.push_back((unit, failed));
    }

    /// Enters from the oldest until one fails. A failure stops the pass (the rest would fail the
    /// same way and order must hold); the unit stays at the front.
    async fn drain<E: Enter>(&mut self, w: &mut E) {
        while let Some((unit, failed)) = self.units.front_mut() {
            match attempt(w, unit).await {
                Ok(()) => {
                    if *failed {
                        metrics::counter!("geodb_outbox_mirror_retried_total").increment(1);
                    }
                    self.pop_head();
                }
                Err(e) => {
                    // Counted once a unit, on its first failure, so a long outage is not
                    // one error a retry tick.
                    if !*failed {
                        *failed = true;
                        count_failure(unit, &e);
                    }
                    if matches!(e, MirrorError::Refused(_)) {
                        self.head_refused += 1;
                        let since = *self.head_since.get_or_insert_with(Instant::now);
                        if self.head_refused >= self.expiry.attempts
                            || since.elapsed() >= self.expiry.age
                        {
                            let attempts = self.head_refused;
                            if let Some((old, _)) = self.pop_head() {
                                metrics::counter!("geodb_outbox_mirror_backlog_expired_total")
                                    .increment(1);
                                warn!(
                                    event = "geodb_outbox.backlog_expired",
                                    ledger_key = %old.ledger_key_hex,
                                    attempts,
                                    error = %e,
                                    "unit refused until its expiry; dropped from the backlog"
                                );
                            }
                            // The next unit is a fresh head: try it in this pass.
                            continue;
                        }
                    }
                    break;
                }
            }
        }
        self.gauge();
    }
}

async fn run<E: Enter>(
    mut w: E,
    mut rx: mpsc::Receiver<Unit>,
    backlog_max: usize,
    expiry: Expiry,
    retry: Duration,
) {
    if backlog_max == 0 {
        while let Some(unit) = rx.recv().await {
            metrics::gauge!("geodb_outbox_mirror_queue").set(rx.len() as f64);
            if let Err(e) = attempt(&mut w, &unit).await {
                count_failure(&unit, &e);
            }
        }
        return;
    }
    let mut backlog = Backlog::new(backlog_max, expiry);
    loop {
        let next = if backlog.units.is_empty() {
            match rx.recv().await {
                Some(u) => Some(u),
                None => return,
            }
        } else {
            match tokio::time::timeout(retry, rx.recv()).await {
                Ok(Some(u)) => Some(u),
                Ok(None) => return,
                Err(_) => None,
            }
        };
        metrics::gauge!("geodb_outbox_mirror_queue").set(rx.len() as f64);
        if let Some(unit) = next {
            backlog.push(unit, false);
        }
        backlog.drain(&mut w).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0707070707070707070707070707070707070707070707070707070707070707";

    fn unit() -> Unit {
        Unit {
            ledger_key_hex: KEY.into(),
            event_type: "proposal_rejected".into(),
            payload: "{\"a\":1}".into(),
            webhooks: vec![[9u8; 16], [1u8; 16]],
        }
    }

    fn worker(script: &str, cooldown: Duration) -> Worker {
        Worker::new(Config {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            timeout: Duration::from_millis(1500),
            cooldown,
        })
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("geodb-outbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    // Logs each request line to a file; answers `entered` the first time, `duplicate` after.
    fn script(log: &std::path::Path) -> String {
        format!(
            "echo ready; n=0; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; \
             if [ $n -eq 0 ]; then printf '+8\\nentered\\n'; else printf '+10\\nduplicate\\n'; fi; n=1; done",
            log.display()
        )
    }

    #[test]
    fn config_is_off_by_default_and_needs_both_paths() {
        assert!(Config::from_vars(|_| None).is_none());
        assert!(
            Config::from_vars(|k| (k == "GEODB_OUTBOX_MIRROR").then(|| "off".into())).is_none()
        );
        let on_only = |k: &str| (k == "GEODB_OUTBOX_MIRROR").then(|| "1".to_string());
        assert!(Config::from_vars(on_only).is_none());
        let all = |k: &str| match k {
            "GEODB_OUTBOX_MIRROR" => Some("true".to_string()),
            "GEODB_OUTBOX_KEY_PATH" => Some("/k".to_string()),
            "GEODB_BINDING_PATH" => Some("/b".to_string()),
            _ => None,
        };
        let cfg = Config::from_vars(all).unwrap();
        assert_eq!(cfg.args.last().unwrap(), "pipe");
    }

    #[test]
    fn event_types_cover_every_outbox_type() {
        let all = [
            "proposal_created",
            "proposal_updated",
            "proposal_voted",
            "proposal_executed",
            "proposal_settings_updated",
            "proposal_rejected",
            "bounty_interest",
            "bounty_allocated",
            "bounty_payout",
            "bounty_created",
            "proposal_comment",
            "comment",
            "entity_votes_threshold",
        ];
        let codes: Vec<u8> = all.iter().map(|t| event_type_code(t).unwrap()).collect();
        let mut sorted = codes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
        assert!(codes.iter().all(|c| *c < 32));
        assert_eq!(event_type_code("nope"), None);
    }

    #[test]
    fn enter_line_shape_and_bounds() {
        let line = enter_line(&unit()).unwrap();
        let hooks = format!("{},{}", "09".repeat(16), "01".repeat(16));
        let payload: String = "{\"a\":1}".bytes().map(|b| format!("{b:02x}")).collect();
        assert_eq!(line, format!("enter\t{KEY}\t5\thex:{payload}\t{hooks}\n"));

        let mut u = unit();
        u.webhooks.clear();
        assert!(enter_line(&u).unwrap().ends_with("\t\n"));
        u.payload = "x".repeat(PAYLOAD_MAX + 1);
        assert!(matches!(enter_line(&u), Err(MirrorError::TooLong(_))));
        u.payload = "x".repeat(PAYLOAD_MAX);
        assert!(enter_line(&u).is_ok());
        for bad in ["", "zz", &"A".repeat(64), &KEY[1..]] {
            let mut b = unit();
            b.ledger_key_hex = bad.to_string();
            assert!(
                matches!(enter_line(&b), Err(MirrorError::BadUnit(_))),
                "{bad}"
            );
        }
        let mut b = unit();
        b.event_type = "other".into();
        assert!(matches!(enter_line(&b), Err(MirrorError::BadUnit(_))));
        b = unit();
        b.webhooks = vec![[0; 16]; HOOKS_MAX + 1];
        assert!(matches!(enter_line(&b), Err(MirrorError::BadUnit(_))));
    }

    #[tokio::test]
    async fn enters_over_one_held_child_and_reads_the_answers() {
        let log = tmp("ok");
        let mut w = worker(&script(&log), Duration::ZERO);
        assert_eq!(w.enter(&unit()).await.unwrap(), Entered::New);
        assert_eq!(w.enter(&unit()).await.unwrap(), Entered::Duplicate);
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(sent.lines().count(), 2);
        assert!(sent.starts_with(&format!("enter\t{KEY}\t5\thex:")));
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn a_refusal_drops_the_child_and_cools_down() {
        let mut w = worker(
            "echo ready; read -r l; printf '%s\\n' '-16' 'NotFoundOrRestri'",
            Duration::from_secs(60),
        );
        assert!(matches!(
            w.enter(&unit()).await,
            Err(MirrorError::Refused(_))
        ));
        assert!(matches!(
            w.enter(&unit()).await,
            Err(MirrorError::CoolingDown)
        ));
    }

    // The first child answers one enter, then dies; the second answers every enter.
    fn dying_script(state: &std::path::Path) -> String {
        format!(
            "echo ready; if [ -e {s} ]; then while IFS= read -r l; do printf '+8\\nentered\\n'; done; \
             else : > {s}; read -r l; printf '+8\\nentered\\n'; exit 0; fi",
            s = state.display()
        )
    }

    #[tokio::test]
    async fn a_pipe_closed_between_units_loses_nothing() {
        let state = tmp("dying");
        let mut w = worker(&dying_script(&state), Duration::from_secs(60));
        assert_eq!(w.enter(&unit()).await.unwrap(), Entered::New);
        // The child has exited; the next unit hits the dead pipe, reopens and still lands, even
        // though the cool-down is a minute.
        assert_eq!(w.enter(&unit()).await.unwrap(), Entered::New);
        assert_eq!(w.enter(&unit()).await.unwrap(), Entered::New);
        let _ = std::fs::remove_file(&state);
    }

    #[tokio::test]
    async fn a_fresh_pipe_that_fails_is_not_retried() {
        let mut w = worker(
            "echo ready; read -r l; printf '%s\\n' '-16' 'NotFoundOrRestri'",
            Duration::from_secs(60),
        );
        assert!(matches!(
            w.enter(&unit()).await,
            Err(MirrorError::Refused(_))
        ));
        assert!(matches!(
            w.enter(&unit()).await,
            Err(MirrorError::CoolingDown)
        ));
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error_not_a_panic() {
        let mut w = worker("exit 0", Duration::ZERO);
        w.cfg.program = "/nonexistent/geodb-client".into();
        assert!(matches!(w.enter(&unit()).await, Err(MirrorError::Spawn(_))));
    }

    #[tokio::test]
    async fn submit_never_blocks_and_drops_past_the_bound() {
        // No child ever answers: the worker stalls on its first spawn timeout, so the queue fills.
        let mut w = worker("sleep 30", Duration::from_secs(60));
        w.cfg.timeout = Duration::from_millis(50);
        let (tx, rx) = mpsc::channel(QUEUE_MAX);
        let task = tokio::spawn(run(w, rx, 0, Expiry::default(), RETRY_INTERVAL));
        let m = OutboxMirror { tx };
        let started = Instant::now();
        for _ in 0..QUEUE_MAX * 2 + 10 {
            m.submit(unit());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        task.abort();
    }

    // A client that fails its first `fail` calls, then answers; records the keys it entered.
    struct Fake {
        fail: usize,
        calls: usize,
        held: std::collections::HashSet<String>,
        entered: Vec<String>,
        /// Keys geo-db answers `Refused` to, however often they are tried.
        refuse: std::collections::HashSet<String>,
    }

    impl Fake {
        fn new(fail: usize) -> Self {
            Fake {
                fail,
                calls: 0,
                held: Default::default(),
                entered: vec![],
                refuse: Default::default(),
            }
        }
    }

    impl Enter for Fake {
        async fn enter(&mut self, unit: &Unit) -> Result<Entered, MirrorError> {
            self.calls += 1;
            if self.refuse.contains(&unit.ledger_key_hex) {
                return Err(MirrorError::Refused("no".into()));
            }
            if self.calls <= self.fail {
                return Err(MirrorError::Closed);
            }
            if self.held.insert(unit.ledger_key_hex.clone()) {
                self.entered.push(unit.ledger_key_hex.clone());
                Ok(Entered::New)
            } else {
                Ok(Entered::Duplicate)
            }
        }
    }

    fn unit_n(n: usize) -> Unit {
        let mut u = unit();
        u.ledger_key_hex = format!("{n:064x}");
        u
    }

    fn backlog(max: usize) -> Backlog {
        Backlog::new(max, Expiry::default())
    }

    #[test]
    fn backlog_flag_is_off_by_default_and_bound_is_configurable() {
        assert_eq!(backlog_from_vars(|_| None), None);
        let on = |k: &str| (k == "GEODB_OUTBOX_MIRROR_BACKLOG").then(|| "1".to_string());
        assert_eq!(backlog_from_vars(on), Some(BACKLOG_MAX_DEFAULT));
        let sized = |k: &str| match k {
            "GEODB_OUTBOX_MIRROR_BACKLOG" => Some("true".to_string()),
            "GEODB_OUTBOX_MIRROR_BACKLOG_MAX" => Some("50".to_string()),
            _ => None,
        };
        assert_eq!(backlog_from_vars(sized), Some(50));
        let bad = |k: &str| match k {
            "GEODB_OUTBOX_MIRROR_BACKLOG" => Some("1".to_string()),
            "GEODB_OUTBOX_MIRROR_BACKLOG_MAX" => Some("0".to_string()),
            _ => None,
        };
        assert_eq!(backlog_from_vars(bad), Some(BACKLOG_MAX_DEFAULT));
        // The bound alone does not turn it on.
        let only_bound = |k: &str| (k == "GEODB_OUTBOX_MIRROR_BACKLOG_MAX").then(|| "5".into());
        assert_eq!(backlog_from_vars(only_bound), None);
    }

    #[tokio::test]
    async fn failed_units_are_kept_and_retried_in_order_without_duplicates() {
        let mut w = Fake::new(3);
        let mut b = backlog(100);
        for n in 0..5 {
            b.push(unit_n(n), false);
            b.drain(&mut w).await;
        }
        // The first three calls failed: unit 0 failed once, unit 1 and 2 waited behind it.
        // Everything then landed, oldest first, each key once.
        assert!(b.units.is_empty());
        let want: Vec<String> = (0..5).map(|n| unit_n(n).ledger_key_hex).collect();
        assert_eq!(w.entered, want);
    }

    #[tokio::test]
    async fn a_failure_keeps_the_head_and_blocks_the_rest() {
        let mut w = Fake::new(usize::MAX);
        let mut b = backlog(100);
        for n in 0..3 {
            b.push(unit_n(n), false);
            b.drain(&mut w).await;
        }
        assert_eq!(b.units.len(), 3);
        assert_eq!(b.units[0].0, unit_n(0));
        // One attempt a pass, on the head only.
        assert_eq!(w.calls, 3);
        w.fail = 0;
        b.drain(&mut w).await;
        assert!(b.units.is_empty());
        assert_eq!(w.entered.len(), 3);
        assert_eq!(w.entered[0], unit_n(0).ledger_key_hex);
    }

    #[tokio::test]
    async fn a_unit_that_landed_but_was_retried_is_a_duplicate_not_a_second_write() {
        let mut w = Fake::new(0);
        w.held.insert(unit_n(1).ledger_key_hex);
        let mut b = backlog(10);
        b.push(unit_n(1), true);
        b.drain(&mut w).await;
        assert!(b.units.is_empty());
        assert!(w.entered.is_empty());
    }

    #[tokio::test]
    async fn overflow_drops_the_oldest_and_keeps_the_newest() {
        let mut w = Fake::new(usize::MAX);
        let mut b = backlog(3);
        for n in 0..5 {
            b.push(unit_n(n), false);
            b.drain(&mut w).await;
        }
        let kept: Vec<Unit> = b.units.iter().map(|(u, _)| u.clone()).collect();
        assert_eq!(kept, vec![unit_n(2), unit_n(3), unit_n(4)]);
        w.fail = 0;
        b.drain(&mut w).await;
        let want: Vec<String> = (2..5).map(|n| unit_n(n).ledger_key_hex).collect();
        assert_eq!(w.entered, want);
    }

    #[tokio::test]
    async fn an_unsendable_unit_is_skipped_not_kept() {
        let mut b = backlog(10);
        let mut bad = unit_n(1);
        bad.ledger_key_hex = "zz".into();
        // The real line check, through `attempt`.
        struct Real;
        impl Enter for Real {
            async fn enter(&mut self, unit: &Unit) -> Result<Entered, MirrorError> {
                enter_line(unit).map(|_| Entered::New)
            }
        }
        b.push(bad, false);
        b.push(unit_n(2), false);
        b.drain(&mut Real).await;
        assert!(b.units.is_empty());
    }

    #[tokio::test]
    async fn run_retries_on_the_tick_after_the_session_recovers() {
        let (tx, rx) = mpsc::channel(16);
        let w = Fake::new(2);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        struct Shared(Fake, std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        impl Enter for Shared {
            async fn enter(&mut self, unit: &Unit) -> Result<Entered, MirrorError> {
                let r = self.0.enter(unit).await;
                if r.is_ok() {
                    self.1.lock().unwrap().push(unit.ledger_key_hex.clone());
                }
                r
            }
        }
        let task = tokio::spawn(run(
            Shared(w, seen.clone()),
            rx,
            10,
            Expiry::default(),
            Duration::from_millis(10),
        ));
        tx.send(unit_n(1)).await.unwrap();
        tx.send(unit_n(2)).await.unwrap();
        // No further traffic: the retry tick alone must drain the backlog, in order.
        for _ in 0..200 {
            if seen.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(tx);
        task.await.unwrap();
        let want: Vec<String> = (1..3).map(|n| unit_n(n).ledger_key_hex).collect();
        assert_eq!(*seen.lock().unwrap(), want);
    }

    fn expiring(max: usize, attempts: u32, age: Duration) -> Backlog {
        Backlog::new(max, Expiry { attempts, age })
    }

    #[tokio::test]
    async fn a_refused_head_expires_after_n_attempts_and_the_rest_keep_their_order() {
        let mut w = Fake::new(0);
        w.refuse.insert(unit_n(0).ledger_key_hex);
        let mut b = expiring(100, 3, Duration::from_secs(3600));
        for n in 0..4 {
            b.push(unit_n(n), false);
        }
        // Passes 1 and 2: unit 0 is refused and blocks 1..3.
        b.drain(&mut w).await;
        b.drain(&mut w).await;
        assert_eq!(b.units.len(), 4);
        assert!(w.entered.is_empty());
        // Pass 3 is the third refusal: 0 expires and the same pass enters 1..3 in order.
        b.drain(&mut w).await;
        assert!(b.units.is_empty());
        let want: Vec<String> = (1..4).map(|n| unit_n(n).ledger_key_hex).collect();
        assert_eq!(w.entered, want);
        assert_eq!((b.head_refused, b.head_since), (0, None));
    }

    #[tokio::test]
    async fn a_refused_head_expires_by_age() {
        let mut w = Fake::new(0);
        w.refuse.insert(unit_n(0).ledger_key_hex);
        // Attempts are out of reach; the age is already spent at the first refusal.
        let mut b = expiring(100, 1_000, Duration::ZERO);
        b.push(unit_n(0), false);
        b.push(unit_n(1), false);
        b.drain(&mut w).await;
        assert!(b.units.is_empty());
        assert_eq!(w.entered, vec![unit_n(1).ledger_key_hex]);
    }

    #[tokio::test]
    async fn transport_failures_never_expire_a_unit() {
        let mut w = Fake::new(usize::MAX);
        let mut b = expiring(100, 1, Duration::ZERO);
        b.push(unit_n(0), false);
        for _ in 0..5 {
            b.drain(&mut w).await;
        }
        assert_eq!(b.units.len(), 1);
        assert_eq!(b.units[0].0, unit_n(0));
        w.fail = 0;
        b.drain(&mut w).await;
        assert_eq!(w.entered, vec![unit_n(0).ledger_key_hex]);
    }

    #[tokio::test]
    async fn an_expired_head_does_not_carry_its_count_to_the_next_unit() {
        let mut w = Fake::new(0);
        w.refuse.insert(unit_n(0).ledger_key_hex);
        w.refuse.insert(unit_n(1).ledger_key_hex);
        let mut b = expiring(100, 2, Duration::from_secs(3600));
        b.push(unit_n(0), false);
        b.push(unit_n(1), false);
        b.push(unit_n(2), false);
        b.drain(&mut w).await;
        assert_eq!(b.units.len(), 3);
        // Second refusal expires 0; 1 starts at zero and is refused once, so it stays.
        b.drain(&mut w).await;
        assert_eq!(b.units.len(), 2);
        assert_eq!(b.units[0].0, unit_n(1));
        assert_eq!(b.head_refused, 1);
        b.drain(&mut w).await;
        assert!(b.units.is_empty());
        assert_eq!(w.entered, vec![unit_n(2).ledger_key_hex]);
    }

    #[test]
    fn expiry_is_configurable_with_defaults_for_bad_values() {
        assert_eq!(expiry_from_vars(|_| None), Expiry::default());
        let set = |k: &str| match k {
            "GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_ATTEMPTS" => Some("5".to_string()),
            "GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_SECS" => Some("45".to_string()),
            _ => None,
        };
        assert_eq!(
            expiry_from_vars(set),
            Expiry {
                attempts: 5,
                age: Duration::from_secs(45)
            }
        );
        let bad = |k: &str| match k {
            "GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_ATTEMPTS" => Some("0".to_string()),
            "GEODB_OUTBOX_MIRROR_BACKLOG_EXPIRE_SECS" => Some("soon".to_string()),
            _ => None,
        };
        assert_eq!(expiry_from_vars(bad), Expiry::default());
    }
}
