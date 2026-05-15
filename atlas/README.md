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
| `ROOT_SPACE_ID` | Yes | - | Root space id as 32-char hex (16 bytes) |
| `USE_MOCK` | No | `false` | Use mock stream instead of live substreams |
| `SUBSTREAMS_ENDPOINT` | No | `https://geotest.substreams.pinax.network:443` | Live substreams endpoint |
| `SUBSTREAMS_API_TOKEN` | No | - | Optional auth token for substreams endpoint |
| `SUBSTREAMS_START_BLOCK` | No | `82655` | Start block for live stream |
| `SUBSTREAMS_END_BLOCK` | No | `u64::MAX` | End block for live stream |
| `ATLAS_CHECKPOINT_DATABASE_URL` | No | - | PostgreSQL URL for checkpoint persistence |
| `ATLAS_INDEXER_ID` | Conditional | - | Required and non-empty when checkpoint persistence is enabled |
| `ATLAS_RUNTIME_COMPATIBILITY_MARKER` | No | `atlas-v2` | Runtime marker used for checkpoint compatibility validation |
| `ATLAS_CHECKPOINT_ALLOW_FRESH_START` | No | `false` | If true, incompatible/corrupt checkpoints fall back to fresh bootstrap |
| `ATLAS_FAIL_OPEN_BOUND` | No | `10` | Max uncheckpointed blocks before Atlas pauses processing |
| `ATLAS_CHECKPOINT_RETRY_ATTEMPTS` | No | `3` | Retry attempts for checkpoint writes |
| `ATLAS_CHECKPOINT_RETRY_BACKOFF_MS` | No | `200` | Base retry backoff in milliseconds |
| `ATLAS_PAUSE_RECOVERY_MAX_ATTEMPTS` | No | `120` | Max paused recovery attempts before returning an error |
| `ATLAS_CHECKPOINT_POOL_MAX_CONNECTIONS` | No | `2` | Checkpoint Postgres pool max connections |
| `ATLAS_CHECKPOINT_POOL_MIN_CONNECTIONS` | No | `0` | Checkpoint Postgres pool min connections |
| `ATLAS_CHECKPOINT_POOL_ACQUIRE_TIMEOUT_MS` | No | `5000` | Pool acquire timeout in ms |
| `ATLAS_CHECKPOINT_POOL_IDLE_TIMEOUT_MS` | No | `60000` | Pool idle timeout in ms |
| `ATLAS_CHECKPOINT_POOL_MAX_LIFETIME_MS` | No | `1800000` | Pool max connection lifetime in ms |
| `ATLAS_CHECKPOINT_STATEMENT_TIMEOUT_MS` | No | `3000` | Per-connection PostgreSQL statement timeout in ms |

### Authentication

When `KAFKA_USERNAME` and `KAFKA_PASSWORD` are both set, the producer automatically enables SASL/SSL authentication (required for DigitalOcean Managed Kafka). When unset, plaintext connections are used (for local development).

## Architecture

```
hermes-relay (StreamSource::mock() or StreamSource::live())
        |
        v
+---------------------------------------+
|              Atlas                     |
|                                       |
|  +-------------+    +--------------+  |
|  | GraphState  |--->| Transitive   |  |
|  |             |    | Processor    |  |
|  +-------------+    +------+-------+  |
|                            |          |
|                     +------v-------+  |
|                     |  Canonical   |  |
|                     |  Processor   |  |
|                     +------+-------+  |
|                            |          |
+----------------------------+----------+
                             |
                             v
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

Diff-emission benchmark notes and reference numbers:
- [`../docs/benchmarks/atlas-diff-emission.md`](../docs/benchmarks/atlas-diff-emission.md)

## Documentation

See the `docs/` directory for detailed architecture documentation:

- [Algorithm Overview](docs/algorithm-overview.md) - High-level data flow
- [Graph Concepts](docs/graph-concepts.md) - Core concepts and terminology
- [Canonical Graph Implementation](docs/canonical-graph-implementation.md) - How canonical computation works
- [Transitive Graph Implementation](docs/transitive-graph-implementation.md) - BFS traversal and caching
- [Benchmarks](docs/benchmarks.md) - Performance benchmarks and memory usage
- [Diff Emission Benchmarks](../docs/benchmarks/atlas-diff-emission.md) - Canonical diff performance evidence

## Specifications

- [Atlas Canonical Graph Spec](../docs/specs/atlas-canonical-graph-spec.md) - Normative behavior and wire contract
- [RFC 0001: Canonical Graph Inputs](../docs/rfcs/0001-canonical-graph-inputs.md)
- [RFC 0002: Graph Diff Emission](../docs/rfcs/0002-graph-diff-emission.md)
- [Atlas Persistence Requirements](docs/agents/requirements/0001-atlas-persistence-requirements.md)
- [Persistence Rollout Checklist](docs/operations/persistence-rollout-checklist.md)

## Related Documents

- [Hermes Architecture](../docs/architecture.md) - Event streaming system that feeds Atlas
