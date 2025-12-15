# 0001: Hermes Telemetry Crate

## Status

Accepted (Implemented)

## Context

The hermes ecosystem currently has minimal observability infrastructure. `hermes-ipfs-cache` uses basic `tracing` with `tracing_subscriber::fmt`, but there's no standardization across services and no support for exporting telemetry to external backends.

As the system grows, we need:
- Consistent instrumentation across all hermes services
- Ability to trace requests across service boundaries
- Support for external observability backends (Axiom, Grafana, Jaeger)
- Namespaced spans to identify which service generated telemetry

### Goals

1. **Single dependency**: All hermes crates use `hermes-instrumentation` instead of depending on `tracing` directly
2. **Automatic namespacing**: Spans are automatically prefixed with the service namespace (e.g., `ipfs-cache.fetch_content`)
3. **Pluggable backends**: Support console output and OpenTelemetry export at runtime
4. **Standard tracing API**: Consumers use familiar `#[instrument]`, `info_span!()`, etc.

### Prior Art

**Effect-TS** provides composable telemetry via higher-order functions like `Effect.withSpan("name")` that wrap operations with tracing context. While Rust's macro system differs from TypeScript's runtime composition, the `tracing` crate ecosystem provides similar capabilities:

- `#[instrument]` attribute for function-level instrumentation
- `.instrument(span)` method for callsite instrumentation of futures
- `span.in_scope(|| ...)` for callsite instrumentation of sync code
- Subscriber layers for processing/exporting telemetry

**Rust tracing ecosystem**:
- `tracing` - instrumentation API (spans, events)
- `tracing-subscriber` - composable subscriber layers
- `tracing-opentelemetry` - OpenTelemetry bridge
- `opentelemetry-otlp` - OTLP export

## Decision

Create a `hermes-instrumentation` crate that:

1. Re-exports `tracing` macros for single-dependency convenience
2. Provides an `init()` function to configure telemetry at startup
3. Installs a custom subscriber layer that auto-prefixes span names with the configured namespace
4. Supports two backends: Console and OTLP (OpenTelemetry Protocol)

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     hermes-instrumentation                       │
├─────────────────────────────────────────────────────────────────┤
│  pub use tracing::{instrument, info, debug, error, ...};        │
│                                                                  │
│  pub fn init(config: Config) -> Result<()>                      │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │  Subscriber Stack                                        │    │
│  │  ┌─────────────────────────────────────────────────┐    │    │
│  │  │  NamespacePrefixLayer (custom)                  │    │    │
│  │  │  - Prepends namespace to all span names         │    │    │
│  │  └─────────────────────────────────────────────────┘    │    │
│  │  ┌─────────────────────────────────────────────────┐    │    │
│  │  │  Backend Layer (Console or OpenTelemetry)       │    │    │
│  │  └─────────────────────────────────────────────────┘    │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### API Design

```rust
// hermes-instrumentation/src/lib.rs

/// Re-export tracing macros for convenience
pub use tracing::{
    debug, error, info, trace, warn,
    debug_span, error_span, info_span, trace_span, warn_span,
    instrument, Instrument,
};

/// Configuration for telemetry initialization
pub struct Config {
    /// Service namespace, prefixed to all span names
    /// e.g., "ipfs-cache" results in spans like "ipfs-cache.fetch_content"
    pub namespace: String,
    
    /// Telemetry backend
    pub backend: Backend,
}

pub enum Backend {
    /// Log spans and events to stdout
    Console,
    
    /// Export via OpenTelemetry Protocol
    Otlp {
        /// OTLP endpoint (e.g., "http://localhost:4317" or "https://api.axiom.co")
        endpoint: String,
    },
}

/// Initialize telemetry with the given configuration.
/// 
/// Must be called once at service startup, before any tracing occurs.
/// 
/// # Example
/// 
/// ```rust
/// use hermes_instrumentation::{init, Config, Backend};
/// 
/// fn main() -> anyhow::Result<()> {
///     hermes_instrumentation::init(Config {
///         namespace: "ipfs-cache".to_string(),
///         backend: Backend::Console,
///     })?;
///     
///     // Now use tracing as normal
///     tracing::info!("Service started");
///     
///     Ok(())
/// }
/// ```
pub fn init(config: Config) -> Result<(), Error>;
```

### Consumer Usage

```rust
// hermes-ipfs-cache/src/main.rs

use hermes_instrumentation::{self, info, instrument, Config, Backend};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize telemetry
    hermes_instrumentation::init(Config {
        namespace: "ipfs-cache".to_string(),
        backend: Backend::Console,
    })?;
    
    info!("Starting Hermes IPFS Cache");
    
    let sink = IpfsCacheSink::new(cache, ipfs_source);
    sink.run(StreamSource::mock()).await?;
    
    Ok(())
}

// hermes-ipfs-cache/src/lib.rs

use hermes_instrumentation::{instrument, info, debug, error};

impl IpfsCacheSink {
    #[instrument(skip(self, events))]
    pub async fn process_block(&self, events: Vec<Event>) -> Result<()> {
        // Span name becomes: "ipfs-cache.process_block"
        info!(event_count = events.len(), "Processing block");
        
        for event in events {
            self.process_event(event).await?;
        }
        
        Ok(())
    }
    
    #[instrument(skip(self))]
    async fn fetch_from_ipfs(&self, cid: &str) -> Result<Bytes> {
        // Span name becomes: "ipfs-cache.fetch_from_ipfs"
        debug!(cid, "Fetching from IPFS");
        // ...
    }
}
```

### Namespace Prefixing Implementation

The automatic namespace prefix is implemented via a custom `tracing_subscriber::Layer`:

```rust
// hermes-instrumentation/src/layer.rs

use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

pub struct NamespacePrefixLayer {
    namespace: String,
}

impl<S> Layer<S> for NamespacePrefixLayer
where
    S: tracing::Subscriber,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        // Prefix the span name with namespace
        // Implementation details TBD - may need to use span extensions
        // to store the prefixed name
    }
}
```

**Note**: The exact implementation of span name prefixing may require using OpenTelemetry's `Resource` attributes instead of modifying span names directly. This would set `service.name` or `service.namespace` as resource attributes, which is more aligned with OpenTelemetry semantic conventions.

Alternative approach using Resource attributes:

```rust
use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;

let resource = Resource::new(vec![
    KeyValue::new("service.name", config.namespace.clone()),
    KeyValue::new("service.namespace", "hermes"),
]);
```

We'll evaluate both approaches during implementation and choose the one that provides better compatibility with observability backends.

### Backend Configuration

#### Console Backend

```rust
Backend::Console => {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true);
    
    tracing_subscriber::registry()
        .with(namespace_layer)
        .with(fmt_layer)
        .init();
}
```

#### OTLP Backend

```rust
Backend::Otlp { endpoint } => {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()?;
    
    let tracer_provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(Resource::new(vec![
            KeyValue::new("service.name", config.namespace.clone()),
        ]))
        .build();
    
    let telemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer_provider.tracer("hermes"));
    
    tracing_subscriber::registry()
        .with(namespace_layer)
        .with(telemetry_layer)
        .init();
}
```

### Future: OpenTelemetry Collector

For production deployments requiring advanced routing (e.g., different backends per service), an OpenTelemetry Collector can be deployed in k8s:

```
┌─────────────┐
│ ipfs-cache  │──┐
└─────────────┘  │
┌─────────────┐  │  OTLP   ┌─────────────────┐        ┌─────────┐
│ hermes-relay│──┼────────▶│  OTel Collector │───────▶│  Axiom  │
└─────────────┘  │         └─────────────────┘        └─────────┘
┌─────────────┐  │
│ pipeline    │──┘
└─────────────┘
```

The collector can route spans based on `service.name` attribute to different backends. This requires no changes to `hermes-instrumentation` - services just point their OTLP endpoint to the collector instead of directly to Axiom.

This is out of scope for the initial implementation but the architecture supports it.

## Implementation Plan

### Phase 1: Crate Setup

1. Create `hermes-instrumentation/` directory structure:
   ```
   hermes-instrumentation/
   ├── Cargo.toml
   ├── src/
   │   ├── lib.rs       # Public API, re-exports
   │   ├── config.rs    # Config, Backend types
   │   ├── init.rs      # init() function
   │   └── layer.rs     # NamespacePrefixLayer (if needed)
   └── README.md
   ```

2. Add dependencies to `Cargo.toml`:
   ```toml
   [dependencies]
   tracing = "0.1"
   tracing-subscriber = { version = "0.3", features = ["env-filter"] }
   tracing-opentelemetry = "0.22"
   opentelemetry = "0.21"
   opentelemetry_sdk = { version = "0.21", features = ["rt-tokio"] }
   opentelemetry-otlp = { version = "0.14", features = ["tonic"] }
   thiserror = "1.0"
   ```

3. Implement `Config` and `Backend` types

### Phase 2: Console Backend

1. Implement `init()` for `Backend::Console`
2. Add basic formatting configuration
3. Test with a simple example

### Phase 3: Namespace Prefixing

1. Evaluate approaches:
   - Custom `Layer` that modifies span names
   - OpenTelemetry `Resource` with `service.name`
   - Both (layer for console, resource for OTLP)

2. Implement chosen approach
3. Verify spans appear with correct prefix in console output

### Phase 4: OTLP Backend

1. Implement `init()` for `Backend::Otlp`
2. Test with local OpenTelemetry Collector or Jaeger
3. Verify spans export correctly with namespace

### Phase 5: Migration

1. Update `hermes-ipfs-cache` to use `hermes-instrumentation`
2. Remove direct `tracing` and `tracing-subscriber` dependencies
3. Verify existing functionality works

### Phase 6: Documentation

1. Add comprehensive README
2. Document configuration options
3. Add examples for common patterns

## File Changes Summary

| File | Action | Phase |
|------|--------|-------|
| `hermes-instrumentation/Cargo.toml` | Create | 1 |
| `hermes-instrumentation/src/lib.rs` | Create | 1 |
| `hermes-instrumentation/src/config.rs` | Create | 1 |
| `hermes-instrumentation/src/init.rs` | Create | 2-4 |
| `hermes-instrumentation/src/layer.rs` | Create | 3 |
| `hermes-instrumentation/README.md` | Create | 6 |
| `hermes-ipfs-cache/Cargo.toml` | Modify | 5 |
| `hermes-ipfs-cache/src/main.rs` | Modify | 5 |
| `hermes-ipfs-cache/src/lib.rs` | Modify | 5 |
| `Cargo.toml` (workspace) | Modify | 1 |

## Consequences

### Positive

- **Unified observability**: All hermes services emit consistent telemetry
- **Single dependency**: Consumers don't need to manage tracing crate versions
- **Flexible backends**: Switch between console and OTLP without code changes
- **Standard patterns**: Uses idiomatic `tracing` API that Rust developers know
- **Future-proof**: OTLP export works with any OpenTelemetry-compatible backend

### Negative

- **Additional dependency**: All hermes crates now depend on `hermes-instrumentation`
- **Initialization requirement**: Must call `init()` before any tracing
- **OpenTelemetry complexity**: OTLP backend brings in significant dependencies

### Neutral

- **No breaking changes**: Existing tracing code continues to work
- **Opt-in migration**: Services can migrate incrementally

## Open Questions

1. **Should we support log level filtering?**
   - Could add `level: Level` to `Config`
   - Or rely on `RUST_LOG` env var
   - Decision: Defer to consumers via env var for now

2. **Should we support multiple backends simultaneously?**
   - e.g., Console + OTLP for local debugging
   - Adds complexity
   - Decision: Start with single backend, add if needed

3. **How to handle shutdown/flush?**
   - OTLP exporter needs graceful shutdown to flush spans
   - May need `shutdown()` function or guard type
   - Decision: Investigate during Phase 4

4. **Metrics support?**
   - OpenTelemetry also supports metrics
   - Could add `Counter`, `Histogram` types
   - Decision: Out of scope for v1, traces only

## References

- [tracing crate documentation](https://docs.rs/tracing/latest/tracing/)
- [tracing-opentelemetry](https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/)
- [OpenTelemetry Rust](https://opentelemetry.io/docs/languages/rust/)
- [OpenTelemetry Collector](https://opentelemetry.io/docs/collector/)
- [OpenTelemetry Semantic Conventions - Service](https://opentelemetry.io/docs/specs/semconv/resource/#service)
