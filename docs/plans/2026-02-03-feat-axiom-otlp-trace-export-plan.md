---
title: "feat: Add optional Axiom OTLP trace export to hermes-instrumentation"
type: feat
date: 2026-02-03
brainstorm: docs/brainstorms/2026-02-03-trace-observability-brainstorm.md
---

# feat: Add Optional Axiom OTLP Trace Export

## Overview

Add opt-in Axiom trace export to `hermes-instrumentation` so services can send traces to both Sentry (for error correlation) and Axiom (for 100% trace storage). This solves the problem of Sentry's server-side Dynamic Sampling dropping traces we need for debugging by block number.

## Problem Statement

Sentry applies server-side Dynamic Sampling even when our SDK sends 100% of traces. This means:
- We cannot reliably find specific traces when debugging by block number
- High-volume, repetitive transactions (like block processing) are aggressively sampled
- Aggregate metrics are accurate, but individual trace access is unreliable

## Proposed Solution

Add an optional `axiom: Option<AxiomConfig>` field to `Backend::Sentry`. When configured via environment variables (`AXIOM_TOKEN`), traces are exported to both Sentry and Axiom. Services opt-in by simply setting the env var.

## Technical Approach

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Service (e.g., kg-indexer)               │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              hermes-instrumentation                  │   │
│  │                                                      │   │
│  │  ┌──────────────────────────────────────────────┐   │   │
│  │  │           SdkTracerProvider                   │   │   │
│  │  │                                               │   │   │
│  │  │  ┌─────────────┐    ┌─────────────────────┐  │   │   │
│  │  │  │   Sentry    │    │   OTLP Batch        │  │   │   │
│  │  │  │   Tracing   │    │   Exporter          │  │   │   │
│  │  │  │   Layer     │    │   (Axiom)           │  │   │   │
│  │  │  └──────┬──────┘    └──────────┬──────────┘  │   │   │
│  │  │         │                      │              │   │   │
│  │  └─────────┼──────────────────────┼──────────────┘   │   │
│  └────────────┼──────────────────────┼──────────────────┘   │
└───────────────┼──────────────────────┼──────────────────────┘
                │                      │
                ▼                      ▼
        ┌───────────────┐      ┌───────────────┐
        │    Sentry     │      │    Axiom      │
        │  (sampled)    │      │  (100%)       │
        └───────────────┘      └───────────────┘
```

### Implementation Phases

#### Phase 1: Core Implementation in hermes-instrumentation

**Files to modify:**

1. **hermes-instrumentation/Cargo.toml** - Add OTLP dependency
2. **hermes-instrumentation/src/config.rs** - Add `AxiomConfig` struct
3. **hermes-instrumentation/src/init.rs** - Add OTLP exporter logic

**Tasks:**

- [ ] Add `opentelemetry-otlp` dependency with `http-proto` and `reqwest-client` features
- [ ] Create `AxiomConfig` struct with `token` and `dataset` fields
- [ ] Add `AxiomConfig::from_env()` constructor
- [ ] Add `axiom: Option<AxiomConfig>` field to `Backend::Sentry` variant
- [ ] In `init_sentry()`, create and register OTLP batch exporter when Axiom is configured
- [ ] Add startup log line confirming Axiom export status
- [ ] Validate config at startup (fail if AXIOM_TOKEN is empty string)

**Estimated effort:** 2-3 hours

#### Phase 2: Enable for kg-indexer

**Files to modify:**

1. **kg-indexer/src/main.rs** - Pass `AxiomConfig` to backend
2. **kg-indexer/k8s/staging/kg-indexer-secrets.yaml** - Add AXIOM_TOKEN
3. **kg-indexer/k8s/production/kg-indexer-secrets.yaml** - Add AXIOM_TOKEN

**Tasks:**

- [ ] Update `build_telemetry_config()` to include `axiom: AxiomConfig::from_env()`
- [ ] Create staging K8s secret with AXIOM_TOKEN
- [ ] Create production K8s secret with AXIOM_TOKEN
- [ ] Deploy to staging and verify traces appear in Axiom
- [ ] Deploy to production

**Estimated effort:** 1 hour

#### Phase 3: Enable for hermes-pipeline

**Files to modify:**

1. **hermes/hermes-pipeline/src/main.rs** - Pass `AxiomConfig` to backend
2. **hermes/k8s/staging/hermes-pipeline-secrets.yaml** - Add AXIOM_TOKEN
3. **hermes/k8s/production/hermes-pipeline-secrets.yaml** - Add AXIOM_TOKEN

**Tasks:**

- [ ] Update `build_telemetry_config()` to include `axiom: AxiomConfig::from_env()`
- [ ] Add AXIOM_TOKEN to K8s secrets
- [ ] Deploy and verify

**Estimated effort:** 30 minutes

## Acceptance Criteria

### Functional Requirements

- [ ] When `AXIOM_TOKEN` is set, traces are exported to both Sentry and Axiom
- [ ] When `AXIOM_TOKEN` is not set, behavior is unchanged (Sentry-only)
- [ ] Default dataset is `gaia-traces` when `AXIOM_DATASET` is not set
- [ ] Traces in Axiom include `service.name` for filtering
- [ ] Service startup fails if `AXIOM_TOKEN` is set but empty

### Non-Functional Requirements

- [ ] OTLP uses batch exporter (non-blocking)
- [ ] Axiom token is never logged
- [ ] Shutdown flushes pending spans to Axiom
- [ ] Runtime export failures use OTLP defaults (retry, then drop)

### Quality Gates

- [ ] `cargo check` passes for hermes-instrumentation
- [ ] `cargo clippy` passes
- [ ] Existing services using hermes-instrumentation still compile
- [ ] Manual verification: traces appear in Axiom for kg-indexer

## Technical Specifications

### New Types (config.rs)

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
    /// Returns None if AXIOM_TOKEN is not set.
    /// Panics if AXIOM_TOKEN is set but empty (configuration error).
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("AXIOM_TOKEN").ok()?;
        if token.is_empty() {
            panic!("AXIOM_TOKEN is set but empty - this is a configuration error");
        }
        let dataset = std::env::var("AXIOM_DATASET")
            .unwrap_or_else(|_| "gaia-traces".to_string());
        Some(Self { token, dataset })
    }
}
```

### Updated Backend Enum (config.rs)

```rust
pub enum Backend {
    Console,
    Sentry {
        dsn: String,
        traces_sample_rate: f32,
        send_default_pii: bool,
        environment: Option<String>,
        release: Option<String>,
        debug: bool,
        /// Optional Axiom export for 100% trace storage.
        axiom: Option<AxiomConfig>,
    },
}
```

### OTLP Exporter Setup (init.rs)

```rust
// In init_sentry(), after provider_builder is created:

if let Some(ref axiom) = axiom {
    use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
    use std::collections::HashMap;

    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), format!("Bearer {}", axiom.token));
    headers.insert("X-Axiom-Dataset".to_string(), axiom.dataset.clone());

    let axiom_exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint("https://api.axiom.co/v1/traces")
        .with_headers(headers)
        .build()
        .map_err(|e| Error::OpenTelemetry(format!("Failed to create Axiom exporter: {}", e)))?;

    provider_builder = provider_builder.with_batch_exporter(axiom_exporter);
    
    eprintln!(
        "Telemetry initialized: service.name={} sentry axiom={}",
        namespace, axiom.dataset
    );
} else {
    eprintln!("Telemetry initialized: service.name={} sentry", namespace);
}
```

### Cargo.toml Addition

```toml
[dependencies]
opentelemetry-otlp = { version = "0.31", features = ["http-proto", "reqwest-client"] }
```

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `AXIOM_TOKEN` | No | - | Axiom API token. When set, enables Axiom export. |
| `AXIOM_DATASET` | No | `gaia-traces` | Axiom dataset name for traces. |

### K8s Secret Template

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: axiom-credentials
  namespace: knowledge
type: Opaque
stringData:
  AXIOM_TOKEN: "<token>"
```

## Error Handling

| Scenario | Behavior |
|----------|----------|
| `AXIOM_TOKEN` not set | Sentry-only mode (silent opt-out) |
| `AXIOM_TOKEN` empty string | Panic at startup (configuration error) |
| OTLP exporter creation fails | Propagate error, fail service startup |
| Runtime export failure | OTLP defaults: retry with backoff, then drop |
| Shutdown with pending spans | Flush attempt with timeout |

## Dependencies & Risks

### Dependencies

- `opentelemetry-otlp` crate (well-maintained, part of opentelemetry-rust)
- Axiom account with API token and traces dataset

### Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| OTLP crate version incompatibility | Low | Medium | Pin to same minor version as existing otel deps |
| Axiom rate limiting | Low | Low | Batch exporter handles backpressure |
| Token exposure in logs | Low | High | Never log token, sanitize errors |

## Success Metrics

1. **Functional:** Can find traces in Axiom by block number for kg-indexer
2. **Reliability:** No service startup failures due to Axiom integration
3. **Performance:** No measurable latency impact (batch export is async)

## References

### Internal

- Brainstorm: `docs/brainstorms/2026-02-03-trace-observability-brainstorm.md`
- Existing telemetry: `hermes-instrumentation/src/init.rs`
- Service telemetry pattern: `hermes/hermes-pipeline/src/main.rs:746-787`

### External

- [Axiom OTLP Documentation](https://axiom.co/docs/send-data/opentelemetry)
- [opentelemetry-otlp crate](https://docs.rs/opentelemetry-otlp)
- [Sentry Dynamic Sampling](https://docs.sentry.io/organization/dynamic-sampling/)
