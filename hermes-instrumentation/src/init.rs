//! Telemetry initialization.

use crate::config::{Backend, Config};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

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
/// **Important:** When using the OTLP HTTP backend with tokio, this must be called
/// **before** the tokio runtime is created. See the crate-level docs for details.
///
/// # Example
///
/// ```rust,no_run
/// use hermes_instrumentation::{init, Config};
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Initialize telemetry before tokio runtime
///     hermes_instrumentation::init(Config::console("my-service"))?;
///
///     tokio::runtime::Builder::new_multi_thread()
///         .enable_all()
///         .build()?
///         .block_on(async {
///             tracing::info!("Service started");
///             Ok(())
///         })
/// }
/// ```
///
/// # Errors
///
/// Returns an error if the global subscriber has already been set or if
/// OpenTelemetry initialization fails.
pub fn init(config: Config) -> Result<(), Error> {
    // Leak the namespace to get a &'static str for tracing
    let namespace: &'static str = Box::leak(config.namespace.into_boxed_str());

    match config.backend {
        Backend::Console => init_console(namespace),
        Backend::OtlpGrpc {
            endpoint,
            headers,
            debug,
        } => init_otlp_grpc(namespace, &endpoint, &headers, debug),
        Backend::OtlpHttp {
            endpoint,
            headers,
            debug,
        } => init_otlp_http(namespace, &endpoint, &headers, debug),
    }
}

/// Shutdown telemetry, flushing any pending spans.
///
/// This is optional for long-running services since the `SdkTracerProvider`
/// will automatically flush and shutdown when the process exits. However,
/// for short-lived processes or tests, calling this ensures all spans are exported.
pub fn shutdown() {
    // The global tracer provider will be shut down when dropped.
    // For batch processors, give time for the background thread to flush.
    std::thread::sleep(std::time::Duration::from_millis(500));
}

fn init_console(namespace: &'static str) -> Result<(), Error> {
    // Create a formatting layer for console output
    // Use a custom format that includes the namespace
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .event_format(NamespacedEventFormat::new(namespace));

    // Use RUST_LOG env var for filtering, with a sensible default
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();

    // Log startup
    tracing::info!(service.name = namespace, "Telemetry initialized");

    Ok(())
}

/// Custom event formatter that prefixes span names with the namespace.
struct NamespacedEventFormat {
    namespace: &'static str,
    timer: tracing_subscriber::fmt::time::SystemTime,
}

impl NamespacedEventFormat {
    fn new(namespace: &'static str) -> Self {
        Self {
            namespace,
            timer: tracing_subscriber::fmt::time::SystemTime,
        }
    }
}

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for NamespacedEventFormat
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        use tracing_subscriber::fmt::time::FormatTime;

        // Timestamp
        self.timer.format_time(&mut writer)?;
        write!(writer, " ")?;

        // Level
        let level = *event.metadata().level();
        write!(writer, "{:>5} ", level)?;

        // Span context with namespace prefix
        if let Some(scope) = ctx.event_scope() {
            let mut first = true;
            for span in scope.from_root() {
                if !first {
                    write!(writer, ":")?;
                }
                // Prefix span name with namespace
                write!(writer, "{}.{}", self.namespace, span.name())?;

                let ext = span.extensions();
                if let Some(fields) = ext
                    .get::<tracing_subscriber::fmt::FormattedFields<N>>()
                    .filter(|f| !f.is_empty())
                {
                    write!(writer, "{{{}}}", fields)?;
                }
                first = false;
            }
            if !first {
                write!(writer, ": ")?;
            }
        }

        // Target
        write!(writer, "{}: ", event.metadata().target())?;

        // Event fields
        ctx.field_format().format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

fn init_otlp_grpc(
    namespace: &'static str,
    endpoint: &str,
    headers: &[(String, String)],
    debug: bool,
) -> Result<(), Error> {
    use opentelemetry::KeyValue;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::tonic_types::metadata::MetadataMap;
    use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracerProvider};

    // Build metadata from headers using the underlying http::HeaderMap
    let mut metadata = MetadataMap::default();
    {
        let header_map = metadata.as_mut();
        for (key, value) in headers {
            if let (Ok(key), Ok(value)) = (
                key.parse::<http::header::HeaderName>(),
                value.parse::<http::header::HeaderValue>(),
            ) {
                header_map.insert(key, value);
            }
        }
    }

    // Create OTLP gRPC exporter with headers
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_metadata(metadata)
        .build()
        .map_err(|e| Error::OpenTelemetry(e.to_string()))?;

    // Create tracer provider with service.name resource
    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", namespace))
        .build();

    // Use batch processor for efficient export in production.
    // Spans are buffered and exported periodically in batches.
    let batch_processor = BatchSpanProcessor::builder(otlp_exporter).build();

    let mut provider_builder = SdkTracerProvider::builder()
        .with_span_processor(batch_processor)
        .with_resource(resource);

    // Optionally add stdout exporter for debugging (simple is fine for sync stdout)
    if debug {
        let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
        provider_builder = provider_builder.with_simple_exporter(stdout_exporter);
    }

    let provider = provider_builder.build();

    // Get tracer before setting global provider
    let tracer = provider.tracer(namespace);

    // Set global tracer provider
    opentelemetry::global::set_tracer_provider(provider);

    // Create OpenTelemetry tracing layer with tracked inactivity for async spans
    let telemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_tracked_inactivity(true);

    // Use LevelFilter for consistency with HTTP backend (prevents potential reentrancy)
    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LevelFilter::INFO);

    tracing_subscriber::registry()
        .with(level)
        .with(telemetry_layer)
        .init();

    // Log startup to console
    eprintln!(
        "Telemetry initialized: service.name={} otlp.endpoint={} (gRPC)",
        namespace, endpoint
    );

    Ok(())
}

fn init_otlp_http(
    namespace: &'static str,
    endpoint: &str,
    headers: &[(String, String)],
    debug: bool,
) -> Result<(), Error> {
    use opentelemetry::KeyValue;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracerProvider};

    // Build headers map
    let header_map: std::collections::HashMap<String, String> = headers.iter().cloned().collect();

    // Create blocking reqwest client - the BatchSpanProcessor runs in a background
    // thread without a tokio runtime, so we need a blocking HTTP client
    let http_client = reqwest::blocking::Client::new();

    // Create OTLP HTTP exporter with headers
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_http_client(http_client)
        .with_endpoint(endpoint)
        .with_headers(header_map)
        .build()
        .map_err(|e| Error::OpenTelemetry(e.to_string()))?;

    // Create tracer provider with service.name resource
    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", namespace))
        .build();

    // IMPORTANT: Use batch processor instead of simple exporter.
    // Simple exporter tries to export synchronously when a span ends, which
    // deadlocks when using an async HTTP client (reqwest) in a tokio runtime.
    // Batch processor exports in a background task, avoiding the deadlock.
    let batch_processor = BatchSpanProcessor::builder(otlp_exporter).build();

    let mut provider_builder = SdkTracerProvider::builder()
        .with_span_processor(batch_processor)
        .with_resource(resource);

    // Optionally add stdout exporter for debugging (simple is fine for sync stdout)
    if debug {
        let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
        provider_builder = provider_builder.with_simple_exporter(stdout_exporter);
    }

    let provider = provider_builder.build();

    // Get tracer before setting global provider
    let tracer = provider.tracer(namespace);

    // Set global tracer provider
    opentelemetry::global::set_tracer_provider(provider);

    // Create OpenTelemetry tracing layer with tracked inactivity for async spans
    let telemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_tracked_inactivity(true);

    // IMPORTANT: Use LevelFilter instead of EnvFilter to prevent reentrancy issues.
    // When the reqwest HTTP client sends spans to OTLP, it may create its own tracing
    // spans which would then get exported, causing issues. LevelFilter avoids this.
    // See: https://github.com/tokio-rs/tracing-opentelemetry/issues/159
    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LevelFilter::INFO);

    // Also add a fmt layer for console output when debug mode is enabled
    if debug {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false);

        tracing_subscriber::registry()
            .with(level)
            .with(telemetry_layer)
            .with(fmt_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(level)
            .with(telemetry_layer)
            .init();
    }

    // Log startup to console
    eprintln!(
        "Telemetry initialized: service.name={} otlp.endpoint={} (HTTP)",
        namespace, endpoint
    );

    Ok(())
}
