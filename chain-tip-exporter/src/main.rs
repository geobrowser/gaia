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
//! - `METRICS_PORT` — port for the Prometheus listener (default: 9464)
//! - `SENTRY_DSN`, `SENTRY_*`, `AXIOM_*` — telemetry (optional)

use std::env;
use std::time::Duration;

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
    info!(
        rpc_url = %rpc_url,
        poll_interval_secs = poll_interval.as_secs(),
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
                info!(
                    block_number = number,
                    block_timestamp = timestamp,
                    "Fetched chain tip"
                );
            }
            Err(e) => {
                // Don't reset gauges on failure — leave them at last known value
                // so `time() - chain_tip_block_number_timestamp` correctly fires
                // a stale-tip alert via the existing PrometheusRule.
                warn!(error = %e, "Failed to fetch latest block");
            }
        }
    }
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
}
