//! Telemetry initialization.

use crate::config::{Backend, Config};
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
/// # Example
///
/// ```rust,no_run
/// use hermes_instrumentation::{init, Config, Backend};
///
/// fn main() -> Result<(), hermes_instrumentation::Error> {
///     hermes_instrumentation::init(Config::console("my-service"))?;
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
    use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};

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
    // Use simple exporter for immediate export (no batching)
    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", namespace))
        .build();

    let mut provider_builder = SdkTracerProvider::builder()
        .with_simple_exporter(otlp_exporter)
        .with_resource(resource);

    // Optionally add stdout exporter for debugging
    if debug {
        let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
        provider_builder = provider_builder.with_simple_exporter(stdout_exporter);
    }

    let provider = provider_builder.build();

    // Get tracer before setting global provider
    let tracer = provider.tracer(namespace);

    // Set global tracer provider
    opentelemetry::global::set_tracer_provider(provider);

    // Create OpenTelemetry tracing layer
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Use RUST_LOG env var for filtering, with a sensible default
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
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
    use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};

    // Build headers map
    let header_map: std::collections::HashMap<String, String> =
        headers.iter().cloned().collect();

    // Create OTLP HTTP exporter with headers
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_headers(header_map)
        .build()
        .map_err(|e| Error::OpenTelemetry(e.to_string()))?;

    // Create tracer provider with service.name resource
    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", namespace))
        .build();

    let mut provider_builder = SdkTracerProvider::builder()
        .with_simple_exporter(otlp_exporter)
        .with_resource(resource);

    // Optionally add stdout exporter for debugging
    if debug {
        let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
        provider_builder = provider_builder.with_simple_exporter(stdout_exporter);
    }

    let provider = provider_builder.build();

    // Get tracer before setting global provider
    let tracer = provider.tracer(namespace);

    // Set global tracer provider
    opentelemetry::global::set_tracer_provider(provider);

    // Create OpenTelemetry tracing layer
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Use RUST_LOG env var for filtering, with a sensible default
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(telemetry_layer)
        .init();

    // Log startup to console
    eprintln!(
        "Telemetry initialized: service.name={} otlp.endpoint={} (HTTP)",
        namespace, endpoint
    );

    Ok(())
}

/// Shutdown the telemetry system, flushing any pending spans.
///
/// This should be called before the application exits to ensure all spans are exported.
pub fn shutdown() {
    // In OpenTelemetry 0.31+, shutdown is handled by dropping the provider
    // or calling shutdown on it directly. The global provider doesn't have
    // a shutdown function anymore, so we just need to ensure spans are flushed.
    // For simple exporters, spans are exported immediately, so this is a no-op.
}
