//! Telemetry initialization.

use crate::config::{Backend, Config};

/// Errors that can occur during telemetry initialization.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to set the global tracing subscriber.
    #[error("failed to set global subscriber: {0}")]
    SetGlobalSubscriber(#[from] tracing::subscriber::SetGlobalDefaultError),

    /// Failed to initialize OpenTelemetry.
    #[error("failed to initialize OpenTelemetry: {0}")]
    OpenTelemetry(String),
}

/// Initialize telemetry with the given configuration.
///
/// Must be called once at service startup, before any tracing occurs.
///
/// # Example
///
/// ```rust,no_run
/// use hermes_instrumentation::{init, Config, Backend};
///
/// fn main() -> Result<(), hermes_instrumentation::Error> {
///     hermes_instrumentation::init(Config {
///         namespace: "my-service".to_string(),
///         backend: Backend::Console,
///     })?;
///
///     tracing::info!("Service started");
///     Ok(())
/// }
/// ```
///
/// # Errors
///
/// Returns an error if the global subscriber has already been set or if
/// OpenTelemetry initialization fails.
pub fn init(config: Config) -> Result<(), Error> {
    match config.backend {
        Backend::Console => init_console(&config.namespace),
        Backend::Otlp { endpoint } => init_otlp(&config.namespace, &endpoint),
    }
}

fn init_console(_namespace: &str) -> Result<(), Error> {
    // TODO: Implement in Phase 2
    todo!("Console backend not yet implemented")
}

fn init_otlp(_namespace: &str, _endpoint: &str) -> Result<(), Error> {
    // TODO: Implement in Phase 4
    todo!("OTLP backend not yet implemented")
}
