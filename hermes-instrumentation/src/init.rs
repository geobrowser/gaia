//! Telemetry initialization.

use crate::config::{Backend, Config};
use opentelemetry_sdk::trace::SdkTracerProvider;
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

/// Guard that shuts down the tracer provider when dropped.
///
/// Keep this guard alive for the duration of your application. When dropped,
/// it will flush any pending spans and shut down the tracer provider.
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
    _sentry: Option<sentry::ClientInitGuard>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            // Shutdown flushes all pending spans before returning
            if let Err(e) = provider.shutdown() {
                eprintln!("Error shutting down tracer provider: {:?}", e);
            }
        }
    }
}

/// Initialize telemetry with the given configuration.
///
/// Returns a guard that must be kept alive for the duration of the application.
/// When the guard is dropped, it will flush any pending spans and shut down
/// the tracer provider.
///
/// Must be called once at service startup, before any tracing occurs.
///
/// **Important:** This must be called **before** the tokio runtime is created.
/// See the crate-level docs for details.
///
/// # Example
///
/// ```rust,no_run
/// use hermes_instrumentation::{init, Config};
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Initialize telemetry before tokio runtime
///     // Keep the guard alive until the end of main
///     let _telemetry = hermes_instrumentation::init(Config::console("my-service"))?;
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
pub fn init(config: Config) -> Result<TelemetryGuard, Error> {
    // Leak the namespace to get a &'static str for tracing
    let namespace: &'static str = Box::leak(config.namespace.into_boxed_str());

    match config.backend {
        Backend::Console => {
            init_console(namespace)?;
            Ok(TelemetryGuard {
                provider: None,
                _sentry: None,
            })
        }
        Backend::Sentry {
            dsn,
            traces_sample_rate,
            send_default_pii,
            environment,
            release,
            debug,
        } => {
            let (provider, sentry_guard) = init_sentry(
                namespace,
                &dsn,
                traces_sample_rate,
                send_default_pii,
                environment.as_deref(),
                release.as_deref(),
                debug,
            )?;
            Ok(TelemetryGuard {
                provider: Some(provider),
                _sentry: Some(sentry_guard),
            })
        }
    }
}

/// Shutdown telemetry, flushing any pending spans.
///
/// **Deprecated:** Use the `TelemetryGuard` returned by `init()` instead.
/// This function is kept for backwards compatibility but does nothing useful.
#[deprecated(note = "Use the TelemetryGuard returned by init() instead")]
pub fn shutdown() {
    // No-op - shutdown is now handled by dropping the TelemetryGuard
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

fn init_sentry(
    namespace: &'static str,
    dsn: &str,
    traces_sample_rate: f32,
    send_default_pii: bool,
    environment: Option<&str>,
    release: Option<&str>,
    debug: bool,
) -> Result<(SdkTracerProvider, sentry::ClientInitGuard), Error> {
    use opentelemetry::KeyValue;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::propagation::TraceContextPropagator;

    let mut options = sentry::ClientOptions {
        traces_sample_rate,
        send_default_pii,
        ..Default::default()
    };
    if let Some(env) = environment {
        options.environment = Some(env.to_string().into());
    }
    if let Some(rel) = release {
        options.release = Some(rel.to_string().into());
    }

    let sentry_guard = sentry::init((dsn, options));

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", namespace))
        .build();

    let mut provider_builder = SdkTracerProvider::builder().with_resource(resource);

    if debug {
        let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
        provider_builder = provider_builder.with_simple_exporter(stdout_exporter);
    }

    let provider = provider_builder.build();
    let tracer = provider.tracer(namespace);

    let telemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_tracked_inactivity(true);

    let sentry_layer = sentry::integrations::tracing::layer();

    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LevelFilter::INFO);

    if debug {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false);

        tracing_subscriber::registry()
            .with(level)
            .with(telemetry_layer)
            .with(sentry_layer)
            .with(fmt_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(level)
            .with(telemetry_layer)
            .with(sentry_layer)
            .init();
    }

    eprintln!("Telemetry initialized: service.name={} sentry", namespace);

    Ok((provider, sentry_guard))
}
