//! Configuration types for telemetry initialization.

/// Configuration for telemetry initialization.
#[derive(Debug, Clone)]
pub struct Config {
    /// Service namespace, prefixed to all span names.
    ///
    /// For example, if namespace is "ipfs-cache", a span named "fetch_content"
    /// will appear as "ipfs-cache.fetch_content" in telemetry output.
    pub namespace: &'static str,

    /// Telemetry backend to use.
    pub backend: Backend,
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
    ///     endpoint: "http://localhost:4317",
    ///     headers: &[],
    ///     debug: false,
    /// }
    /// ```
    OtlpGrpc {
        /// OTLP gRPC endpoint (typically port 4317).
        endpoint: &'static str,

        /// Optional headers/metadata for authentication.
        headers: &'static [(&'static str, &'static str)],

        /// If true, also emit OTEL spans to stdout.
        debug: bool,
    },

    /// Export telemetry via OpenTelemetry Protocol (OTLP) over HTTP.
    ///
    /// Required for cloud providers like Axiom that only support HTTP:
    ///
    /// ```ignore
    /// Backend::OtlpHttp {
    ///     endpoint: "https://api.axiom.co/v1/traces",
    ///     headers: &[
    ///         ("Authorization", "Bearer API_TOKEN"),
    ///         ("X-Axiom-Dataset", "my-dataset"),
    ///     ],
    ///     debug: false,
    /// }
    /// ```
    OtlpHttp {
        /// OTLP HTTP endpoint (include full path, e.g., `/v1/traces`).
        endpoint: &'static str,

        /// Headers for authentication.
        headers: &'static [(&'static str, &'static str)],

        /// If true, also emit OTEL spans to stdout.
        debug: bool,
    },
}
