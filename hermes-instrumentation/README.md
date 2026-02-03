# hermes-instrumentation

Unified telemetry for the Hermes ecosystem.

## Overview

This crate provides a single dependency for all observability needs across Hermes services. It wraps the `tracing` crate ecosystem and provides:

- Automatic namespace prefixing for spans (e.g., `ipfs-cache.fetch_content`)
- Console and Sentry backend support (via OpenTelemetry spans)
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

Initialize telemetry **before** creating the tokio runtime so the global subscriber is set once and spans aren't missed:

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

**Why?** The global tracing subscriber can only be set once, and setting it before the runtime avoids subtle initialization order issues.

## Backends

### Console

Outputs formatted logs to stdout with namespace-prefixed spans:

```rust
Backend::Console
```

### Sentry

Export spans to Sentry using the Sentry OpenTelemetry integration:

```rust
Backend::Sentry {
    dsn: "https://...@o0.ingest.sentry.io/0".into(),
    traces_sample_rate: 1.0,
    send_default_pii: false,
    environment: Some("production".into()),
    release: Some("my-service@1.2.3".into()),
    debug: false,
    axiom: None, // Or use AxiomConfig::from_env() for Axiom export
}
```

Set `debug: true` to also emit spans to stdout.

### Axiom (Optional)

Export traces to Axiom for 100% trace storage (Sentry uses server-side sampling):

```rust
use hermes_instrumentation::AxiomConfig;

Backend::Sentry {
    // ... other fields ...
    axiom: AxiomConfig::from_env(), // Reads AXIOM_TOKEN and AXIOM_DATASET
}
```

Set `AXIOM_TOKEN` to enable. Dataset defaults to `hermes-pipeline` if `AXIOM_DATASET` is not set.

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

# Sentry (requires SENTRY_DSN)
cargo run -p hermes-instrumentation --example sentry
```
