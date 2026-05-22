//! Prometheus metrics for the Hermes ecosystem.
//!
//! Exposes a small public API for installing a Prometheus recorder and HTTP
//! `/metrics` listener, plus typed setters for the gauges that hermes services
//! emit. Service identification (e.g. `hermes-pipeline` vs `hermes-ipfs-cache`)
//! is provided by the Prometheus `ServiceMonitor` target label, so the gauges
//! themselves carry no labels.
//!
//! # Usage
//!
//! ```rust,no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! // Must be called from inside a Tokio runtime — the HTTP listener is spawned
//! // onto the current runtime. Pass `None` to bind on the default port (9464),
//! // or `Some(port)` to override (e.g. from a `METRICS_PORT` env var).
//! hermes_instrumentation::metrics::install("hermes-pipeline", None)?;
//!
//! hermes_instrumentation::metrics::set_latest_processed_block(123);
//! hermes_instrumentation::metrics::set_latest_processed_block_timestamp(1_714_000_000);
//! # Ok(()) }
//! ```

use std::net::SocketAddr;

use metrics_exporter_prometheus::{BuildError, PrometheusBuilder};

pub const DEFAULT_PORT: u16 = 9464;

const LATEST_PROCESSED_BLOCK: &str = "hermes_latest_processed_block";
const LATEST_PROCESSED_BLOCK_TIMESTAMP: &str = "hermes_latest_processed_block_timestamp";

/// Errors returned from [`install`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The exporter failed to bind its HTTP listener or register the recorder.
    /// Wraps the underlying [`BuildError`] from `metrics-exporter-prometheus`,
    /// which covers double-install (`set_global_recorder` returns `Err` rather
    /// than panicking) as well as listener bind failures.
    #[error("failed to install Prometheus metrics exporter: {0}")]
    Builder(#[from] BuildError),
}

/// Install the Prometheus recorder and start an HTTP `/metrics` listener.
///
/// Must be called from inside a Tokio runtime — the listener is spawned via
/// `Handle::current()`. Calling outside a runtime will panic.
///
/// `port: None` binds on [`DEFAULT_PORT`]; pass `Some(port)` to override
/// (e.g. from a `METRICS_PORT` env var).
///
/// `service` is informational (logged at startup); the metric labels come from
/// the `ServiceMonitor` target, not the metric itself.
///
/// Calling more than once per process returns `Err` rather than panicking.
pub fn install(service: &str, port: Option<u16>) -> Result<(), Error> {
    let port = port.unwrap_or(DEFAULT_PORT);
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()?;
    tracing::info!(
        service.name = service,
        port = port,
        "Prometheus metrics listener installed"
    );
    Ok(())
}

/// Update the `hermes_latest_processed_block` gauge.
///
/// Call this each time a block has been fully processed end-to-end — block
/// summary emitted to Kafka, cursor durably persisted, or the block was
/// observed with no work to do (e.g. an empty IPFS-cache block). The
/// chain-tip distance against `chain_tip_block_number` is what drives the
/// lag alerts, so the gauge must advance on every block we successfully
/// observed, otherwise quiet stretches will look like indexer lag.
pub fn set_latest_processed_block(block: u64) {
    metrics::gauge!(LATEST_PROCESSED_BLOCK).set(block as f64);
}

/// Update the `hermes_latest_processed_block_timestamp` gauge (Unix seconds).
///
/// This is the *block's* clock time (`clock.timestamp.seconds` from the
/// substream), not the time the gauge was set. Lag alerts compute
/// `time() - <gauge>` to produce a "seconds behind real-time" signal that
/// doesn't depend on a chain-tip exporter being healthy.
pub fn set_latest_processed_block_timestamp(unix_seconds: i64) {
    metrics::gauge!(LATEST_PROCESSED_BLOCK_TIMESTAMP).set(unix_seconds as f64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    /// Build a local recorder, run the closure with it scoped, and return the
    /// rendered Prometheus text. Avoids touching the global recorder so tests
    /// can run in parallel without `serial_test`.
    fn render_with(f: impl FnOnce()) -> String {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, f);
        handle.render()
    }

    #[test]
    fn set_latest_processed_block_emits_gauge() {
        let rendered = render_with(|| set_latest_processed_block(42));
        assert!(
            rendered.contains("hermes_latest_processed_block 42"),
            "expected gauge in output, got:\n{rendered}"
        );
    }

    #[test]
    fn set_latest_processed_block_timestamp_emits_gauge() {
        let rendered = render_with(|| set_latest_processed_block_timestamp(1_714_000_000));
        assert!(
            rendered.contains("hermes_latest_processed_block_timestamp 1714000000"),
            "expected timestamp gauge in output, got:\n{rendered}"
        );
    }

    #[test]
    fn gauge_replaces_previous_value() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            set_latest_processed_block(42);
            set_latest_processed_block(7);
        });
        let rendered = handle.render();
        assert!(
            rendered.contains("hermes_latest_processed_block 7"),
            "expected replaced value, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("hermes_latest_processed_block 42"),
            "old value should not be present, got:\n{rendered}"
        );
    }
}
