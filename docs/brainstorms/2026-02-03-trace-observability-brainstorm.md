---
date: 2026-02-03
topic: trace-observability-sentry-sampling
---

# Trace Observability: Solving Sentry's Sampling Gap

## Problem

We export traces to Sentry at 100% sample rate (`SENTRY_TRACES_SAMPLE_RATE=1.0`), but **Sentry's UI doesn't show all traces**. When debugging issues by block number, the specific trace we need may not be visible.

### Root Cause: Sentry's Server-Side Dynamic Sampling

Research confirmed that Sentry applies **two layers of sampling**:

1. **SDK-side sampling** (our `tracesSampleRate` setting) - controls what we *send*
2. **Server-side Dynamic Sampling** - controls what Sentry *stores*

Even when sending 100% of traces, Sentry's server-side Dynamic Sampling may drop traces before storage. From their docs:

> "Dynamic Sampling employs advanced sampling techniques to **retain a representative sample** of the data you send to Sentry."

This is particularly aggressive for high-volume, repetitive transaction types (like block processing), which are exactly the traces we need to debug.

### Impact

- Cannot find specific traces when debugging by block number
- Broken subtraces and orphan traces in the UI
- Aggregate metrics are accurate (via weighted extrapolation), but individual trace access is unreliable

## What We're Building

A **dual-destination observability approach** with **opt-in Axiom export** at the `hermes-instrumentation` level:

1. **Canonical logs with trace context in K8s** - Structured JSON logs with trace IDs, block numbers, and key context directly to stdout (captured by K8s logging)
2. **Optional Axiom export in `hermes-instrumentation`** - Services that need real-time trace lookup can opt-in to Axiom export alongside Sentry

This preserves Sentry's value (error correlation, alerts, session replay) while ensuring 100% trace availability for services that need it.

## Why This Approach

### Approaches Considered

| Approach | Pros | Cons |
|----------|------|------|
| **A: Dual backend (Tempo/Jaeger)** | Full trace storage, native Grafana integration | New infrastructure to deploy and maintain |
| **B: Enhanced logging only** | Simple, works with existing infra | Can't visualize full trace waterfalls |
| **C: Sentry Enterprise** | Single vendor | Expensive, may not solve the problem |
| **D: Axiom + K8s logs (chosen)** | Real-time querying, already used in indexer, minimal new infra | Two UIs for traces |

### Why Axiom + K8s Logs

1. **Axiom already integrated** - The `indexer` service already exports to Axiom; we can extend this pattern
2. **Real-time querying** - Axiom excels at real-time log/trace search, perfect for "find trace by block number"
3. **Minimal new infrastructure** - No new services to deploy; just configuration changes
4. **K8s logs as backup** - Canonical structured logs ensure we can always grep for context even without Axiom
5. **Keeps Sentry's value** - Continue using Sentry for error-centric debugging, alerts, and its excellent UX

## Key Decisions

### 1. Opt-in Axiom Export in `hermes-instrumentation`

Add an **optional** `axiom` field to the `Backend::Sentry` variant:

```rust
Backend::Sentry {
    dsn: String,
    traces_sample_rate: f32,
    // ... existing fields ...
    
    /// Optional Axiom configuration for real-time trace export.
    /// When set, traces are exported to both Sentry and Axiom.
    axiom: Option<AxiomConfig>,
}

pub struct AxiomConfig {
    /// Axiom API token
    pub token: String,
    /// Dataset name (e.g., "gaia.kg-indexer")
    pub dataset: String,
}
```

**Rationale:** 
- Not every service needs real-time traces (e.g., `hermes-ipfs-cache` probably doesn't)
- Services opt-in by providing `AXIOM_TOKEN` and `AXIOM_DATASET` env vars
- Change is contained in `hermes-instrumentation`; services just add env vars
- Follows the existing pattern of optional configuration

### 2. Canonical Log Format for K8s

All services will emit structured JSON logs with consistent fields:

```json
{
  "timestamp": "2026-02-03T12:00:00Z",
  "level": "info",
  "service": "kg-indexer",
  "trace_id": "abc123...",
  "span_id": "def456...",
  "block_number": 12345678,
  "message": "Processing block",
  "duration_ms": 150
}
```

**Rationale:** Even without Axiom, we can `kubectl logs | jq` to find traces by block number.

### 3. Keep Sentry as Primary Error/Alert System

Sentry remains the primary destination for:
- Error tracking and alerting
- Session replay correlation
- Release health monitoring

**Rationale:** Sentry's error UX and alert system are valuable; we're augmenting, not replacing.

### 4. Service Opt-in Strategy

| Service | Needs Real-time Traces? | Axiom Export |
|---------|------------------------|--------------|
| `kg-indexer` | Yes (block debugging) | Opt-in |
| `hermes-pipeline` | Yes (block debugging) | Opt-in |
| `atlas` | Maybe | TBD |
| `search-indexer` | Probably not | No |
| `hermes-ipfs-cache` | No | No |
| `vote-indexer` | Maybe | TBD |

**Rationale:** Start with the services where we actually need to debug by block number.

## Axiom Integration Details

### OTLP Configuration

Axiom supports native OTLP/HTTP — no special SDK needed:

| Setting | Value |
|---------|-------|
| **Endpoint** | `https://api.axiom.co/v1/traces` |
| **Protocol** | HTTP/protobuf |
| **Headers** | `Authorization: Bearer <AXIOM_TOKEN>`, `X-Axiom-Dataset: <DATASET>` |

### Dataset Strategy

**Decision: Single unified dataset** — `gaia-traces`

All services write to the same dataset with `service.name` as a field. This enables:
- Cross-service trace queries (e.g., "find all traces for block 12345678")
- Simpler credential management (one API token for all services)
- Easier dashboard creation

### Rust Implementation

```rust
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use std::collections::HashMap;

let mut headers = HashMap::new();
headers.insert("Authorization".to_string(), format!("Bearer {}", axiom_token));
headers.insert("X-Axiom-Dataset".to_string(), "gaia-traces".to_string());

let axiom_exporter = SpanExporter::builder()
    .with_http()
    .with_endpoint("https://api.axiom.co/v1/traces")
    .with_headers(headers)
    .build()?;
```

### Crate Features Required

Add to `hermes-instrumentation/Cargo.toml`:
```toml
opentelemetry-otlp = { version = "0.x", features = ["http-proto", "reqwest-client"] }
```

## Open Questions

1. **Retention policy** - How long to retain 100% traces in Axiom? (Cost consideration)

## Decisions Made

- **API telemetry**: K8s logging is sufficient for the API; no Axiom export needed for TypeScript services at this time.

## References

- [Sentry Dynamic Sampling docs](https://docs.sentry.io/organization/dynamic-sampling/)
- [Sentry Trace Explorer - How Sampling Affects Queries](https://docs.sentry.io/product/explore/traces/#how-sampling-affects-queries-in-trace-explorer)
- Existing Axiom integration: `indexer/src/main.rs`
- API Sentry plan: `docs/plans/api-sentry-instrumentation.md`

## Implementation Sketch

### `hermes-instrumentation` Changes

**1. Add `AxiomConfig` struct:**

```rust
/// Optional Axiom configuration for real-time trace export.
#[derive(Debug, Clone)]
pub struct AxiomConfig {
    /// Axiom API token (from AXIOM_TOKEN env var)
    pub token: String,
    /// Dataset name (default: "gaia-traces")
    pub dataset: String,
}

impl AxiomConfig {
    /// Create from environment variables if AXIOM_TOKEN is set.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("AXIOM_TOKEN").ok()?;
        let dataset = std::env::var("AXIOM_DATASET")
            .unwrap_or_else(|_| "gaia-traces".to_string());
        Some(Self { token, dataset })
    }
}
```

**2. Update `Backend::Sentry` variant:**

```rust
Backend::Sentry {
    dsn: String,
    traces_sample_rate: f32,
    send_default_pii: bool,
    environment: Option<String>,
    release: Option<String>,
    debug: bool,
    /// Optional Axiom export for real-time trace access.
    /// When set, traces are exported to both Sentry AND Axiom.
    axiom: Option<AxiomConfig>,  // NEW
}
```

**3. In `init_sentry()`, add Axiom span exporter:**

```rust
// If Axiom configured, add OTLP exporter
if let Some(axiom) = axiom_config {
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), format!("Bearer {}", axiom.token));
    headers.insert("X-Axiom-Dataset".to_string(), axiom.dataset);
    
    let axiom_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint("https://api.axiom.co/v1/traces")
        .with_headers(headers)
        .build()?;
    
    provider_builder = provider_builder.with_batch_exporter(axiom_exporter);
}
```

**4. Update `TelemetryGuard`** — no changes needed; the `SdkTracerProvider::shutdown()` already handles all registered exporters.

### Service Changes

For services that opt-in (e.g., `kg-indexer`):

1. **Add K8s secret** with `AXIOM_TOKEN` (shared across services, or per-service)
2. **Update service startup** to pass `AxiomConfig::from_env()`:

```rust
let backend = Backend::Sentry {
    dsn: env::var("SENTRY_DSN")?,
    traces_sample_rate: 1.0,
    // ... other fields ...
    axiom: AxiomConfig::from_env(),  // Opt-in via env var
};
```

### K8s Logging

- Ensure `fmt_layer` uses JSON format when `debug: false` (for machine-parseable K8s logs)
- Or: Add a separate `json_logs: bool` config option

## Next Steps

→ `/workflows:plan` for implementation details covering:
- `hermes-instrumentation` changes (AxiomConfig, dual export)
- K8s logging configuration for canonical JSON logs
- Environment variable standardization
- Which services to enable first
