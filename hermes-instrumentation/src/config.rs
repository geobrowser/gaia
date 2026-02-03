//! Configuration types for telemetry initialization.

/// Optional Axiom configuration for real-time trace export.
///
/// When configured, traces are exported to both Sentry (for error correlation)
/// and Axiom (for 100% trace storage without server-side sampling).
#[derive(Clone)]
pub struct AxiomConfig {
    /// Axiom API token (from `AXIOM_TOKEN` env var).
    pub token: String,

    /// Dataset name for traces (from `AXIOM_DATASET` env var, default: "gaia-traces").
    pub dataset: String,
}

// Custom Debug implementation to prevent token leakage in logs
impl std::fmt::Debug for AxiomConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AxiomConfig")
            .field("dataset", &self.dataset)
            .finish_non_exhaustive()
    }
}

impl AxiomConfig {
    /// Create from environment variables if `AXIOM_TOKEN` is set.
    ///
    /// Returns `None` if `AXIOM_TOKEN` is not set.
    ///
    /// # Panics
    ///
    /// Panics if `AXIOM_TOKEN` or `AXIOM_DATASET` is set but empty (configuration error).
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("AXIOM_TOKEN").ok()?;
        if token.is_empty() {
            panic!("AXIOM_TOKEN is set but empty - this is a configuration error");
        }
        let dataset = std::env::var("AXIOM_DATASET").unwrap_or_else(|_| "gaia-traces".to_string());
        if dataset.is_empty() {
            panic!("AXIOM_DATASET is set but empty - this is a configuration error");
        }
        Some(Self { token, dataset })
    }
}

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

        /// Optional Axiom export for 100% trace storage.
        ///
        /// When set, traces are exported to both Sentry and Axiom via OTLP.
        /// Use [`AxiomConfig::from_env()`] to configure from environment variables.
        axiom: Option<AxiomConfig>,
    },
}
