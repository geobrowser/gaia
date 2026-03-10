# kg-indexer

Consumes Hermes Kafka events and indexes them into PostgreSQL for the Knowledge Graph.

## Overview

kg-indexer is part of the Hermes event pipeline. It reads events from multiple Kafka topics, buffers them per blockchain block, and writes each block's events as a single PostgreSQL transaction to preserve ordering. A background tally worker asynchronously computes proposal vote tallies, decoupled from the main write path.

```
hermes-pipeline → Kafka topics → kg-indexer → PostgreSQL → API
                                      ↑
                                 hermes.blocks
                              (batch close signal)
```

Block-level buffering relies on `sequence` and `is_last` fields from hermes-pipeline (see [hermes-pipeline ADR-001](../hermes-pipeline/docs/DECISIONS.md)). When `is_last` arrives for a block, all buffered events are sorted by sequence and processed in a single transaction. If `is_last` never arrives, stale block detection kicks in after a configurable timeout.

## Topics Consumed

| Topic | Description |
|-------|-------------|
| `hermes.blocks` | Block summary — used as batch close signal |
| `knowledge.edits` | Entity and relation CRUD operations |
| `space.creations` | New space registrations |
| `space.membership` | Editor/member additions and removals |
| `space.trust.extensions` | Subspace trust changes (verified, related, topic) |
| `space.governance` | Proposals created, voted, executed |

## Configuration

All environment variables are documented in [`.env.example`](.env.example). Key variables:

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes | — | PostgreSQL connection string |
| `KAFKA_BROKER` | No | `localhost:9092` | Kafka bootstrap server address |
| `KAFKA_GROUP_ID` | No | `kg-indexer` | Consumer group ID |
| `ENVIRONMENT` | Conditional | — | `staging` or `production` — sets Kafka topic prefix (read by hermes-kafka, required in production) |
| `BLOCK_STALE_TIMEOUT_MS` | No | `1000` | Stale block detection timeout in ms |
| `TALLY_WORKER_INTERVAL_MS` | No | `5000` | Tally worker run interval in ms |
| `TALLY_WORKER_BATCH_SIZE` | No | `1000` | Tally worker batch size |
| `LOG_EVENT_IDS` | No | `false` | Emit per-event `event-id` logs |
| `SENTRY_DSN` | No | — | Enables Sentry telemetry when set |

For Kafka authentication (`KAFKA_USERNAME`, `KAFKA_PASSWORD`, `KAFKA_SSL_CA_PEM`), see [hermes-kafka README](../hermes-kafka/README.md). Full Sentry configuration is documented in [`.env.example`](.env.example).

## Local Development

Prerequisites: PostgreSQL and Kafka running locally.

```bash
# Start Kafka (from repo root)
docker-compose -f hermes/docker-compose.yaml up -d

# Run kg-indexer (from repo root)
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/gaia KAFKA_BROKER=localhost:9092 cargo run -p kg-indexer
```

## Documentation

- [GOTCHAS.md](docs/GOTCHAS.md) — Buffering behavior and operational details
- [DECISIONS.md](docs/DECISIONS.md) — Architecture decision records
- [hermes-pipeline](../hermes-pipeline/) — Upstream event producer
- [hermes-kafka](../hermes-kafka/) — Shared Kafka consumer/producer library
- [hermes-schema](../hermes-schema/) — Protobuf message definitions
