//! Example demonstrating Sentry telemetry export.
//!
//! Usage:
//!   SENTRY_DSN=... cargo run -p hermes-instrumentation --example sentry

use hermes_instrumentation::{info, info_span, Config, Instrument, Backend};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dsn = std::env::var("SENTRY_DSN").expect("SENTRY_DSN must be set");

    let config = Config::new(
        "hermes-instrumentation",
        Backend::Sentry {
            dsn,
            traces_sample_rate: 1.0,
            send_default_pii: false,
            environment: std::env::var("SENTRY_ENVIRONMENT").ok(),
            release: std::env::var("SENTRY_RELEASE").ok(),
            debug: true,
        },
    );

    let _telemetry = hermes_instrumentation::init(config)?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            info!("Sentry example started");

            async {
                info!("Doing work");
            }
            .instrument(info_span!("example_work"))
            .await;

            Ok::<(), Box<dyn std::error::Error>>(())
        })?;

    Ok(())
}
