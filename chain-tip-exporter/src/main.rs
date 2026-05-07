//! Chain-tip exporter
//!
//! Polls an EVM JSON-RPC endpoint for the latest block and publishes
//! `chain_tip_block_number` and `chain_tip_block_number_timestamp` gauges.
//! Pairs with the `hermes_latest_processed_block{,_timestamp}` gauges from
//! hermes-pipeline / hermes-ipfs-cache so lag alerts can compute
//! `chain_tip_block_number - hermes_latest_processed_block`.
//!
//! ## Configuration
//!
//! - `RPC_URL` — EVM JSON-RPC endpoint (required)
//! - `POLL_INTERVAL_SECS` — polling interval (default: 30)
//! - `LATEST_BLOCK_BEHIND_THRESHOLD_SECS` — max latest-block age in seconds before emitting a warn
//! - `METRICS_PORT` — port for the Prometheus listener (default: 9464)
//! - `SENTRY_DSN`, `SENTRY_*`, `AXIOM_*` — telemetry (optional)

use std::env;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use hermes_instrumentation::{Backend, Config, info, warn};
use serde::Deserialize;

fn build_telemetry_config() -> Config {
    let backend = match env::var("SENTRY_DSN") {
        Ok(dsn) => {
            let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            let send_default_pii = env::var("SENTRY_SEND_DEFAULT_PII")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);
            let environment = env::var("SENTRY_ENVIRONMENT").ok();
            let release = env::var("SENTRY_RELEASE").ok();
            let debug = env::var("SENTRY_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            Backend::Sentry {
                dsn,
                traces_sample_rate,
                send_default_pii,
                environment,
                release,
                debug,
                axiom: hermes_instrumentation::AxiomConfig::from_env(),
            }
        }
        _ => Backend::Console,
    };

    Config::new("chain-tip-exporter", backend)
}

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring crypto provider");

    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    info!("Chain-tip exporter starting");

    let metrics_port: Option<u16> = env::var("METRICS_PORT").ok().and_then(|s| s.parse().ok());
    hermes_instrumentation::metrics::install("chain-tip-exporter", metrics_port)?;

    let rpc_url = env::var("RPC_URL").context("RPC_URL must be set")?;
    let poll_interval = Duration::from_secs(
        env::var("POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30),
    );
    let latest_block_behind_threshold = env::var("LATEST_BLOCK_BEHIND_THRESHOLD_SECS")
        .ok()
        .map(|s| {
            s.parse::<u64>()
                .context("LATEST_BLOCK_BEHIND_THRESHOLD_SECS must be seconds")
                .map(Duration::from_secs)
        })
        .transpose()?;
    info!(
        poll_interval_secs = poll_interval.as_secs(),
        latest_block_behind_threshold_secs = latest_block_behind_threshold.map(|d| d.as_secs()),
        "Configuration loaded"
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let mut ticker = tokio::time::interval(poll_interval);
    loop {
        ticker.tick().await;
        match fetch_latest_block(&client, &rpc_url).await {
            Ok((number, timestamp)) => {
                metrics::gauge!("chain_tip_block_number").set(number as f64);
                metrics::gauge!("chain_tip_block_number_timestamp").set(timestamp as f64);
                if let Some(threshold) = latest_block_behind_threshold {
                    alert_if_latest_block_is_stale(number, timestamp, threshold)?;
                }
                info!(
                    block_number = number,
                    block_timestamp = timestamp,
                    "Fetched chain tip"
                );
            }
            Err(e) => {
                // Don't reset gauges on failure — leave them at last known value
                // so dashboards can still show the last observed chain tip.
                warn!(error = %e, "Failed to fetch latest block");
            }
        }
    }
}

fn alert_if_latest_block_is_stale(
    block_number: u64,
    block_timestamp: i64,
    threshold: Duration,
) -> anyhow::Result<()> {
    let now = current_unix_timestamp()?;
    let block_age = latest_block_age_secs(now, block_timestamp);
    if block_age > threshold.as_secs() {
        let block_age_minutes = block_age / 60;
        warn!(
            block_number,
            block_timestamp,
            latest_block_age_secs = block_age,
            latest_block_age_minutes = block_age_minutes,
            threshold_secs = threshold.as_secs(),
            "RPC latest block is behind by {} minutes",
            block_age_minutes
        );
    }
    Ok(())
}

fn current_unix_timestamp() -> anyhow::Result<i64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?;
    Ok(now.as_secs() as i64)
}

fn latest_block_age_secs(now: i64, block_timestamp: i64) -> u64 {
    now.saturating_sub(block_timestamp).max(0) as u64
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize, Debug)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct Block {
    number: String,
    timestamp: String,
}

async fn fetch_latest_block(client: &reqwest::Client, url: &str) -> anyhow::Result<(u64, i64)> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_getBlockByNumber",
        "params": ["latest", false],
        "id": 1,
    });

    let resp: RpcResponse<Block> = client
        .post(url)
        .json(&body)
        .send()
        .await
        .context("RPC request failed")?
        .error_for_status()
        .context("RPC returned non-2xx")?
        .json()
        .await
        .context("RPC response not valid JSON")?;

    if let Some(err) = resp.error {
        anyhow::bail!("RPC error {}: {}", err.code, err.message);
    }

    let block = resp.result.context("missing result in RPC response")?;
    let number = u64::from_str_radix(block.number.trim_start_matches("0x"), 16)
        .context("parsing block number")?;
    let timestamp = i64::from_str_radix(block.timestamp.trim_start_matches("0x"), 16)
        .context("parsing block timestamp")?;
    Ok((number, timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_block_response() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "number": "0x14d4d",
                "timestamp": "0x67e8a3f0"
            }
        });
        let resp: RpcResponse<Block> = serde_json::from_value(body).unwrap();
        let block = resp.result.unwrap();
        assert_eq!(
            u64::from_str_radix(block.number.trim_start_matches("0x"), 16).unwrap(),
            85325
        );
        assert_eq!(
            i64::from_str_radix(block.timestamp.trim_start_matches("0x"), 16).unwrap(),
            1_743_299_568
        );
    }

    #[test]
    fn surfaces_rpc_errors() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32600, "message": "Invalid Request" }
        });
        let resp: RpcResponse<Block> = serde_json::from_value(body).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid Request");
    }

    #[test]
    fn latest_block_age_is_zero_for_future_timestamps() {
        assert_eq!(latest_block_age_secs(1_000, 1_005), 0);
    }

    #[test]
    fn latest_block_age_uses_unix_seconds() {
        assert_eq!(latest_block_age_secs(1_300, 1_000), 300);
    }
}
