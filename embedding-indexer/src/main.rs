use std::time::Duration;

use clap::Parser;
use embedding_indexer::health::{self, Health};
use embedding_indexer::{Config, Engine, IndexerError};
use tracing::{error, info, warn};

fn init_tracing() {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    if std::env::var("LOG_FORMAT").is_ok_and(|v| v == "json") {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
    info!("shutdown signal received");
}

/// Start with backoff: OpenSearch or the service may still be coming up. A fatal error exits.
async fn start_with_retry(cfg: &Config, health: &Health) -> Engine {
    let mut delay = Duration::from_secs(1);
    loop {
        match Engine::start(cfg.clone()).await {
            Ok(engine) => return engine,
            Err(e @ IndexerError::Fatal(_)) => {
                error!(error = %e, "cannot start");
                std::process::exit(1);
            }
            Err(e) => {
                health.set_degraded(true);
                warn!(error = %e, retry_in_s = delay.as_secs(), "start failed; retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cfg = Config::parse();
    let health = Health::default();
    let _health_server = health::spawn(health.clone(), cfg.health_port);

    let mut engine = start_with_retry(&cfg, &health).await;
    health.set_degraded(false);
    health.set_ready(true);

    if cfg.once {
        let stats = engine.run_once().await?;
        info!(cycles = stats.len(), "run once complete");
        return Ok(());
    }

    let poll = Duration::from_millis(cfg.poll_interval_ms);
    let mut backoff = Duration::from_secs(1);
    let mut consecutive_failures = 0u32;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            result = engine.cycle() => match result {
                Ok(stats) => {
                    consecutive_failures = 0;
                    backoff = Duration::from_secs(1);
                    health.set_degraded(false);
                    // Backfill slices continue immediately; follow mode paces itself.
                    if engine.mode() == embedding_indexer::store::Mode::Follow || stats.backfill_complete {
                        tokio::time::sleep(poll).await;
                    }
                }
                Err(e @ IndexerError::Fatal(_)) => {
                    error!(error = %e, "fatal; exiting so the deployment restarts and re-verifies");
                    std::process::exit(1);
                }
                Err(e) => {
                    consecutive_failures += 1;
                    if consecutive_failures >= 3 {
                        health.set_degraded(true);
                    }
                    warn!(error = %e, consecutive_failures, retry_in_s = backoff.as_secs(), "cycle failed; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }
    Ok(())
}
