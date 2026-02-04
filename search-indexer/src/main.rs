//! Search Indexer Main Entry Point
//!
//! This is the main binary for the Geo Knowledge Graph search indexer.
//! It consumes entity events from Kafka and indexes them into OpenSearch.

use dotenv::dotenv;
use hermes_instrumentation::{error, info};
use search_indexer::health::start_health_server;
use search_indexer::{Dependencies, IndexingError};
use std::env;

/// Build telemetry configuration for Console or Sentry backend
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
            let debug = env::var("SENTRY_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            println!(
                "Telemetry: Sentry (env: {}, sample_rate: {})",
                environment.as_deref().unwrap_or("none"),
                traces_sample_rate
            );

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
        _ => {
            println!("Telemetry: Console (set SENTRY_DSN to enable Sentry)");
            Backend::Console
        }
    };

    Config::new("search-indexer", backend)
}

#[tokio::main]
async fn main() -> Result<(), IndexingError> {
    // Load environment variables from .env file
    dotenv().ok();

    // Initialize telemetry (keep guard alive for proper Sentry shutdown)
    let _telemetry_guard = hermes_instrumentation::init(build_telemetry_config())?;

    info!(
        service_name = "search-indexer",
        service_version = env!("CARGO_PKG_VERSION"),
        "Starting Geo Search Indexer"
    );

    // Initialize dependencies
    let deps = match Dependencies::new().await {
        Ok(deps) => {
            info!("Dependencies initialized successfully");
            deps
        }
        Err(e) => {
            error!(error = %e, "Failed to initialize dependencies");
            return Err(e);
        }
    };

    // Start health check server
    let health_port = env::var("HEALTH_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8080);
    let _health_handle =
        start_health_server(deps.provider.clone(), deps.kafka_admin.clone(), health_port);
    info!(port = health_port, "Health check server started");

    // Run the orchestrator
    match deps.orchestrator.run().await {
        Ok(()) => {
            info!("Search indexer completed successfully");
            Ok(())
        }
        Err(e) => {
            error!(error = %e, "Search indexer failed");
            Err(e.into())
        }
    }
}
