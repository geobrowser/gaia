//! Unified telemetry for the Hermes ecosystem.
//!
//! This crate provides a single dependency for all observability needs across Hermes services.
//! It wraps the `tracing` crate ecosystem and provides:
//!
//! - Automatic namespace prefixing for spans
//! - Console and OpenTelemetry (OTLP) backend support
//! - Re-exported tracing macros for convenience
//!
//! # Usage
//!
//! ```rust,no_run
//! use hermes_instrumentation::{init, info, instrument, Config, Backend};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Initialize telemetry at startup
//!     hermes_instrumentation::init(Config {
//!         namespace: "my-service".to_string(),
//!         backend: Backend::Console,
//!     })?;
//!
//!     info!("Service started");
//!     Ok(())
//! }
//! ```

mod config;
mod init;

// Re-export configuration types
pub use config::{Backend, Config};

// Re-export initialization
pub use init::{init, Error};

// Re-export tracing macros for convenience
pub use tracing::{
    // Event macros
    debug,
    error,
    info,
    trace,
    warn,
    // Span macros
    debug_span,
    error_span,
    info_span,
    trace_span,
    warn_span,
    // Attributes and utilities
    instrument,
    Instrument,
    Level,
    Span,
};
