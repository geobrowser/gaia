//! Delivery worker entry point.
//!
//! Polls the notification_deliveries table for pending rows and delivers
//! them to registered webhooks with HMAC-SHA256 signatures.

use std::env;

use hermes_instrumentation::{debug, error, info};

use delivery_worker::deliver::{
    backoff_seconds, deliver_webhook, is_success, should_retry, MAX_RETRIES,
};
use delivery_worker::error::WorkerError;
use delivery_worker::storage::Storage;

fn main() -> Result<(), WorkerError> {
    dotenv::dotenv().ok();

    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    info!("Starting delivery-worker");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| WorkerError::Config(format!("Failed to build tokio runtime: {}", e)))?
        .block_on(async_main())
}

/// Build telemetry configuration from environment variables.
fn build_telemetry_config() -> hermes_instrumentation::Config {
    use hermes_instrumentation::{Backend, Config};

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
            let debug_mode = env::var("SENTRY_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            println!(
                "Telemetry: Sentry (env: {}, release: {}, debug: {})",
                environment.as_deref().unwrap_or("none"),
                release.as_deref().unwrap_or("none"),
                if debug_mode { "yes" } else { "no" }
            );

            Backend::Sentry {
                dsn,
                traces_sample_rate,
                send_default_pii,
                environment,
                release,
                debug: debug_mode,
                axiom: hermes_instrumentation::AxiomConfig::from_env(),
            }
        }
        _ => {
            println!("Telemetry: Console (set SENTRY_DSN to enable Sentry)");
            Backend::Console
        }
    };

    Config::new("delivery-worker", backend)
}

async fn async_main() -> Result<(), WorkerError> {
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| WorkerError::Config("DATABASE_URL not set".into()))?;

    let poll_interval_ms: u64 = env::var("POLL_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);

    let max_retries: i32 = env::var("MAX_RETRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(MAX_RETRIES);

    let batch_size: i64 = env::var("BATCH_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    // Initialize storage and HTTP client
    let storage = Storage::connect(&database_url).await?;
    info!("Connected to database");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| WorkerError::Config(format!("Failed to build HTTP client: {}", e)))?;

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    let mut delivered_count: u64 = 0;
    let mut failed_count: u64 = 0;

    // Heartbeat: log stats every 60 seconds so operators can see the service is alive
    let heartbeat_interval_secs: u64 = env::var("HEARTBEAT_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let mut heartbeat_timer =
        tokio::time::interval(tokio::time::Duration::from_secs(heartbeat_interval_secs));
    heartbeat_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    info!(
        poll_interval_ms = poll_interval_ms,
        max_retries = max_retries,
        batch_size = batch_size,
        "Starting delivery loop"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down...");
                break;
            }
            _ = heartbeat_timer.tick() => {
                info!(
                    delivered = delivered_count,
                    failed = failed_count,
                    "Heartbeat"
                );
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(poll_interval_ms)) => {
                match storage.fetch_pending(batch_size).await {
                    Ok(deliveries) => {
                        if deliveries.is_empty() {
                            debug!("No pending deliveries");
                            continue;
                        }

                        info!(count = deliveries.len(), "Processing pending deliveries");

                        for delivery in deliveries {
                            let payload_bytes = serde_json::to_vec(&delivery.payload)
                                .unwrap_or_default();

                            let result = deliver_webhook(
                                &client,
                                &delivery.webhook_url,
                                &delivery.webhook_secret,
                                &payload_bytes,
                            )
                            .await;

                            match result {
                                Ok(status) if is_success(status) => {
                                    if let Err(e) = storage.mark_delivered(delivery.delivery_id).await {
                                        error!(error = %e, "Failed to mark delivery as delivered");
                                    } else {
                                        delivered_count += 1;
                                        debug!(
                                            delivery_id = %delivery.delivery_id,
                                            status = status,
                                            "Delivery successful"
                                        );
                                    }
                                }
                                Ok(status) => {
                                    let attempt = delivery.attempts as i32 + 1;
                                    let error_msg = format!("HTTP {}", status);

                                    if attempt >= max_retries || !should_retry(status) {
                                        if let Err(e) = storage.mark_failed(delivery.delivery_id, &error_msg).await {
                                            error!(error = %e, "Failed to mark delivery as failed");
                                        } else {
                                            failed_count += 1;
                                            error!(
                                                delivery_id = %delivery.delivery_id,
                                                status = status,
                                                attempts = attempt,
                                                "Delivery permanently failed"
                                            );
                                        }
                                    } else {
                                        let backoff = backoff_seconds(attempt);
                                        if let Err(e) = storage.mark_retry(
                                            delivery.delivery_id,
                                            backoff,
                                            &error_msg,
                                        ).await {
                                            error!(error = %e, "Failed to update retry");
                                        } else {
                                            debug!(
                                                delivery_id = %delivery.delivery_id,
                                                attempt = attempt,
                                                next_retry_secs = backoff,
                                                "Delivery scheduled for retry"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    let attempt = delivery.attempts as i32 + 1;
                                    let error_msg = e.to_string();

                                    if attempt >= max_retries {
                                        if let Err(e) = storage.mark_failed(delivery.delivery_id, &error_msg).await {
                                            error!(error = %e, "Failed to mark delivery as failed");
                                        } else {
                                            failed_count += 1;
                                            error!(
                                                delivery_id = %delivery.delivery_id,
                                                attempts = attempt,
                                                "Delivery permanently failed (network error)"
                                            );
                                        }
                                    } else {
                                        let backoff = backoff_seconds(attempt);
                                        if let Err(e) = storage.mark_retry(
                                            delivery.delivery_id,
                                            backoff,
                                            &error_msg,
                                        ).await {
                                            error!(error = %e, "Failed to update retry");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to fetch pending deliveries");
                    }
                }
            }
        }
    }

    info!(
        delivered = delivered_count,
        failed = failed_count,
        "Shutdown complete"
    );

    Ok(())
}
