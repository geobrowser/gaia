//! Configuration types for telemetry initialization.

/// Configuration for telemetry initialization.
#[derive(Debug, Clone)]
pub struct Config {
    /// Service namespace, prefixed to all span names.
    ///
    /// For example, if namespace is "ipfs-cache", a span named "fetch_content"
    /// will appear as "ipfs-cache.fetch_content" in telemetry output.
    pub namespace: String,

    /// Telemetry backend to use.
    pub backend: Backend,
}

impl Config {
    /// Create a new configuration with the given namespace and backend.
    pub fn new(namespace: impl Into<String>, backend: Backend) -> Self {
        Self {
            namespace: namespace.into(),
            backend,
        }
    }

    /// Create a console backend configuration.
    pub fn console(namespace: impl Into<String>) -> Self {
        Self::new(namespace, Backend::Console)
    }
}

/// Telemetry backend selection.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Log spans and events to stdout.
    ///
    /// Useful for local development and debugging.
    Console,

    /// Export telemetry to Sentry using OpenTelemetry spans.
    ///
    /// ```ignore
    /// Backend::Sentry {
    ///     dsn: "https://...@o0.ingest.sentry.io/0".into(),
    ///     traces_sample_rate: 1.0,
    ///     send_default_pii: false,
    ///     environment: Some("production".into()),
    ///     release: Some("my-service@1.2.3".into()),
    ///     debug: false,
    /// }
    /// ```
    Sentry {
        /// Sentry DSN / ingest URL.
        dsn: String,

        /// Sample rate for transactions (0.0 - 1.0).
        traces_sample_rate: f32,

        /// Whether to send default PII (IP address, headers, etc.).
        send_default_pii: bool,

        /// Optional environment tag (e.g., "prod", "staging").
        environment: Option<String>,

        /// Optional release name (e.g., "service@1.2.3").
        release: Option<String>,

        /// If true, also emit spans to stdout.
        debug: bool,
    },
}
