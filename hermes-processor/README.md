# Hermes Processor

Processes events from the mock-substream and publishes them to Kafka as Hermes protobuf messages.

## Overview

The hermes-processor consumes deterministic topology events from the shared `mock-substream` crate and transforms them into Hermes protobuf messages:

- `SpaceCreated` → `HermesCreateSpace` → `space.creations` topic
- `TrustExtended` → `HermesSpaceTrustExtension` → `space.trust.extensions` topic  
- `EditPublished` → `HermesEdit` → `knowledge.edits` topic

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

Run hermes-processor:

```bash
KAFKA_BROKER=localhost:9092 cargo run -p hermes-processor
```

Access Kafka UI at http://localhost:8080 to view messages.

## Configuration

| Environment Variable | Required | Default | Description |
|---------------------|----------|---------|-------------|
| `KAFKA_BROKER` | No | `localhost:9092` | Kafka bootstrap server address |
| `KAFKA_USERNAME` | No | - | SASL username for managed Kafka authentication |
| `KAFKA_PASSWORD` | No | - | SASL password for managed Kafka authentication |

### Authentication

When `KAFKA_USERNAME` and `KAFKA_PASSWORD` are both set, the producer automatically enables SASL/SSL authentication (required for DigitalOcean Managed Kafka). When unset, plaintext connections are used (for local development).

## Event Flow

```
mock-substream crate
        │
        ▼
┌───────────────────┐
│ hermes-processor  │
│                   │
│ SpaceCreated ────►│──► space.creations
│ TrustExtended ───►│──► space.trust.extensions
│ EditPublished ───►│──► knowledge.edits
└───────────────────┘
```

## Test Topology

The processor emits a deterministic topology with:
- 18 spaces (11 canonical + 7 non-canonical)
- 19 trust extensions (14 explicit + 5 topic-based)
- 6 edits with GRC-20 operations

## Building

```bash
cargo build -p hermes-processor --release
```

## Producer Settings

- **Compression**: ZSTD
- **Batching**: Up to 10,000 messages per batch
- **Timeout**: 5 seconds message timeout
- **Buffer**: 1GB queue buffer
