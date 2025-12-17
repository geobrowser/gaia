# Search Indexer

Main binary for the Geo Knowledge Graph search indexer. Creates an orchestrator that handles consuming entity events from Kafka and indexing them into OpenSearch for full-text search across the Knowledge Graph.

## Quick Start

```bash
# 1. Start Kafka
cd ../hermes && docker-compose up -d kafka

# 2. Run the indexer
cd ../search-indexer
OPENSEARCH_URL=http://localhost:9200 \
KAFKA_BROKER=localhost:9092 \
cargo run
```

Or use the full docker-compose stack:

```bash
cd search-indexer-deploy
docker-compose up -d
```

## Overview

The search indexer consumes entity events from Kafka and indexes them into OpenSearch
for fast full-text search across the Geo Knowledge Graph.

## Architecture

The indexer follows the Consumer-Processor-Loader pattern using tokio tasks for each component:

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Consumer   │ ──▶ │  Processor  │ ──▶ │   Loader    │
│  (Kafka)    │     │ (Transform) │     │ (OpenSearch)│
└─────────────┘     └─────────────┘     └─────────────┘
        │                                      │
        │      ◀──      ack/nack     ◀──       │
        └──────────────────────────────────────┘
                     Orchestrator 
                (Setup channels and tasks)
```

### Components

- **Consumer**: Consumes entity events from Kafka topics (`knowledge.edits`) and sends them directly to the processor via channels
- **Processor**: Transforms raw Kafka events into `EntityDocument` structures and sends them directly to the loader. Runs in its own tokio task with a `run()` method that accepts channels and returns a task handle.
- **Loader**: Batches and indexes documents into OpenSearch using `UpdateEntityRequest` and sends acknowledgments directly back to the consumer. Runs in its own tokio task with a `run()` method that accepts channels and returns a task handle.
- **Orchestrator**: Sets up channels between components, spawns all tasks, monitors for shutdown signals, and tracks metrics. Components communicate directly with each other without going through the orchestrator.

## Configuration

Environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `OPENSEARCH_URL` | OpenSearch server URL | `http://localhost:9200` |
| `INDEX_ALIAS` | Index alias name | `entities` |
| `ENTITIES_INDEX_VERSION` | Index version number | `0` |
| `KAFKA_BROKER` | Kafka broker address | `localhost:9092` |
| `KAFKA_GROUP_ID` | Consumer group ID | `search-indexer` |
| `KAFKA_TOPIC` | Kafka topic to consume | `knowledge.edits` |
| `KAFKA_BATCH_SIZE` | Messages to batch before sending | `50` |
| `KAFKA_BATCH_TIMEOUT_MS` | Max wait time before flushing batch (ms) | `1000` |
| `OPENSEARCH_CONNECTION_MODE` | Connection mode: `fail-fast` or `retry` | `retry` |
| `OPENSEARCH_RETRY_INTERVAL_SECS` | Retry interval in seconds (retry mode only) | `15` |
| `AXIOM_TOKEN` | Axiom API token (optional) | - |
| `AXIOM_DATASET` | Axiom dataset name | `gaia.search-indexer` |
| `RUST_LOG` | Log level filter | `search_indexer=info` |

### Connection Modes

The search-indexer supports two connection modes for OpenSearch:

- **`retry`** (default): Continuously retries connecting to OpenSearch every 15 seconds (configurable via `OPENSEARCH_RETRY_INTERVAL_SECS`) until successful. This is useful when OpenSearch may not be immediately available (e.g., during container startup).

- **`fail-fast`**: Immediately fails if unable to connect to OpenSearch. Useful when you want the container to crash if OpenSearch is unavailable, allowing orchestration systems (like Kubernetes) to handle restarts.

## Running

### Prerequisites

1. OpenSearch running at `OPENSEARCH_URL`
2. Kafka broker running at `KAFKA_BROKER`
3. `knowledge.edits` topic exists in Kafka

### Start the indexer

```bash
# With environment variables
OPENSEARCH_URL=http://localhost:9200 \
KAFKA_BROKER=localhost:9092 \
cargo run --release

# Or with .env file
cp .env.example .env
# Edit .env with your configuration
cargo run --release
```

### Docker

#### Building the image

```bash
# From the repository root
docker build -f search-indexer/Dockerfile -t search-indexer .
```

#### Running with docker-compose

The search-indexer is included in the `search-indexer-deploy/docker-compose.yaml` file:

```bash
# Start OpenSearch and search-indexer together
cd search-indexer-deploy
docker-compose up -d

# View logs
docker-compose logs -f search-indexer
```

**Note**: The docker-compose setup connects to the Kafka broker from the `hermes` docker-compose network. Make sure the hermes Kafka broker is running:

```bash
# Start Kafka broker
cd ../hermes
docker-compose up -d kafka
```

#### Running standalone

```bash
# With retry mode (default)
docker run -e OPENSEARCH_URL=http://opensearch:9200 \
           -e KAFKA_BROKER=kafka:29092 \
           -e OPENSEARCH_CONNECTION_MODE=retry \
           search-indexer

# With fail-fast mode
docker run -e OPENSEARCH_URL=http://opensearch:9200 \
           -e KAFKA_BROKER=kafka:29092 \
           -e OPENSEARCH_CONNECTION_MODE=fail-fast \
           search-indexer
```

## Development

### Building

```bash
cargo build
```

### Testing

```bash
cargo test
```

### Running locally

```bash
# Start dependencies
docker-compose -f ../hermes/docker-compose.yml up -d

# Run the indexer
cargo run
```

## Verifying the Indexer

After starting, verify the indexer is working:

```bash
# Check OpenSearch cluster health
curl "http://localhost:9200/_cluster/health?pretty"

# Check if the entities index exists
curl "http://localhost:9200/_cat/indices?v"

# Query indexed documents
curl "http://localhost:9200/entities/_search?pretty" -H 'Content-Type: application/json' -d '{
  "query": { "match_all": {} },
  "size": 5
}'
```

## Monitoring

The indexer logs to stdout in JSON format. When `AXIOM_TOKEN` is set, logs are
also sent to Axiom for centralized monitoring.

Key metrics to monitor:
- Documents indexed per second
- Index latency (ms)
- Kafka consumer lag
- Error rates

## Troubleshooting

### Common issues

**Cannot connect to OpenSearch**
- Check `OPENSEARCH_URL` is correct
- Verify OpenSearch is running: `curl http://localhost:9200`

**Cannot connect to Kafka**
- Check `KAFKA_BROKER` is correct
- Verify Kafka is running and `knowledge.edits` topic exists

**High latency**
- Check OpenSearch cluster health
- Monitor Kafka consumer lag
- Consider increasing batch size in loader config

