# kg-indexer

Consumes Hermes Kafka events and indexes them into PostgreSQL for the Knowledge Graph.

## Overview

kg-indexer reads Hermes event topics, buffers events per block, and writes them in a single DB transaction to preserve block ordering. It also consumes canonical block summaries (`hermes.blocks`) to close batches deterministically even when `is_last` is missing on the topics it reads.

## Topics consumed

- `hermes.blocks` (block summary)
- `knowledge.edits`
- `space.creations`
- `space.membership`
- `space.trust.extensions`
- `space.governance`

## Configuration

Environment variables:

- `DATABASE_URL` (required)
- `KAFKA_BROKER` (default: `localhost:9092`)
- `KAFKA_GROUP_ID` (default: `kg-indexer`)
- `BLOCK_STALE_TIMEOUT_MS` (default: `250`)
- `KAFKA_USERNAME` / `KAFKA_PASSWORD` (optional SASL)
- `KAFKA_SSL_CA_PEM` (optional)

## Logging and tracing

Canonical batch logs:

- `kg_indexer.batch_start` — batch intent
- `kg_indexer.batch_end` — batch outcome
- `kg_indexer.event_error` — per-event failures

Debug flag:

- `LOG_EVENT_IDS=true` — emits per-event `event-id` logs (off by default)

OTEL:

- If `OTEL_URL` is set, spans are exported via OTLP (HTTP)
- Otherwise logs go to stdout (console backend)
- `OTEL_DEBUG=true` mirrors spans to stdout when OTLP is enabled

## Development

Run locally:

```bash
cargo run -p kg-indexer
```

## Notes

See `docs/GOTCHAS.md` for buffering behavior and other operational details.
