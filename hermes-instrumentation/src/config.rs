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

/// Telemetry backend selection.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Log spans and events to stdout.
    ///
    /// Useful for local development and debugging.
    Console,

    /// Export telemetry via OpenTelemetry Protocol (OTLP).
    ///
    /// Can target any OTLP-compatible backend:
    /// - OpenTelemetry Collector
    /// - Jaeger (with OTLP receiver)
    /// - Axiom (direct OTLP ingestion)
    /// - Grafana Cloud
    Otlp {
        /// OTLP gRPC endpoint.
        ///
        /// Examples:
        /// - `http://localhost:4317` (local collector)
        /// - `https://api.axiom.co` (Axiom direct)
        endpoint: String,
    },
}
