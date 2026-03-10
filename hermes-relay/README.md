# Hermes Relay

Shared library for connecting Hermes transformers to the blockchain data source via hermes-substream.

## Purpose

hermes-relay provides the infrastructure that all Hermes transformer binaries share:

- **Stream source abstraction** — `StreamSource` for choosing between mock and live data
- **Sink traits** — `Sink` and `PreprocessedSink` for consuming block-scoped data
- **Mock testing** — `MockSource` and `mock_events` for deterministic test data
- **Action constants** — type constants for client-side filtering of raw blockchain actions

## Consumers

- [hermes-pipeline](../hermes-pipeline/) — all blockchain event processing
- [atlas](../atlas/) — canonical graph computation
- [hermes-ipfs-cache](../hermes-ipfs-cache/) — IPFS content caching

## Key Types

```rust
use hermes_relay::{Sink, StreamSource, HermesModule};

// Implement Sink to consume blockchain events
impl Sink for MyTransformer {
    type Error = anyhow::Error;
    async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
        // ...
    }
}

// Development: mock data
transformer.run(StreamSource::mock()).await?;

// Production: live substream
transformer.run(StreamSource::live(endpoint, HermesModule::Actions, start, end)).await?;
```

See the [module-level docs](src/lib.rs) for full API documentation and examples.

## Documentation

- [ADR 0001: Multiple Substreams Module Consumers](docs/decisions/0001-multiple-substreams-modules-consumers.md) — why we filter client-side
- [Hermes Architecture](../docs/architecture.md) — overall system design
