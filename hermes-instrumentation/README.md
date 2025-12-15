# hermes-instrumentation

Unified telemetry for the Hermes ecosystem.

## Overview

This crate provides a single dependency for all observability needs across Hermes services. It wraps the `tracing` crate ecosystem and provides:

- Automatic namespace prefixing for spans (e.g., `ipfs-cache.fetch_content`)
- Console and OpenTelemetry (OTLP) backend support
- Re-exported tracing macros for convenience

## Usage

Add the dependency:

```toml
[dependencies]
hermes-instrumentation = { path = "../hermes-instrumentation" }
```

Initialize telemetry at startup:

```rust
use hermes_instrumentation::{init, info, info_span, Config, Instrument};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize telemetry BEFORE starting the tokio runtime (see note below)
    hermes_instrumentation::init(Config::console("my-service"))?;

    // Start tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    info!("Service started");

    // Create spans explicitly at call sites
    async {
        info!("Doing work");
    }
    .instrument(info_span!("my_operation"))
    .await;

    Ok(())
}
```

### Important: Initialization Order with Async Runtimes

When using the OTLP HTTP backend with tokio, you **must** initialize telemetry **before** creating the tokio runtime:

```rust
// ✅ Correct: Initialize before runtime
fn main() -> Result<(), Box<dyn std::error::Error>> {
    hermes_instrumentation::init(config)?;
    
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

// ❌ Wrong: Will panic with "Cannot drop a runtime in a context where blocking is not allowed"
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    hermes_instrumentation::init(config)?;
    // ...
}
```

**Why?** The OTLP HTTP backend uses a `BatchSpanProcessor` that runs in a background thread. This processor uses a blocking HTTP client (`reqwest::blocking`) which creates its own internal tokio runtime. Tokio runtimes cannot be nested, so if you initialize telemetry inside `#[tokio::main]`, it will panic when the blocking client tries to create its nested runtime.

The Console and OTLP gRPC backends do not have this restriction.

## Backends

### Console

Outputs formatted logs to stdout with namespace-prefixed spans:

```rust
Backend::Console
```

### OTLP (gRPC)

For local collectors that support gRPC (Jaeger, OpenTelemetry Collector):

```rust
Backend::OtlpGrpc {
    endpoint: "http://localhost:4317".into(),
    headers: vec![],
    debug: false,
}
```

### OTLP (HTTP)

For cloud providers that require HTTP (Axiom, Grafana Cloud):

```rust
Backend::OtlpHttp {
    endpoint: "https://api.axiom.co/v1/traces".into(),
    headers: vec![
        ("Authorization".into(), "Bearer API_TOKEN".into()),
        ("X-Axiom-Dataset".into(), "my-dataset".into()),
    ],
    debug: false,
}
```

Set `debug: true` to also emit OTEL spans to stdout, showing trace IDs, span IDs, timing, and all attributes.

## Instrumentation

### Events (logs)

Use the re-exported macros for logging:

```rust
use hermes_instrumentation::{trace, debug, info, warn, error};

info!(user_id = 42, "User logged in");
error!(error = %err, "Request failed");
```

### Spans

Create spans explicitly at call sites using `.instrument()` for async code:

```rust
use hermes_instrumentation::{info_span, Instrument};

async {
    // work here
}
.instrument(info_span!("operation_name", key = "value"))
.await;
```

For sync code, use `span.in_scope()`:

```rust
use hermes_instrumentation::info_span;

info_span!("sync_operation").in_scope(|| {
    // work here
});
```

The `#[instrument]` attribute macro is also available for automatic function instrumentation:

```rust
use hermes_instrumentation::instrument;

#[instrument]
async fn my_function(id: u32) {
    // automatically creates a span named "my_function"
}
```

## Shutdown

For long-running services, the tracer provider will automatically flush and shutdown when the process exits. For short-lived processes or tests, you can optionally call `shutdown()` to ensure all spans are exported:

```rust
hermes_instrumentation::shutdown();
```

## Examples

Run the examples to see telemetry in action:

```sh
# Console output
cargo run -p hermes-instrumentation --example console

# OTLP over gRPC (for Jaeger, OTel Collector)
cargo run -p hermes-instrumentation --example otlp_grpc

# OTLP over HTTP (for Axiom, Grafana Cloud)
cargo run -p hermes-instrumentation --example otlp_http
```
