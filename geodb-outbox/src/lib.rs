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
//! The delivery side reads the same work class through [`reader::OutboxReader`], over the same
//! pipe client, behind its own flag `GEODB_OUTBOX_SHADOW_READ` (also off by default).

pub mod reader;

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

/// The handle the indexer holds. Cheap to clone; `submit` never blocks.
#[derive(Clone)]
pub struct OutboxMirror {
    tx: mpsc::Sender<Unit>,
}

impl OutboxMirror {
    /// Starts the background task on the current tokio runtime. Panics outside one.
    pub fn new(cfg: Config) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_MAX);
        tokio::spawn(run(Worker::new(cfg), rx));
        Self { tx }
    }

    /// `Some` only when `GEODB_OUTBOX_MIRROR` is on and configured.
    pub fn from_env() -> Option<Self> {
        Config::from_env().map(Self::new)
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

async fn run(mut w: Worker, mut rx: mpsc::Receiver<Unit>) {
    while let Some(unit) = rx.recv().await {
        metrics::gauge!("geodb_outbox_mirror_queue").set(rx.len() as f64);
        match w.enter(&unit).await {
            Ok(Entered::New) => {
                metrics::counter!("geodb_outbox_mirror_entered_total").increment(1);
            }
            Ok(Entered::Duplicate) => {
                metrics::counter!("geodb_outbox_mirror_duplicates_total").increment(1);
            }
            Err(MirrorError::TooLong(n)) => {
                metrics::counter!("geodb_outbox_mirror_skipped_total").increment(1);
                warn!(
                    event = "geodb_outbox.payload_too_long",
                    ledger_key = %unit.ledger_key_hex,
                    payload_bytes = n,
                    limit = PAYLOAD_MAX,
                    "payload over the geo-db bound; skipped, not truncated"
                );
            }
            Err(MirrorError::CoolingDown) => {
                metrics::counter!("geodb_outbox_mirror_errors_total").increment(1);
            }
            Err(e) => {
                metrics::counter!("geodb_outbox_mirror_errors_total").increment(1);
                warn!(
                    event = "geodb_outbox.mirror_failed",
                    ledger_key = %unit.ledger_key_hex,
                    error = %e,
                    "geo-db outbox mirror failed; Postgres is unaffected"
                );
            }
        }
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
        let task = tokio::spawn(run(w, rx));
        let m = OutboxMirror { tx };
        let started = Instant::now();
        for _ in 0..QUEUE_MAX * 2 + 10 {
            m.submit(unit());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        task.abort();
    }
}
