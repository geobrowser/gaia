//! Best-effort mirror of an indexer's `meta` cursor row into geo-db's derived class.
//!
//! Postgres stays the source of truth. After the Postgres cursor write succeeds, the caller hands
//! `(writer id, block number)` to [`CursorMirror::mirror`], which sends one `put` over a held
//! `geodb-client derived pipe` child (geo-db `docs/api/10-embedded-and-wire.md`, the `derived pipe`
//! row). Nothing here can fail the block path: every error is logged, counted
//! (`geodb_cursor_mirror_errors_total`) and swallowed, each call is bounded by a timeout, and after
//! a failure the child is not respawned until a cool-down passes.
//!
//! Only the decimal `block_number` is mirrored. The derived class holds at most 64 value bytes
//! (`value_max`), which a substreams cursor does not fit, and `block_number` is what the two
//! cross-service readers use.
//!
//! The mirror is OFF unless `GEODB_CURSOR_MIRROR` is `1` or `true`.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Default derived collection: the next line after the registry's last (`proposal_vote_counts = 9`
/// at geo-db 3a83b5a2dc). A put to an unregistered collection is refused until geo-db registers it.
pub const DEFAULT_COLLECTION: u32 = 10;

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
        let on = matches!(
            get("GEODB_CURSOR_MIRROR").as_deref().map(str::trim),
            Some("1") | Some("true")
        );
        if !on {
            return None;
        }
        let (Some(key), Some(binding)) = (get("GEODB_KEY_PATH"), get("GEODB_BINDING_PATH")) else {
            warn!(
                event = "geodb_cursor.misconfigured",
                "GEODB_CURSOR_MIRROR is on but GEODB_KEY_PATH or GEODB_BINDING_PATH is unset; mirror disabled"
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

struct Pipe {
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
    state: Mutex<State>,
}

impl CursorMirror {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: Mutex::new(State::default()),
        }
    }

    /// `Some` only when `GEODB_CURSOR_MIRROR` is on and configured.
    pub fn from_env() -> Option<Self> {
        Config::from_env().map(Self::new)
    }

    /// Mirrors `(id, block)`. Never fails the caller: an error is logged and counted.
    pub async fn mirror(&self, id: &str, block: u64) {
        if let Err(e) = self.try_mirror(id, block).await {
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
    pub async fn try_mirror(&self, id: &str, block: u64) -> Result<(), MirrorError> {
        // The pipe's fields are tab-separated, so an id with a tab or newline cannot be sent.
        if id.is_empty() || id.contains(['\t', '\n']) {
            return Err(MirrorError::Refused(format!("bad key {id:?}")));
        }
        let mut st = self.state.lock().await;
        if st.pipe.is_none() {
            if let Some(t) = st.failed_at {
                if t.elapsed() < self.cfg.cooldown {
                    return Err(MirrorError::CoolingDown);
                }
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
        let line = format!("put\t{}\t0\t0\t{}\t{}\n", self.cfg.collection, id, block);
        let res = match tokio::time::timeout(self.cfg.timeout, request(pipe, &line)).await {
            Ok(r) => r,
            Err(_) => Err(MirrorError::Timeout(self.cfg.timeout)),
        };
        match res {
            Ok(()) => {
                st.failed_at = None;
                Ok(())
            }
            Err(e) => {
                // A refusal or error leaves the tool exiting; drop it (kill_on_drop) and respawn
                // after the cool-down. A `want` frame for a malformed line would also land here.
                st.pipe = None;
                st.failed_at = Some(Instant::now());
                Err(e)
            }
        }
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
            child,
            stdin,
            stdout,
        })
    }
}

/// One request line, one frame: `+<n>\n<n bytes>` ok, `-<n>\n<n bytes>` refused.
async fn request(p: &mut Pipe, line: &str) -> Result<(), MirrorError> {
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
    // A put's answer is `applied: <n>`; cap what a broken child can make us allocate.
    if n > 4096 {
        return Err(MirrorError::BadFrame(head.to_string()));
    }
    let mut body = vec![0u8; n];
    p.stdout.read_exact(&mut body).await?;
    let body = String::from_utf8_lossy(&body).into_owned();
    match sign {
        "+" => Ok(()),
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

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("geodb-cursor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[tokio::test]
    async fn sends_put_lines_over_one_held_child() {
        let log = tmp("ok");
        let m = fake(&ok_script(&log), Duration::ZERO);
        m.try_mirror("hermes_pipeline", 100).await.unwrap();
        m.try_mirror("hermes_ipfs_cache", 101).await.unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            sent,
            "put\t10\t0\t0\thermes_pipeline\t100\nput\t10\t0\t0\thermes_ipfs_cache\t101\n"
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
        let e = m.try_mirror("a", 1).await.unwrap_err();
        assert!(matches!(e, MirrorError::Refused(_)), "{e}");
        assert!(matches!(
            m.try_mirror("a", 2).await.unwrap_err(),
            MirrorError::CoolingDown
        ));
        tokio::time::sleep(Duration::from_millis(80)).await;
        m.try_mirror("a", 3).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "put\t10\t0\t0\ta\t3\n"
        );
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&flag);
    }

    #[tokio::test]
    async fn child_exit_mid_run_is_an_error_not_a_panic() {
        let m = fake("echo ready; read -r l; exit 0", Duration::ZERO);
        assert!(matches!(
            m.try_mirror("a", 1).await.unwrap_err(),
            MirrorError::Closed
        ));
    }

    #[tokio::test]
    async fn a_silent_child_times_out() {
        let m = fake("echo ready; sleep 30", Duration::ZERO);
        let t = Instant::now();
        assert!(matches!(
            m.try_mirror("a", 1).await.unwrap_err(),
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
        m.mirror("a", 1).await; // swallowed
        let m = fake("echo nope", Duration::ZERO);
        assert!(matches!(
            m.try_mirror("a", 1).await.unwrap_err(),
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

    #[test]
    fn tab_in_key_is_refused_before_any_io() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let m = fake("exit 1", Duration::ZERO);
        let e = rt.block_on(m.try_mirror("a\tb", 1)).unwrap_err();
        assert!(matches!(e, MirrorError::Refused(_)));
    }
}
