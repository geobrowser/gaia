//! Metrics for the CronJob binaries: push the registry to a Pushgateway once, at exit.
//!
//! The long-running `ranking-indexer` serves `/metrics` and is scraped. A CronJob pod lives
//! for seconds and nothing scrapes Job pods, so its counters (the `*_RUST_COMPUTE` shadow
//! counters, `ranking_indexer_pg_writes_skipped_total`) were no-ops. When `PUSHGATEWAY_URL`
//! is set, [`JobMetrics::install`] installs a recorder and [`JobMetrics::push_at_exit`] sends
//! what it holds in one `PUT /metrics/job/<job>`. Unset, nothing is installed and every
//! counter stays the no-op it was.
//!
//! A push never fails the job: a refused connection or a slow gateway is logged and dropped.
//! `PUT` replaces the job's group, so the gateway holds the last run's counts, not a running
//! total; read it as "what the last run saw".

use std::time::Duration;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Environment variable naming the Pushgateway base URL, e.g. `http://pushgateway.monitoring:9091`.
pub const PUSHGATEWAY_URL_ENV: &str = "PUSHGATEWAY_URL";

const PUSH_TIMEOUT: Duration = Duration::from_secs(5);

pub struct JobMetrics {
    job: &'static str,
    target: Option<(String, PrometheusHandle)>,
}

/// `<base>/metrics/job/<job>`, tolerating a trailing slash on the base.
pub fn push_url(base: &str, job: &str) -> String {
    format!("{}/metrics/job/{}", base.trim_end_matches('/'), job)
}

/// `PUT` an exposition-format body to `url`. The error is a plain string for logging.
pub async fn push_rendered(url: &str, body: String) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(PUSH_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .put(url)
        .header("content-type", "text/plain; version=0.0.4")
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("pushgateway answered {}", resp.status()))
    }
}

impl JobMetrics {
    /// Install the recorder and register every series at zero when `PUSHGATEWAY_URL` is set
    /// and non-empty; otherwise a no-op handle.
    pub fn install(job: &'static str) -> Self {
        let url = std::env::var(PUSHGATEWAY_URL_ENV)
            .ok()
            .filter(|u| !u.trim().is_empty());
        let Some(base) = url else {
            return Self { job, target: None };
        };
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        if let Err(e) = metrics::set_global_recorder(recorder) {
            tracing::warn!(job, error = %e, "metrics recorder already installed; not pushing");
            return Self { job, target: None };
        }
        crate::metrics::register();
        Self {
            job,
            target: Some((push_url(&base, job), handle)),
        }
    }

    /// Build with an explicit recorder handle and URL, without touching the global recorder.
    pub fn with_handle(job: &'static str, url: String, handle: PrometheusHandle) -> Self {
        Self {
            job,
            target: Some((url, handle)),
        }
    }

    /// Push once. Logs and returns on any failure.
    pub async fn push_at_exit(self) {
        let Some((url, handle)) = self.target else {
            return;
        };
        match push_rendered(&url, handle.render()).await {
            Ok(()) => tracing::info!(job = self.job, "pushed job metrics"),
            Err(e) => tracing::warn!(job = self.job, error = %e, "job metrics push failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{rust_compute_shadow, ComputeFlag, ShadowOutcome};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One-shot HTTP server: answers 200 and returns the raw request it read.
    async fn capture_one(status: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= want {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            sock.write_all(
                format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
            String::from_utf8_lossy(&buf).to_string()
        });
        (base, task)
    }

    #[test]
    fn push_url_trims_trailing_slash() {
        assert_eq!(
            push_url("http://pg:9091/", "j"),
            "http://pg:9091/metrics/job/j"
        );
        assert_eq!(
            push_url("http://pg:9091", "j"),
            "http://pg:9091/metrics/job/j"
        );
    }

    #[tokio::test]
    async fn a_bins_shadow_counters_reach_the_gateway() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            crate::metrics::register();
            rust_compute_shadow(ComputeFlag::DebateWatch, ShadowOutcome::Mismatch);
        });
        let (base, server) = capture_one("200 OK").await;
        JobMetrics::with_handle(
            "ranking-indexer-feed-signals-sync",
            push_url(&base, "ranking-indexer-feed-signals-sync"),
            handle,
        )
        .push_at_exit()
        .await;
        let req = server.await.unwrap();
        assert!(
            req.starts_with("PUT /metrics/job/ranking-indexer-feed-signals-sync HTTP/1.1"),
            "request line: {req}"
        );
        assert!(
            req.contains("ranking_indexer_rust_compute_shadow_total{flag=\"debate_watch\",outcome=\"mismatch\"} 1"),
            "body: {req}"
        );
        assert!(
            req.contains("ranking_indexer_rust_compute_shadow_total{flag=\"debate_watch\",outcome=\"match\"} 0"),
            "zero series missing: {req}"
        );
    }

    #[tokio::test]
    async fn push_rendered_reports_a_refusing_gateway() {
        let (base, server) = capture_one("500 Internal Server Error").await;
        let err = push_rendered(&push_url(&base, "j"), "x 1\n".into())
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(err.contains("500"), "{err}");
    }

    #[tokio::test]
    async fn an_unreachable_gateway_does_not_panic() {
        let recorder = PrometheusBuilder::new().build_recorder();
        // Port 1 on loopback refuses at once.
        JobMetrics::with_handle("j", push_url("http://127.0.0.1:1", "j"), recorder.handle())
            .push_at_exit()
            .await;
    }

    #[tokio::test]
    async fn no_url_means_no_push() {
        // `install` reads the environment; with it unset the handle is a no-op.
        let m = JobMetrics {
            job: "j",
            target: None,
        };
        m.push_at_exit().await;
    }

    /// Every bin that records a shadow or pg-write counter must install the push and call it
    /// at exit, or its counters are lost again with the pod.
    #[test]
    fn every_counting_bin_pushes_at_exit() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let src = std::fs::read_to_string(&path).unwrap();
            if !(src.contains("rust_compute_shadow") || src.contains("PgWriteSite")) {
                continue;
            }
            checked += 1;
            assert!(
                src.contains("JobMetrics::install(") && src.contains(".push_at_exit()"),
                "{} records counters but does not push them at exit",
                path.display()
            );
        }
        assert!(checked >= 11, "scan found only {checked} bins");
    }
}
