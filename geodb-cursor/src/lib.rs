//! Best-effort mirror of an indexer's `meta` cursor row into geo-db's derived class.
//!
//! Postgres stays the source of truth. After the Postgres cursor write succeeds, the caller hands
//! `(writer id, block number)` to [`CursorMirror::mirror`], which sends one `put` over a held
//! `geodb-client derived pipe` child (geo-db `docs/api/10-embedded-and-wire.md`, the `derived pipe`
//! row). Nothing here can fail the block path: every error is logged, counted
//! (`geodb_cursor_mirror_errors_total`) and swallowed, each call is bounded by a timeout, and after
//! a failure the child is not respawned until a cool-down passes. The serve ends a session at its
//! grant TTL (300 s) or after 60 s idle, so a held child is replaced at [`SESSION_REFRESH`] and a
//! request that a held child's end-of-session refused is replayed once on a new one.
//!
//! The row is `cursors` (collection 10, geo-db 6a770ba752), keyed by the writer id. Its value is the
//! bytes `<decimal block_number>:<opaque cursor>`, sent as `hex:` so the tab-separated line cannot be
//! broken by the cursor's content. A value over [`VALUE_MAX`] (1024 bytes) is logged, counted
//! (`geodb_cursor_mirror_skipped_total`) and skipped, never truncated. The 1024 limit is an estimate
//! (four times a 256-byte longest cursor), so the cursor's byte length is recorded on every call as
//! the gauge `geodb_cursor_bytes` and in the skip log.
//!
//! The mirror is OFF unless `GEODB_CURSOR_MIRROR` is `1` or `true`.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

pub mod derived;
pub mod derived_read;
pub mod meta_read;

/// Default derived collection: `cursors = 10` (geo-db 6a770ba752).
pub const DEFAULT_COLLECTION: u32 = 10;

/// Most bytes of a write's answer frame (`applied: <n>`).
const WRITE_FRAME_MAX: usize = 4096;

/// `cursor_value_max` of the `cursors` collection: the most bytes of one value.
pub const VALUE_MAX: usize = 1024;

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
    #[error("value of {0} bytes is over the {VALUE_MAX} byte bound")]
    TooLong(usize),
    #[error("cooling down after a failure")]
    CoolingDown,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub collection: u32,
    /// Bound on one request, and on the spawn handshake.
    pub timeout: Duration,
    /// After a failure, no respawn for this long.
    pub cooldown: Duration,
}

impl Config {
    /// Reads the environment. `None` when the mirror is off (the default) or misconfigured.
    /// `GEODB_CURSOR_MIRROR`, `GEODB_CLIENT_BIN` (default `geodb-client`), `GEODB_KEY_PATH` and
    /// `GEODB_BINDING_PATH` (both required), `GEODB_CURSOR_COLLECTION` (default 10).
    pub fn from_env() -> Option<Config> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Config> {
        Self::from_vars_for("GEODB_CURSOR_MIRROR", get)
    }

    /// `from_vars` for the mirror whose on-switch is the variable `flag` (`1` or `true`).
    pub fn from_vars_for(flag: &str, get: impl Fn(&str) -> Option<String>) -> Option<Config> {
        let on = matches!(
            get(flag).as_deref().map(str::trim),
            Some("1") | Some("true")
        );
        if !on {
            return None;
        }
        let (Some(key), Some(binding)) = (get("GEODB_KEY_PATH"), get("GEODB_BINDING_PATH")) else {
            warn!(
                event = "geodb_cursor.misconfigured",
                flag,
                "mirror is on but GEODB_KEY_PATH or GEODB_BINDING_PATH is unset; mirror disabled"
            );
            return None;
        };
        let collection = get("GEODB_CURSOR_COLLECTION")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(DEFAULT_COLLECTION);
        Some(Config {
            program: get("GEODB_CLIENT_BIN")
                .unwrap_or_else(|| "geodb-client".into())
                .into(),
            args: ["--key", &key, "--binding", &binding, "derived", "pipe"]
                .iter()
                .map(OsString::from)
                .collect(),
            collection,
            timeout: Duration::from_secs(2),
            cooldown: Duration::from_secs(10),
        })
    }
}

/// The serve mints each session's grant for `grant_ttl_s` (300 s, `geodb/supervisor.sh`) and closes
/// a pipe idle past 60 s; a held child is replaced once it is this old, before the grant lapses.
pub const SESSION_REFRESH: Duration = Duration::from_secs(240);

struct Pipe {
    opened_at: Instant,
    /// Whether this child has answered a request: only then can a failure be the session ending.
    answered: bool,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(Default)]
struct State {
    pipe: Option<Pipe>,
    failed_at: Option<Instant>,
}

/// Holds the `derived pipe` child. Cheap to build; the child starts on first use.
pub struct CursorMirror {
    cfg: Config,
    session_refresh: Duration,
    state: Mutex<State>,
}

/// Whether a new session can cure `e`: the pipe was closed, or the serve ended it for an idle
/// bound or an expired grant. Any other refusal is the serve's answer to the request itself.
fn session_lost(e: &MirrorError) -> bool {
    match e {
        MirrorError::Closed | MirrorError::Io(_) => true,
        MirrorError::Refused(m) => {
            m.contains("CapabilityTokenExpired") || m.contains("TransportClosed")
        }
        _ => false,
    }
}

impl CursorMirror {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            session_refresh: SESSION_REFRESH,
            state: Mutex::new(State::default()),
        }
    }

    /// Replaces the held child once it is older than `d` (default [`SESSION_REFRESH`]).
    pub fn with_session_refresh(mut self, d: Duration) -> Self {
        self.session_refresh = d;
        self
    }

    /// `Some` only when `GEODB_CURSOR_MIRROR` is on and configured.
    pub fn from_env() -> Option<Self> {
        Config::from_env().map(Self::new)
    }

    /// Mirrors `(id, cursor, block)`. Never fails the caller: an error is logged and counted.
    pub async fn mirror(&self, id: &str, cursor: &str, block: u64) {
        metrics::gauge!("geodb_cursor_bytes", "indexer" => id.to_string()).set(cursor.len() as f64);
        if let Err(e) = self.try_mirror(id, cursor, block).await {
            if let MirrorError::TooLong(n) = e {
                metrics::counter!("geodb_cursor_mirror_skipped_total", "indexer" => id.to_string())
                    .increment(1);
                warn!(
                    event = "geodb_cursor.value_too_long",
                    indexer_id = id,
                    block_number = block,
                    cursor_bytes = cursor.len(),
                    value_bytes = n,
                    limit = VALUE_MAX,
                    "cursor value over the geo-db bound; skipped, not truncated"
                );
                return;
            }
            if !matches!(e, MirrorError::CoolingDown) {
                warn!(
                    event = "geodb_cursor.mirror_failed",
                    indexer_id = id,
                    block_number = block,
                    error = %e,
                    "geo-db cursor mirror failed; Postgres is unaffected"
                );
            }
            metrics::counter!("geodb_cursor_mirror_errors_total", "indexer" => id.to_string())
                .increment(1);
        } else {
            metrics::counter!("geodb_cursor_mirror_puts_total", "indexer" => id.to_string())
                .increment(1);
        }
    }

    /// The fallible form, for tests.
    pub async fn try_mirror(&self, id: &str, cursor: &str, block: u64) -> Result<(), MirrorError> {
        // The pipe's fields are tab-separated, so an id with a tab or newline cannot be sent.
        if id.is_empty() || id.contains(['\t', '\n']) {
            return Err(MirrorError::Refused(format!("bad key {id:?}")));
        }
        let value = format!("{block}:{cursor}");
        if value.len() > VALUE_MAX {
            return Err(MirrorError::TooLong(value.len()));
        }
        let hex: String = value.bytes().map(|b| format!("{b:02x}")).collect();
        let line = format!("put\t{}\t0\t0\t{}\thex:{}\n", self.cfg.collection, id, hex);
        self.send_lines(std::slice::from_ref(&line)).await
    }

    /// Sends request lines in order over the held child, one frame answered per line, under one
    /// hold of the pipe. The first failure drops the child (respawned after the cool-down) and
    /// ends the batch; the lines after it are not sent. `Ok` when every line was answered `+`.
    pub async fn send_lines(&self, lines: &[String]) -> Result<(), MirrorError> {
        self.exchange(lines, WRITE_FRAME_MAX).await.map(|_| ())
    }

    /// As [`send_lines`](Self::send_lines), and returns the body of each `+` frame, in order. A
    /// frame larger than `frame_max` bytes is a broken child. This is the read path of the
    /// derived `get` and `scan` verbs (see [`derived_read::DerivedReader`]).
    pub async fn exchange(
        &self,
        lines: &[String],
        frame_max: usize,
    ) -> Result<Vec<String>, MirrorError> {
        let mut bodies = Vec::with_capacity(lines.len());
        let mut st = self.state.lock().await;
        // A session near the end of its grant is replaced before the request, not refused by it.
        if st
            .pipe
            .as_ref()
            .is_some_and(|p| p.opened_at.elapsed() > self.session_refresh)
        {
            info!(
                event = "geodb_cursor.session_refresh",
                "geodb-client session is near its grant expiry; replacing it"
            );
            st.pipe = None;
        }
        let mut replayed = false;
        let mut i = 0;
        while i < lines.len() {
            if st.pipe.is_none() {
                // After a failure the next spawn waits out the cool-down, except the one that
                // replays a line a lost session refused: that failure was the old session's.
                if !replayed
                    && st
                        .failed_at
                        .is_some_and(|t| t.elapsed() < self.cfg.cooldown)
                {
                    return Err(MirrorError::CoolingDown);
                }
                match tokio::time::timeout(self.cfg.timeout, self.spawn()).await {
                    Ok(Ok(p)) => {
                        info!(
                            event = "geodb_cursor.pipe_started",
                            "geodb-client derived pipe ready"
                        );
                        st.pipe = Some(p);
                    }
                    Ok(Err(e)) => {
                        st.failed_at = Some(Instant::now());
                        return Err(e);
                    }
                    Err(_) => {
                        st.failed_at = Some(Instant::now());
                        return Err(MirrorError::Timeout(self.cfg.timeout));
                    }
                }
            }
            let pipe = st.pipe.as_mut().expect("set above");
            let res =
                match tokio::time::timeout(self.cfg.timeout, request(pipe, &lines[i], frame_max))
                    .await
                {
                    Ok(r) => r,
                    Err(_) => Err(MirrorError::Timeout(self.cfg.timeout)),
                };
            match res {
                Ok(body) => {
                    if let Some(p) = st.pipe.as_mut() {
                        p.answered = true;
                    }
                    bodies.push(body);
                    i += 1;
                    replayed = false;
                }
                Err(e) => {
                    // A refusal or error leaves the tool exiting; drop it (kill_on_drop). A pipe
                    // that had answered before and was then ended by the serve (idle bound or
                    // expired grant) costs nothing: the same line goes once more over a new
                    // session, which is safe because puts are last-write-wins and gets, scans
                    // and deletes repeat harmlessly. Anything else cools down.
                    let reused = st.pipe.as_ref().is_some_and(|p| p.answered);
                    st.pipe = None;
                    if reused && !replayed && session_lost(&e) {
                        metrics::counter!("geodb_cursor_session_replays_total").increment(1);
                        info!(
                            event = "geodb_cursor.session_replay",
                            error = %e,
                            "held pipe was ended by the serve; replaying the request on a new session"
                        );
                        replayed = true;
                        continue;
                    }
                    st.failed_at = Some(Instant::now());
                    return Err(e);
                }
            }
        }
        st.failed_at = None;
        Ok(bodies)
    }

    async fn spawn(&self) -> Result<Pipe, MirrorError> {
        let mut child = Command::new(&self.cfg.program)
            .args(&self.cfg.args)
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
            opened_at: Instant::now(),
            answered: false,
            child,
            stdin,
            stdout,
        })
    }
}

/// One request line, one frame: `+<n>\n<n bytes>` ok, `-<n>\n<n bytes>` refused.
async fn request(p: &mut Pipe, line: &str, frame_max: usize) -> Result<String, MirrorError> {
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
    if n > frame_max {
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

impl Drop for Pipe {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(script: &str, cooldown: Duration) -> CursorMirror {
        CursorMirror::new(Config {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            collection: 10,
            timeout: Duration::from_millis(1500),
            cooldown,
        })
    }

    // Echoes each request line into $LOG so the test can read what was sent, answers `applied: 1`.
    fn ok_script(log: &std::path::Path) -> String {
        format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+10\\napplied: 1'; done",
            log.display()
        )
    }

    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("geodb-cursor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[tokio::test]
    async fn sends_put_lines_over_one_held_child() {
        let log = tmp("ok");
        let m = fake(&ok_script(&log), Duration::ZERO);
        m.try_mirror("hermes_pipeline", "CUR1", 100).await.unwrap();
        m.try_mirror("hermes_ipfs_cache", "CUR2", 101)
            .await
            .unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            sent,
            format!(
                "put\t10\t0\t0\thermes_pipeline\thex:{}\nput\t10\t0\t0\thermes_ipfs_cache\thex:{}\n",
                hex("100:CUR1"),
                hex("101:CUR2")
            )
        );
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn refusal_is_an_error_then_respawns_after_cooldown() {
        let log = tmp("refuse");
        // First child refuses once and exits; the second is healthy.
        let flag = tmp("refuse-flag");
        let script = format!(
            "echo ready; if [ ! -e {f} ]; then : > {f}; read -r l; printf -- '-26\\nrefused: DefiniteRefusal X'; exit 1; fi; {ok}",
            f = flag.display(),
            ok = ok_script(&log).replacen("echo ready; ", "", 1)
        );
        let m = fake(&script, Duration::from_millis(50));
        let e = m.try_mirror("a", "c", 1).await.unwrap_err();
        assert!(matches!(e, MirrorError::Refused(_)), "{e}");
        assert!(matches!(
            m.try_mirror("a", "c", 2).await.unwrap_err(),
            MirrorError::CoolingDown
        ));
        tokio::time::sleep(Duration::from_millis(80)).await;
        m.try_mirror("a", "c", 3).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("put\t10\t0\t0\ta\thex:{}\n", hex("3:c"))
        );
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&flag);
    }

    // The first child answers `answer` requests, then refuses the next with `refusal` and exits;
    // every later child is healthy. Each start and each request line is appended to $LOG.
    fn expiring_script(
        log: &std::path::Path,
        flag: &std::path::Path,
        answer: u32,
        refusal: &str,
    ) -> String {
        format!(
            "echo spawn >> {log}; echo ready; n=0; first=0; [ -e {f} ] || {{ first=1; : > {f}; }}; \
             while IFS= read -r l; do printf '%s\\n' \"$l\" >> {log}; \
             if [ $first = 1 ] && [ $n -ge {answer} ]; then printf -- '-{len}\\n{refusal}'; exit 1; fi; \
             n=$((n+1)); printf '+10\\napplied: 1'; done",
            log = log.display(),
            f = flag.display(),
            len = refusal.len(),
        )
    }

    #[tokio::test]
    async fn an_expired_grant_on_a_held_pipe_replays_the_request_on_a_new_session() {
        let (log, flag) = (tmp("expired"), tmp("expired-flag"));
        let script = expiring_script(&log, &flag, 1, "DefiniteRefusal CapabilityTokenExpired");
        let m = fake(&script, Duration::from_secs(60));
        m.try_mirror("a", "c", 1).await.unwrap();
        // Past the grant: refused once, replayed on a new session, despite the long cool-down.
        m.try_mirror("a", "c", 2).await.unwrap();
        m.try_mirror("a", "c", 3).await.unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        let put = |b: u32| format!("put\t10\t0\t0\ta\thex:{}", hex(&format!("{b}:c")));
        assert_eq!(
            sent,
            format!(
                "spawn\n{}\n{}\nspawn\n{}\n{}\n",
                put(1),
                put(2),
                put(2),
                put(3)
            )
        );
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&flag);
    }

    #[tokio::test]
    async fn a_lost_session_in_the_middle_of_a_batch_resumes_at_the_refused_line() {
        let (log, flag) = (tmp("resume"), tmp("resume-flag"));
        let script = expiring_script(&log, &flag, 1, "error: TransportClosed");
        let m = fake(&script, Duration::from_secs(60));
        m.try_mirror("w", "c", 0).await.unwrap(); // the held pipe has answered once
        let lines: Vec<String> = ["x", "y", "z"]
            .iter()
            .map(|k| format!("put\t10\t0\t0\t{k}\thex:00\n"))
            .collect();
        let bodies = m.exchange(&lines, WRITE_FRAME_MAX).await.unwrap();
        assert_eq!(bodies.len(), 3);
        let sent = std::fs::read_to_string(&log).unwrap();
        // x was refused by the old session and sent again; y and z were sent once.
        assert_eq!(sent.matches("\tx\t").count(), 2);
        assert_eq!(sent.matches("\ty\t").count(), 1);
        assert_eq!(sent.matches("\tz\t").count(), 1);
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&flag);
    }

    #[tokio::test]
    async fn a_session_older_than_the_refresh_is_replaced_before_it_can_expire() {
        let log = tmp("refresh");
        let script = format!("echo spawn >> {}; {}", log.display(), ok_script(&log));
        let m = fake(&script, Duration::ZERO).with_session_refresh(Duration::from_millis(50));
        m.try_mirror("a", "c", 1).await.unwrap();
        m.try_mirror("a", "c", 2).await.unwrap(); // young: same child
        tokio::time::sleep(Duration::from_millis(80)).await;
        m.try_mirror("a", "c", 3).await.unwrap(); // old: new child
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(sent.matches("spawn").count(), 2, "{sent}");
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn a_refusal_that_is_not_the_session_ending_is_not_replayed() {
        let (log, flag) = (tmp("norep"), tmp("norep-flag"));
        let script = expiring_script(&log, &flag, 1, "DefiniteRefusal NotPermitted");
        let m = fake(&script, Duration::from_secs(60));
        m.try_mirror("a", "c", 1).await.unwrap();
        assert!(matches!(
            m.try_mirror("a", "c", 2).await.unwrap_err(),
            MirrorError::Refused(_)
        ));
        assert!(matches!(
            m.try_mirror("a", "c", 3).await.unwrap_err(),
            MirrorError::CoolingDown
        ));
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&flag);
    }

    #[tokio::test]
    async fn child_exit_mid_run_is_an_error_not_a_panic() {
        let m = fake("echo ready; read -r l; exit 0", Duration::ZERO);
        assert!(matches!(
            m.try_mirror("a", "c", 1).await.unwrap_err(),
            MirrorError::Closed
        ));
    }

    #[tokio::test]
    async fn a_silent_child_times_out() {
        let m = fake("echo ready; sleep 30", Duration::ZERO);
        let t = Instant::now();
        assert!(matches!(
            m.try_mirror("a", "c", 1).await.unwrap_err(),
            MirrorError::Timeout(_)
        ));
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn missing_binary_and_bad_handshake_do_not_panic() {
        let m = CursorMirror::new(Config {
            program: "/nonexistent/geodb-client".into(),
            args: vec![],
            collection: 10,
            timeout: Duration::from_secs(1),
            cooldown: Duration::ZERO,
        });
        m.mirror("a", "c", 1).await; // swallowed
        let m = fake("echo nope", Duration::ZERO);
        assert!(matches!(
            m.try_mirror("a", "c", 1).await.unwrap_err(),
            MirrorError::NotReady(_)
        ));
    }

    #[test]
    fn flag_defaults_off_and_needs_paths() {
        let none = |_: &str| None;
        assert!(Config::from_vars(none).is_none());
        let off = |k: &str| (k == "GEODB_CURSOR_MIRROR").then(|| "0".to_string());
        assert!(Config::from_vars(off).is_none());
        let no_paths = |k: &str| (k == "GEODB_CURSOR_MIRROR").then(|| "1".to_string());
        assert!(Config::from_vars(no_paths).is_none());
        let on = |k: &str| match k {
            "GEODB_CURSOR_MIRROR" => Some("true".to_string()),
            "GEODB_KEY_PATH" => Some("/k".into()),
            "GEODB_BINDING_PATH" => Some("/b".into()),
            _ => None,
        };
        let c = Config::from_vars(on).unwrap();
        assert_eq!(c.collection, DEFAULT_COLLECTION);
        assert_eq!(c.args.last().unwrap(), "pipe");
    }

    #[tokio::test]
    async fn a_cursor_with_a_tab_travels_as_hex_and_the_bound_is_exact() {
        let log = tmp("bound");
        let m = fake(&ok_script(&log), Duration::ZERO);
        // 1024 bytes total: "7:" plus a 1022-byte cursor holding a tab and a newline.
        let mut cursor = "x\ty\nz".to_string();
        cursor.push_str(&"a".repeat(1022 - cursor.len()));
        m.try_mirror("w", &cursor, 7).await.unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            sent.lines().count(),
            1,
            "one line, tab and newline hex-escaped"
        );
        assert!(sent.starts_with("put\t10\t0\t0\tw\thex:373a78"));
        // One byte over: refused before any I/O, nothing sent, child never needed.
        let over = format!("{cursor}b");
        let e = m.try_mirror("w", &over, 7).await.unwrap_err();
        assert!(matches!(e, MirrorError::TooLong(1025)), "{e}");
        m.mirror("w", &over, 7).await; // logged, counted, swallowed
        assert_eq!(std::fs::read_to_string(&log).unwrap(), sent);
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn tab_in_key_is_refused_before_any_io() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let m = fake("exit 1", Duration::ZERO);
        let e = rt.block_on(m.try_mirror("a\tb", "c", 1)).unwrap_err();
        assert!(matches!(e, MirrorError::Refused(_)));
    }
}
