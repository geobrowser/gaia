# Hermes Kafka

Shared Kafka producer library for Hermes transformer binaries.

## Purpose

hermes-kafka provides common Kafka configuration used by all services that interact with Kafka:

- **Producer creation** — `create_producer` and `create_producer_with_config` with sensible defaults (zstd compression, idempotent writes, `acks=all`)
- **SASL/SSL authentication** — automatic when `KAFKA_USERNAME` and `KAFKA_PASSWORD` are set (for DigitalOcean Managed Kafka)
- **Environment isolation** — `get_topic_prefix` and `prefixed_topic` for staging vs production topic separation
- **Timeout configuration** — configurable message and send timeouts

## Consumers

- [hermes-pipeline](../hermes-pipeline/) — uses producer APIs to publish events
- [atlas](../atlas/) — uses producer APIs to publish graph updates
- [kg-indexer](../kg-indexer/) — uses topic prefix utilities for environment isolation
- [search-indexer](../search-indexer/) — uses topic prefix utilities for environment isolation

## Key Types

```rust
use hermes_kafka::{create_producer, ProducerConfig, FutureProducer};

// Simple: create from broker address (reads SASL creds from env)
let producer = create_producer("localhost:9092", "my-service")?;

// Explicit: full configuration
let config = ProducerConfig::from_env("localhost:9092", "my-service");
let producer = create_producer_with_config(&config)?;

// Environment isolation: prefix topics for staging
let prefix = hermes_kafka::get_topic_prefix();  // "staging." or ""
let topic = hermes_kafka::prefixed_topic(prefix, "knowledge.edits");
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `KAFKA_BROKER` | `localhost:9092` | Kafka bootstrap server |
| `KAFKA_USERNAME` | — | SASL username (enables SASL/SSL) |
| `KAFKA_PASSWORD` | — | SASL password |
| `KAFKA_SSL_CA_PEM` | — | Custom CA cert (PEM format) |
| `KAFKA_MESSAGE_TIMEOUT_MS` | `30000` | Producer delivery timeout |
| `KAFKA_SEND_TIMEOUT_MS` | `KAFKA_MESSAGE_TIMEOUT_MS` | Send queue timeout |
| `ENVIRONMENT` | — | `staging` or `production` (for topic prefix) |

## Documentation

- [Hermes Architecture](../docs/architecture.md) — overall system design
- [Kafka Environment Isolation Plan](../docs/plans/2026-02-02-feat-kafka-environment-isolation-plan.md) — staging/production topic isolation
