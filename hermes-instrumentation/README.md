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
use hermes_instrumentation::{init, info, info_span, Config, Backend, Instrument};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize with console output
    hermes_instrumentation::init(Config {
        namespace: "my-service",
        backend: Backend::Console,
    })?;

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

## Backends

### Console

Outputs formatted logs to stdout with namespace-prefixed spans:

```rust
Backend::Console
```

### OTLP

Exports traces via OpenTelemetry Protocol (gRPC) to any OTLP-compatible backend (Jaeger, Grafana Tempo, Axiom, etc.):

```rust
Backend::Otlp {
    endpoint: "http://localhost:4317",
}
```

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

For sync code, use `span.enter()`:

```rust
use hermes_instrumentation::info_span;

let span = info_span!("sync_operation");
let _guard = span.enter();
// work here - span active until _guard is dropped
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

When using the OTLP backend, call `shutdown()` before exit to flush pending spans:

```rust
hermes_instrumentation::shutdown();
```
