//! Delivery worker entry point.
//!
//! Polls the notification_deliveries table for pending rows and delivers
//! them to registered webhooks with HMAC-SHA256 signatures.
//! Deliveries within each batch are processed concurrently via `JoinSet`.

use std::env;
use std::sync::Arc;

use hermes_instrumentation::{debug, error, info, instrument, warn};
use tokio::task::JoinSet;

use delivery_worker::deliver::{
    backoff_seconds, deliver_webhook, is_success, should_retry, MAX_RETRIES,
};
use delivery_worker::error::WorkerError;
use delivery_worker::geodb_claim::{self, ClaimSource, Decision, GeoDbClaimer, Settled};
use delivery_worker::metrics::{self, Outcome};
use delivery_worker::shadow::{self, Shadow};
use delivery_worker::storage::Storage;
use geodb_outbox::reader::{Claimed, OutboxReader};

/// Outcome of a single delivery attempt, returned from spawned tasks.
enum DeliveryOutcome {
    Delivered,
    Failed,
    Retried,
    Error,
}

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
    let database_url =
        env::var("DATABASE_URL").map_err(|_| WorkerError::Config("DATABASE_URL not set".into()))?;

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

    let webhook_timeout_secs: u64 = env::var("WEBHOOK_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    let max_concurrent: usize = env::var("MAX_CONCURRENT_DELIVERIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let health_port: u16 = env::var("HEALTH_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    // Prometheus /metrics on 9464 (hermes_instrumentation::metrics::DEFAULT_PORT),
    // the port the ServiceMonitor scrapes; override with METRICS_PORT for local runs.
    // Separate from the health server on HEALTH_PORT, matching the other indexers.
    let metrics_port: Option<u16> = env::var("METRICS_PORT").ok().and_then(|s| s.parse().ok());
    hermes_instrumentation::metrics::install("delivery-worker", metrics_port)
        .map_err(|e| WorkerError::Config(format!("metrics install failed: {}", e)))?;
    metrics::register();

    // Initialize storage (wrapped in Arc for concurrent task access)
    let storage = Arc::new(Storage::connect(&database_url).await?);
    info!("Connected to database");

    // Start health check server
    let _health_handle =
        delivery_worker::health::start_health_server(storage.pool().clone(), health_port);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(webhook_timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| WorkerError::Config(format!("Failed to build HTTP client: {}", e)))?;

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    let _signal_handle = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Off unless DELIVERY_WORKER_CLAIM_SOURCE=geodb: claim from geo-db's work class instead of
    // Postgres. A requested but unconfigured geo-db source stops the worker rather than quietly
    // keeping the queue where the operator did not ask for it.
    let geodb_claimer: Option<Arc<GeoDbClaimer<OutboxReader>>> =
        match geodb_claim::claim_source_from(env::var(geodb_claim::ENV_VAR).ok().as_deref()) {
            ClaimSource::Postgres => None,
            ClaimSource::GeoDb => {
                geodb_claim::require_webhooks_file(
                    ClaimSource::GeoDb,
                    storage.uses_webhooks_file(),
                )
                .map_err(|m| WorkerError::Config(m.into()))?;
                let cfg = geodb_claim::config_from(|k| env::var(k).ok()).ok_or_else(|| {
                WorkerError::Config(
                    "DELIVERY_WORKER_CLAIM_SOURCE=geodb needs GEODB_OUTBOX_KEY_PATH and GEODB_BINDING_PATH"
                        .into(),
                )
            })?;
                let lease = std::time::Duration::from_secs(geodb_claim::lease_secs_from(
                    env::var(geodb_claim::LEASE_ENV_VAR).ok().as_deref(),
                ));
                geodb_claim::register();
                warn!(
                    lease_secs = lease.as_secs(),
                    "Claiming deliveries from geo-db, not Postgres"
                );
                Some(Arc::new(GeoDbClaimer::from_config(cfg, lease)))
            }
        };

    // Off unless GEODB_OUTBOX_SHADOW_READ is on: compares each terminal delivery with geo-db's
    // mirrored unit. Measurement only; it never delivers and never touches Postgres. It would
    // complete the units the claimer is delivering, so it is off whenever the claimer is on.
    let shadow: Option<Shadow> = if geodb_claimer.is_some() {
        None
    } else {
        shadow::start()
    };
    if shadow.is_some() {
        info!("geo-db outbox shadow reader on");
    }

    let mut delivered_count: u64 = 0;
    let mut failed_count: u64 = 0;

    // Heartbeat: log stats periodically so operators can see the service is alive
    let heartbeat_interval_secs: u64 = env::var("HEARTBEAT_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let mut heartbeat_timer =
        tokio::time::interval(tokio::time::Duration::from_secs(heartbeat_interval_secs));
    heartbeat_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Stale claim reaper: reset in_progress deliveries that a crashed worker left behind
    let mut reaper_timer = tokio::time::interval(tokio::time::Duration::from_secs(60));
    reaper_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // JoinSet for concurrent delivery tasks — persists across loop iterations
    // so in-flight tasks can be drained on shutdown.
    let mut join_set: JoinSet<DeliveryOutcome> = JoinSet::new();

    info!(
        poll_interval_ms = poll_interval_ms,
        max_retries = max_retries,
        batch_size = batch_size,
        max_concurrent = max_concurrent,
        "Starting delivery loop"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down, draining in-flight deliveries...");
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    drain_join_set(&mut join_set, &mut delivered_count, &mut failed_count),
                ).await {
                    Ok(()) => info!("All in-flight deliveries drained"),
                    Err(_) => warn!("Timed out draining deliveries after 10s"),
                }
                break;
            }
            _ = heartbeat_timer.tick() => {
                let pending_count = sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM notification_deliveries WHERE status = 'pending'"
                )
                .fetch_one(storage.pool())
                .await
                .inspect(|n| metrics::set_pending(*n))
                .unwrap_or(-1);

                let in_progress_count = sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM notification_deliveries WHERE status = 'in_progress'"
                )
                .fetch_one(storage.pool())
                .await
                .inspect(|n| metrics::set_in_progress(*n))
                .unwrap_or(-1);

                let pool_size = storage.pool().size();
                let pool_idle = storage.pool().num_idle();

                info!(
                    delivered = delivered_count,
                    failed = failed_count,
                    pending = pending_count,
                    in_progress = in_progress_count,
                    pool_size = pool_size,
                    pool_idle = pool_idle,
                    "Heartbeat"
                );
            }
            // No reaper with a geo-db source: an expired lease is claimed again by the next claim.
            _ = reaper_timer.tick(), if geodb_claimer.is_none() => {
                match storage.reset_stale_claims(300).await {
                    Ok(count) if count > 0 => {
                        warn!(reset = count, "Reset stale in_progress deliveries to pending");
                        metrics::stale_claims_reset(count);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        error!(error = %e, "Failed to reset stale claims");
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(poll_interval_ms)) => {
                // Drain completed tasks from the previous cycle
                drain_completed(&mut join_set, &mut delivered_count, &mut failed_count);

                if let Some(claimer) = &geodb_claimer {
                    let limit = u16::try_from(batch_size).unwrap_or(u16::MAX);
                    match claimer.claim(limit).await {
                        Ok(units) => {
                            if !units.is_empty() {
                                info!(count = units.len(), "Processing geo-db claimed deliveries");
                            }
                            for unit in units {
                                while join_set.len() >= max_concurrent {
                                    if let Some(result) = join_set.join_next().await {
                                        collect_outcome(result, &mut delivered_count, &mut failed_count);
                                    }
                                }
                                let client = client.clone();
                                let storage = Arc::clone(&storage);
                                let claimer = Arc::clone(claimer);
                                join_set.spawn(async move {
                                    process_geodb_unit(&client, &storage, &claimer, unit, max_retries).await
                                });
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to claim deliveries from geo-db");
                            metrics::claim_error();
                        }
                    }
                    continue;
                }

                match storage.claim_pending(batch_size).await {
                    Ok(deliveries) => {
                        if deliveries.is_empty() {
                            debug!("No pending deliveries");
                            continue;
                        }

                        info!(count = deliveries.len(), "Processing pending deliveries");

                        for delivery in deliveries {
                            // Wait if we've hit max concurrency
                            while join_set.len() >= max_concurrent {
                                if let Some(result) = join_set.join_next().await {
                                    collect_outcome(result, &mut delivered_count, &mut failed_count);
                                }
                            }

                            let client = client.clone();
                            let storage = Arc::clone(&storage);
                            let shadow = shadow.clone();

                            join_set.spawn(async move {
                                process_delivery(&client, &storage, delivery, max_retries, shadow.as_ref()).await
                            });
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to claim pending deliveries");
                        metrics::claim_error();
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

/// Deliver one unit claimed from geo-db and settle it over the pipe. The same signing, status
/// handling and retry policy as [`deliver_one`]; only the bookkeeping differs.
async fn process_geodb_unit(
    client: &reqwest::Client,
    storage: &Storage,
    claimer: &GeoDbClaimer<OutboxReader>,
    unit: Claimed,
    max_retries: i32,
) -> DeliveryOutcome {
    let webhook_id = uuid::Uuid::from_bytes(unit.webhook());
    // The unit's payload is the outbox row's JSON text; a payload that is not JSON can never be
    // delivered, as a serialization error does not on the Postgres path.
    let payload: Option<Vec<u8>> = serde_json::from_slice::<serde_json::Value>(&unit.payload)
        .ok()
        .and_then(|v| serde_json::to_vec(&v).ok());
    let target = match storage.webhook_target(webhook_id).await {
        Ok(t) => t,
        Err(e) => {
            // The webhook source is unreadable: leave the unit leased; it comes back.
            error!(error = %e, %webhook_id, "Could not resolve webhook; unit left leased");
            return DeliveryOutcome::Error;
        }
    };
    let (decision, outcome) = match (payload, target) {
        (None, _) => {
            error!(%webhook_id, "Claimed unit has no JSON payload, failing it");
            (Decision::Fail, DeliveryOutcome::Failed)
        }
        (_, None) => {
            error!(%webhook_id, "Webhook not found, failing the unit");
            (Decision::Fail, DeliveryOutcome::Failed)
        }
        (Some(bytes), Some((url, secret))) => {
            let result = deliver_webhook(client, &url, &secret, &bytes).await;
            // last_error is not stored by the work class (plan 10.16 W6); this line is its home.
            match &result {
                Ok(status) if !is_success(*status) => {
                    warn!(%webhook_id, status, attempts = unit.attempts, "Webhook answered non-success")
                }
                Err(e) => {
                    warn!(%webhook_id, error = %e, attempts = unit.attempts, "Webhook request failed")
                }
                _ => {}
            }
            let d = geodb_claim::decide(unit.attempts, max_retries, &result.map_err(|_| ()));
            let o = match d {
                Decision::Delivered => DeliveryOutcome::Delivered,
                Decision::Retry { .. } => DeliveryOutcome::Retried,
                Decision::Fail => DeliveryOutcome::Failed,
            };
            (d, o)
        }
    };
    match claimer.settle(&unit, decision).await {
        Settled::Acked => outcome,
        Settled::LeaseLost => {
            warn!(%webhook_id, "Lease lost before the result was recorded; another claim owns the unit");
            DeliveryOutcome::Error
        }
        Settled::PipeError => {
            error!(%webhook_id, "Could not record the result over the pipe; unit left leased");
            DeliveryOutcome::Error
        }
    }
}

/// Process a single delivery: serialize, POST, update DB status.
#[instrument(
    name = "delivery_worker.process_delivery",
    skip(client, storage, delivery, max_retries, shadow),
    fields(
        delivery_id = %delivery.delivery_id,
        webhook_url = %delivery.webhook_url,
    )
)]
async fn process_delivery(
    client: &reqwest::Client,
    storage: &Storage,
    delivery: delivery_worker::storage::PendingDelivery,
    max_retries: i32,
    shadow: Option<&Shadow>,
) -> DeliveryOutcome {
    let outcome = deliver_one(client, storage, &delivery, max_retries).await;
    // A terminal row is what the shadow compares; a retry is not terminal. try_send only.
    if let (Some(shadow), DeliveryOutcome::Delivered | DeliveryOutcome::Failed) = (shadow, &outcome)
    {
        shadow.observe(
            &delivery.idempotency_key,
            delivery.webhook_id,
            &delivery.payload,
        );
    }
    outcome
}

async fn deliver_one(
    client: &reqwest::Client,
    storage: &Storage,
    delivery: &delivery_worker::storage::PendingDelivery,
    max_retries: i32,
) -> DeliveryOutcome {
    let payload_bytes = match serde_json::to_vec(&delivery.payload) {
        Ok(bytes) => bytes,
        Err(e) => {
            let error_msg = format!("serialization error: {}", e);
            error!(
                delivery_id = %delivery.delivery_id,
                error = %e,
                "Failed to serialize payload, marking as failed"
            );
            if let Err(e) = storage.mark_failed(delivery.delivery_id, &error_msg).await {
                error!(error = %e, "Failed to mark delivery as failed");
                return DeliveryOutcome::Error;
            }
            return DeliveryOutcome::Failed;
        }
    };

    let result = deliver_webhook(
        client,
        &delivery.webhook_url,
        &delivery.webhook_secret,
        &payload_bytes,
    )
    .await;

    match result {
        Ok(status) if is_success(status) => {
            if let Err(e) = storage.mark_delivered(delivery.delivery_id).await {
                error!(error = %e, "Failed to mark delivery as delivered");
                return DeliveryOutcome::Error;
            }
            debug!(
                delivery_id = %delivery.delivery_id,
                status = status,
                "Delivery successful"
            );
            DeliveryOutcome::Delivered
        }
        Ok(status) => {
            let attempt = delivery.attempts as i32 + 1;
            let error_msg = format!("HTTP {}", status);

            if attempt >= max_retries || !should_retry(status) {
                if let Err(e) = storage.mark_failed(delivery.delivery_id, &error_msg).await {
                    error!(error = %e, "Failed to mark delivery as failed");
                    return DeliveryOutcome::Error;
                }
                error!(
                    delivery_id = %delivery.delivery_id,
                    status = status,
                    attempts = attempt,
                    "Delivery permanently failed"
                );
                DeliveryOutcome::Failed
            } else {
                let backoff = backoff_seconds(attempt);
                if let Err(e) = storage
                    .mark_retry(delivery.delivery_id, backoff, &error_msg)
                    .await
                {
                    error!(error = %e, "Failed to update retry");
                    return DeliveryOutcome::Error;
                }
                debug!(
                    delivery_id = %delivery.delivery_id,
                    attempt = attempt,
                    next_retry_secs = backoff,
                    "Delivery scheduled for retry"
                );
                DeliveryOutcome::Retried
            }
        }
        Err(e) => {
            let attempt = delivery.attempts as i32 + 1;
            let error_msg = e.to_string();

            if attempt >= max_retries {
                if let Err(e) = storage.mark_failed(delivery.delivery_id, &error_msg).await {
                    error!(error = %e, "Failed to mark delivery as failed");
                    return DeliveryOutcome::Error;
                }
                error!(
                    delivery_id = %delivery.delivery_id,
                    attempts = attempt,
                    "Delivery permanently failed (network error)"
                );
                DeliveryOutcome::Failed
            } else {
                let backoff = backoff_seconds(attempt);
                if let Err(e) = storage
                    .mark_retry(delivery.delivery_id, backoff, &error_msg)
                    .await
                {
                    error!(error = %e, "Failed to update retry");
                    return DeliveryOutcome::Error;
                }
                DeliveryOutcome::Retried
            }
        }
    }
}

/// Collect the outcome from a completed JoinSet task and update counters.
fn collect_outcome(
    result: Result<DeliveryOutcome, tokio::task::JoinError>,
    delivered_count: &mut u64,
    failed_count: &mut u64,
) {
    match result {
        Ok(DeliveryOutcome::Delivered) => {
            *delivered_count += 1;
            metrics::delivery(Outcome::Delivered);
        }
        Ok(DeliveryOutcome::Failed) => {
            *failed_count += 1;
            metrics::delivery(Outcome::Failed);
        }
        Ok(DeliveryOutcome::Retried) => metrics::delivery(Outcome::Retried),
        Ok(DeliveryOutcome::Error) => metrics::delivery(Outcome::Error),
        Err(e) => {
            error!(error = %e, "Delivery task panicked");
            metrics::delivery(Outcome::Error);
        }
    }
}

/// Drain all completed tasks without blocking.
fn drain_completed(
    join_set: &mut JoinSet<DeliveryOutcome>,
    delivered_count: &mut u64,
    failed_count: &mut u64,
) {
    while let Some(result) = join_set.try_join_next() {
        collect_outcome(result, delivered_count, failed_count);
    }
}

/// Drain all tasks (blocking) — used during shutdown.
async fn drain_join_set(
    join_set: &mut JoinSet<DeliveryOutcome>,
    delivered_count: &mut u64,
    failed_count: &mut u64,
) {
    while let Some(result) = join_set.join_next().await {
        collect_outcome(result, delivered_count, failed_count);
    }
}
