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

    /// Export telemetry via OpenTelemetry Protocol (OTLP) over gRPC.
    ///
    /// Best for local collectors that support gRPC:
    /// - OpenTelemetry Collector
    /// - Jaeger (with OTLP receiver)
    ///
    /// ```ignore
    /// Backend::OtlpGrpc {
    ///     endpoint: "http://localhost:4317".into(),
    ///     headers: vec![],
    ///     debug: false,
    /// }
    /// ```
    OtlpGrpc {
        /// OTLP gRPC endpoint (typically port 4317).
        endpoint: String,

        /// Optional headers/metadata for authentication.
        headers: Vec<(String, String)>,

        /// If true, also emit OTEL spans to stdout.
        debug: bool,
    },

    /// Export telemetry via OpenTelemetry Protocol (OTLP) over HTTP.
    ///
    /// Required for cloud providers like Axiom that only support HTTP:
    ///
    /// ```ignore
    /// Backend::OtlpHttp {
    ///     endpoint: "https://api.axiom.co/v1/traces".into(),
    ///     headers: vec![
    ///         ("Authorization".into(), "Bearer API_TOKEN".into()),
    ///         ("X-Axiom-Dataset".into(), "my-dataset".into()),
    ///     ],
    ///     debug: false,
    /// }
    /// ```
    OtlpHttp {
        /// OTLP HTTP endpoint (include full path, e.g., `/v1/traces`).
        endpoint: String,

        /// Headers for authentication.
        headers: Vec<(String, String)>,

        /// If true, also emit OTEL spans to stdout.
        debug: bool,
    },
}
