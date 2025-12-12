# Atlas

Graph processing system for computing canonical space topology.

## Overview

Atlas consumes space topology events from hermes-relay and computes:

1. **Transitive Graph** - All spaces reachable from a given root via explicit edges
2. **Canonical Graph** - The subset of spaces that are "canonical" (trusted) based on reachability from the root space

The canonical graph is published to Kafka for downstream consumers.

## Local Development

### Using Docker Compose (recommended)

Start the full stack:

```bash
cd hermes
docker-compose up
```

This starts Kafka, Kafka UI, hermes-processor, and atlas together.

### Running Individually

Start Kafka:

```bash
cd hermes
docker-compose up kafka kafka-ui
```

Run atlas:

```bash
KAFKA_BROKER=localhost:9092 KAFKA_TOPIC=topology.canonical cargo run -p atlas
```

Access Kafka UI at http://localhost:8080 to view messages.

## Configuration

| Environment Variable | Required | Default | Description |
|---------------------|----------|---------|-------------|
| `KAFKA_BROKER` | No | `localhost:9092` | Kafka bootstrap server address |
| `KAFKA_TOPIC` | No | `topology.canonical` | Topic to publish canonical graph updates |
| `KAFKA_USERNAME` | No | - | SASL username for managed Kafka authentication |
| `KAFKA_PASSWORD` | No | - | SASL password for managed Kafka authentication |

### Authentication

When `KAFKA_USERNAME` and `KAFKA_PASSWORD` are both set, the producer automatically enables SASL/SSL authentication (required for DigitalOcean Managed Kafka). When unset, plaintext connections are used (for local development).

## Architecture

```
hermes-relay (StreamSource::mock() or StreamSource::live())
        │
        ▼
┌───────────────────────────────────────┐
│              Atlas                     │
│                                       │
│  ┌─────────────┐    ┌──────────────┐  │
│  │ GraphState  │───►│ Transitive   │  │
│  │             │    │ Processor    │  │
│  └─────────────┘    └──────┬───────┘  │
│                            │          │
│                     ┌──────▼───────┐  │
│                     │  Canonical   │  │
│                     │  Processor   │  │
│                     └──────┬───────┘  │
│                            │          │
└────────────────────────────┼──────────┘
                             ▼
                   topology.canonical topic
```

### Stream Source Configuration

Atlas uses `hermes-relay`'s `StreamSource` to choose between mock and live data:

```rust
use hermes_relay::{Sink, StreamSource, HermesModule};

// Development: mock data (all test topology events in one block)
sink.run(StreamSource::mock()).await?;

// Production: live substream
let source = StreamSource::live(
    "https://substreams.example.com",
    HermesModule::Actions,
    start_block,
    end_block,
);
sink.run(source).await?;
```

## Graph Concepts

### Explicit Edges
Direct trust relationships between spaces:
- **Verified** - Strong trust (grants canonicality)
- **Related** - Weaker association

### Topic Edges
Indirect relationships via shared topics:
- A space can "subscribe" to a topic
- All spaces announcing that topic become reachable

### Canonical Graph
A space is canonical if:
1. It is the root space, OR
2. It is reachable from the root via explicit edges only

Topic edges can add subtrees to the canonical graph, but only if the target spaces are themselves canonical.

## Test Topology

Atlas processes a deterministic topology with:
- 11 canonical spaces (reachable from Root)
- 7 non-canonical spaces (isolated islands)
- 14 explicit edges + 5 topic edges

## Building

```bash
cargo build -p atlas --release
```

## Benchmarks

Run performance benchmarks:

```bash
cargo bench -p atlas
```

## Documentation

See the `docs/` directory for detailed architecture documentation:

- [Algorithm Overview](docs/algorithm-overview.md) - High-level data flow
- [Graph Concepts](docs/graph-concepts.md) - Core concepts and terminology
- [Canonical Graph Implementation](docs/canonical-graph-implementation.md) - How canonical computation works
- [Transitive Graph Implementation](docs/transitive-graph-implementation.md) - BFS traversal and caching

## Related Documents

- [Hermes Architecture](../docs/hermes-architecture.md) - Event streaming system that feeds Atlas
