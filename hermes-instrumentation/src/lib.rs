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
//! use hermes_instrumentation::{init, info, info_span, Config, Backend, Instrument};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Initialize telemetry at startup
//!     hermes_instrumentation::init(Config::console("my-service"))?;
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
pub use init::{Error, init, shutdown};

// Re-export tracing macros for convenience
pub use tracing::{
    Instrument,
    Level,
    Span,
    // Event macros
    debug,
    // Span macros
    debug_span,
    error,
    error_span,
    info,
    info_span,
    // Utilities
    instrument,
    trace,
    trace_span,
    warn,
    warn_span,
};
