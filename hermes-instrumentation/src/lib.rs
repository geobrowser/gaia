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
//! use hermes_instrumentation::{init, info, info_span, Config, Instrument};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Initialize telemetry BEFORE starting the tokio runtime.
//!     // Keep the guard alive until the end of main to ensure spans are flushed.
//!     let _telemetry = hermes_instrumentation::init(Config::console("my-service"))?;
//!
//!     tokio::runtime::Builder::new_multi_thread()
//!         .enable_all()
//!         .build()?
//!         .block_on(async {
//!             info!("Service started");
//!             // ... application runs ...
//!             Ok(())
//!         })
//! }
//! ```
//!
//! # Important: OTLP HTTP and Async Runtimes
//!
//! When using the OTLP HTTP backend, you **must** initialize telemetry before creating
//! a tokio runtime. The HTTP backend uses a blocking reqwest client internally (for the
//! background batch processor), which creates its own internal runtime. Tokio runtimes
//! cannot be nested, so initializing inside `#[tokio::main]` will panic.
//!
//! The Console and OTLP gRPC backends do not have this restriction.

mod config;
mod init;

// Re-export configuration types
pub use config::{Backend, Config};

// Re-export initialization
#[allow(deprecated)]
pub use init::{Error, TelemetryGuard, init, shutdown};

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
