use std::env;

use actions_indexer::{Dependencies, IndexingError};
use actions_indexer_pipeline::orchestrator::Orchestrator;
use dotenv::dotenv;
use hermes_instrumentation::info;

fn main() -> Result<(), IndexingError> {
    dotenv().ok();

    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    info!("Starting actions-indexer");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| IndexingError::Config(format!("Failed to build tokio runtime: {}", e)))?
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
            let debug = env::var("SENTRY_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            println!(
                "Telemetry: Sentry (env: {}, release: {}, debug: {})",
                environment.as_deref().unwrap_or("none"),
                release.as_deref().unwrap_or("none"),
                if debug { "yes" } else { "no" }
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

    Config::new("actions-indexer", backend)
}

async fn async_main() -> Result<(), IndexingError> {
    let dependencies = Dependencies::new().await?;

    let orchestrator = Orchestrator::new(
        dependencies.consumer,
        dependencies.processor,
        dependencies.loader,
    );
    orchestrator.run().await?;
    Ok(())
}
