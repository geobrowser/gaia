//! A held `geodb-client pipe` child: one request line `<space>[,<space>...]\t<geoql>\n`, one frame
//! back, `+<n>\n<n bytes>` (the rows, tab-separated cells, one a line) or `-<n>\n<n bytes>` (a
//! refusal). The same protocol the api's card client speaks (`api/src/geodb/client.ts`) and the
//! framing `geodb-cursor` reads for `derived pipe`.
//!
//! Requests are serial (one child, under a mutex): this only feeds a sampled shadow. Every request is
//! bounded by the timeout; any failure or refusal drops the child, and no new one starts until the
//! cool-down passes, so a geo-db outage costs one failed spawn per cool-down, not one per lookup.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::{GeoDbReader, TwinError};

/// Most bytes of one answer frame read; a larger header is a broken child.
const FRAME_MAX: usize = 8 << 20;

#[derive(Clone, Debug)]
pub struct PipeConfig {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Bound on one request, and on the spawn handshake.
    pub timeout: Duration,
    /// After a failure, no respawn for this long.
    pub cooldown: Duration,
}

struct Pipe {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for Pipe {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[derive(Default)]
struct State {
    pipe: Option<Pipe>,
    failed_at: Option<Instant>,
}

pub struct PipeReader {
    cfg: PipeConfig,
    state: Mutex<State>,
}

impl PipeReader {
    pub fn new(cfg: PipeConfig) -> Self {
        Self {
            cfg,
            state: Mutex::new(State::default()),
        }
    }

    async fn spawn(&self) -> Result<Pipe, TwinError> {
        let mut child = Command::new(&self.cfg.program)
            .args(&self.cfg.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(TwinError::Spawn)?;
        let stdin = child.stdin.take().ok_or(TwinError::Closed)?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or(TwinError::Closed)?);
        let mut first = String::new();
        if stdout.read_line(&mut first).await? == 0 {
            return Err(TwinError::Closed);
        }
        if first.trim_end() != "ready" {
            return Err(TwinError::NotReady(first));
        }
        Ok(Pipe {
            child,
            stdin,
            stdout,
        })
    }
}

async fn request(p: &mut Pipe, line: &str) -> Result<String, TwinError> {
    p.stdin.write_all(line.as_bytes()).await?;
    p.stdin.flush().await?;
    let mut head = String::new();
    if p.stdout.read_line(&mut head).await? == 0 {
        return Err(TwinError::Closed);
    }
    let head = head.trim_end();
    let (sign, n) = head.split_at(head.len().min(1));
    let n: usize = n
        .parse()
        .map_err(|_| TwinError::BadFrame(head.to_string()))?;
    if n > FRAME_MAX {
        return Err(TwinError::BadFrame(head.to_string()));
    }
    let mut body = vec![0u8; n];
    p.stdout.read_exact(&mut body).await?;
    let body = String::from_utf8_lossy(&body).into_owned();
    match sign {
        "+" => Ok(body),
        "-" => Err(TwinError::Refused(body.chars().take(200).collect())),
        _ => Err(TwinError::BadFrame(head.to_string())),
    }
}

#[async_trait]
impl GeoDbReader for PipeReader {
    async fn query(&self, spaces: &[String], doc: &str) -> Result<String, TwinError> {
        if spaces.is_empty() || doc.contains(['\t', '\n']) {
            return Err(TwinError::BadRequest);
        }
        let line = format!("{}\t{}\n", spaces.join(","), doc);
        let mut st = self.state.lock().await;
        if st.pipe.is_none() {
            if st
                .failed_at
                .is_some_and(|t| t.elapsed() < self.cfg.cooldown)
            {
                return Err(TwinError::CoolingDown);
            }
            match tokio::time::timeout(self.cfg.timeout, self.spawn()).await {
                Ok(Ok(p)) => st.pipe = Some(p),
                Ok(Err(e)) => {
                    st.failed_at = Some(Instant::now());
                    return Err(e);
                }
                Err(_) => {
                    st.failed_at = Some(Instant::now());
                    return Err(TwinError::Timeout(self.cfg.timeout));
                }
            }
        }
        let pipe = st.pipe.as_mut().expect("set above");
        let res = match tokio::time::timeout(self.cfg.timeout, request(pipe, &line)).await {
            Ok(r) => r,
            Err(_) => Err(TwinError::Timeout(self.cfg.timeout)),
        };
        match res {
            Ok(body) => {
                st.failed_at = None;
                Ok(body)
            }
            Err(e) => {
                // A timed-out child still owes a reply, and a refusal ends the tool: drop it.
                st.pipe = None;
                st.failed_at = Some(Instant::now());
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(script: &str, cooldown: Duration) -> PipeReader {
        PipeReader::new(PipeConfig {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            timeout: Duration::from_millis(1500),
            cooldown,
        })
    }

    fn spaces() -> Vec<String> {
        vec!["a".repeat(32), "b".repeat(32)]
    }

    #[tokio::test]
    async fn sends_one_line_and_reads_one_frame() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("sent");
        // Echoes the request into the log, answers a 5-byte body.
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+5\\nab\\ncd'; done",
            log.display()
        );
        let r = fake(&script, Duration::ZERO);
        assert_eq!(r.query(&spaces(), ".{ x }").await.unwrap(), "ab\ncd");
        assert_eq!(r.query(&spaces(), ".{ y }").await.unwrap(), "ab\ncd");
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            sent,
            format!(
                "{a},{b}\t.{{ x }}\n{a},{b}\t.{{ y }}\n",
                a = "a".repeat(32),
                b = "b".repeat(32)
            )
        );
    }

    #[tokio::test]
    async fn a_refusal_is_an_error_then_the_child_cools_down() {
        let r = fake(
            "echo ready; read -r l; printf -- '-9\\nrefused!!'; exit 1",
            Duration::from_secs(60),
        );
        assert!(matches!(
            r.query(&spaces(), "d").await.unwrap_err(),
            TwinError::Refused(_)
        ));
        assert!(matches!(
            r.query(&spaces(), "d").await.unwrap_err(),
            TwinError::CoolingDown
        ));
    }

    #[tokio::test]
    async fn a_silent_child_times_out_and_a_missing_binary_does_not_panic() {
        let r = fake("echo ready; sleep 30", Duration::ZERO);
        assert!(matches!(
            r.query(&spaces(), "d").await.unwrap_err(),
            TwinError::Timeout(_)
        ));
        let r = PipeReader::new(PipeConfig {
            program: "/nonexistent/geodb-client".into(),
            args: vec![],
            timeout: Duration::from_secs(1),
            cooldown: Duration::ZERO,
        });
        assert!(matches!(
            r.query(&spaces(), "d").await.unwrap_err(),
            TwinError::Spawn(_)
        ));
        let r = fake("echo nope", Duration::ZERO);
        assert!(matches!(
            r.query(&spaces(), "d").await.unwrap_err(),
            TwinError::NotReady(_)
        ));
    }

    #[tokio::test]
    async fn a_line_breaking_document_is_refused_before_any_io() {
        let r = fake("exit 1", Duration::ZERO);
        assert!(matches!(
            r.query(&spaces(), "a\tb").await.unwrap_err(),
            TwinError::BadRequest
        ));
        assert!(matches!(
            r.query(&[], "d").await.unwrap_err(),
            TwinError::BadRequest
        ));
    }
}
