# hermes-instrumentation

Unified telemetry for the Hermes ecosystem.

## Overview

`hermes-instrumentation` provides a single dependency for all observability needs across Hermes services. It wraps the `tracing` crate ecosystem and provides:

- Automatic namespace prefixing for spans
- Console and OpenTelemetry (OTLP) backend support
- Re-exported tracing macros for convenience

## Usage

```rust
use hermes_instrumentation::{init, info, instrument, Config, Backend};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize telemetry at startup
    hermes_instrumentation::init(Config {
        namespace: "my-service".to_string(),
        backend: Backend::Console,
    })?;

    info!("Service started");
    
    process_work().await
}

#[instrument]
async fn process_work() -> anyhow::Result<()> {
    // This span will be named "my-service.process_work"
    info!("Processing...");
    Ok(())
}
```

## Configuration

### Console Backend

Logs spans and events to stdout:

```rust
Backend::Console
```

### OTLP Backend

Exports telemetry via OpenTelemetry Protocol:

```rust
Backend::Otlp {
    endpoint: "http://localhost:4317".to_string(),
}
```

The OTLP endpoint can point to:
- OpenTelemetry Collector
- Jaeger (with OTLP receiver)
- Axiom (direct OTLP ingestion)
- Any OTLP-compatible backend

## Architecture

See [docs/plans/0001-telemetry-crate.md](docs/plans/0001-telemetry-crate.md) for the full design document.
