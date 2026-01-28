# Search Indexer

Main binary for the Geo Knowledge Graph search indexer. Creates an orchestrator that handles consuming entity events from Kafka and indexing them into OpenSearch for full-text search across the Knowledge Graph.

## Quick Start

```bash
# 1. Start Kafka
cd ../hermes && docker-compose up -d kafka

# 2. Run the indexer (with auto index creation for local dev)
cd ../search-indexer
OPENSEARCH_URL=http://localhost:9200 \
KAFKA_BROKER=localhost:9092 \
cargo run --features search-indexer-repository/auto_index_creation
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

### Index Management

The `auto_index_creation` feature is **disabled by default** for production safety. Indices must be created manually using the `search-admin` tool.

**Local Development**: The feature can be enabled explicitly:
- Via cargo: `cargo run --features search-indexer-repository/auto_index_creation`
- Via docker-compose: Already enabled in `search-indexer-deploy/docker-compose.yaml`

**Production**: The feature is disabled in:
- Docker builds (no build arg passed)
- Kubernetes deployments
- Release binaries

See the [search-admin documentation](../search-admin/README.md) for manual index creation.

### Environment variables:

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
| `KAFKA_USERNAME` | SASL username for managed Kafka (optional, enables SASL/SSL if set) | - |
| `KAFKA_PASSWORD` | SASL password for managed Kafka (required if username is set) | - |
| `KAFKA_SSL_CA_PEM` | Custom CA certificate in PEM format (optional) | - |
| `OPENSEARCH_CONNECTION_MODE` | Connection mode: `fail-fast` or `retry` | `retry` |
| `OPENSEARCH_RETRY_INTERVAL_SECS` | Retry interval in seconds (retry mode only) | `15` |
| `HEALTH_PORT` | HTTP port for health check endpoints | `8080` |

### Telemetry Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `SENTRY_DSN` | Sentry project DSN (enables Sentry when set) | - |
| `SENTRY_TRACES_SAMPLE_RATE` | Trace sampling rate 0.0-1.0 | `1.0` |
| `SENTRY_SEND_DEFAULT_PII` | Include PII in events (`true` or `false`) | `false` |
| `SENTRY_ENVIRONMENT` | Environment tag (e.g., "staging", "production") | - |
| `SENTRY_RELEASE` | Release version (e.g., "search-indexer@1.2.3") | - |
| `SENTRY_DEBUG` | Enable debug mode (logs spans to stdout) | `false` |

### Connection Modes

The search-indexer supports two connection modes for OpenSearch:

- **`retry`** (default): Continuously retries connecting to OpenSearch every 15 seconds (configurable via `OPENSEARCH_RETRY_INTERVAL_SECS`) until successful. This is useful when OpenSearch may not be immediately available (e.g., during container startup).

- **`fail-fast`**: Immediately fails if unable to connect to OpenSearch. Useful when you want the container to crash if OpenSearch is unavailable, allowing orchestration systems (like Kubernetes) to handle restarts.

## Telemetry and Monitoring

The search-indexer uses the unified `hermes-instrumentation` telemetry crate for observability, supporting both local development (Console backend) and production monitoring (Sentry backend).

### Telemetry Backends

#### Console Backend (Default)

When `SENTRY_DSN` is not set, telemetry uses the Console backend:
- Outputs structured logs to stdout
- Suitable for local development and simple deployments
- No external dependencies required

```bash
# Console backend is used automatically when SENTRY_DSN is not set
cargo run
# Output: Telemetry: Console (set SENTRY_DSN to enable Sentry)
```

#### Sentry Backend

When `SENTRY_DSN` is set, telemetry switches to the Sentry backend:
- Distributed tracing with performance monitoring
- Error tracking with full context and stack traces
- Automatic span instrumentation for batch processing
- View traces in Sentry's Performance dashboard

```bash
# Enable Sentry backend
export SENTRY_DSN="https://examplePublicKey@o0.ingest.sentry.io/0"
export SENTRY_ENVIRONMENT="production"
export SENTRY_TRACES_SAMPLE_RATE="0.1"
cargo run
# Output: Telemetry: Sentry (env: production, sample_rate: 0.1)
```

### Instrumented Spans

The search-indexer automatically creates performance spans for key operations:

- **`search_indexer.consume_entities_batch`**: Entity event batch consumption
  - Fields: `batch_size`, `event_count`, `offset_start`, `offset_end`
- **`search_indexer.consume_scores_batch`**: Score event batch consumption
  - Fields: `batch_size`, `event_count`, `offset_start`, `offset_end`
- **`search_indexer.handle_entity_batch`**: Entity event processing
  - Fields: `event_count`
- **`search_indexer.process_score_batch`**: Score event processing
  - Fields: `event_count`
- **`search_indexer.bulk_operations`**: OpenSearch bulk indexing
  - Fields: `operation_count`

### Sampling Strategy

Trace sampling controls what percentage of transactions are sent to Sentry. For high-volume Kafka processing, proper sampling is critical:

| Environment | Recommended Rate | Reasoning |
|-------------|-----------------|-----------|
| Development | `1.0` (100%) | Capture everything for debugging |
| Staging | `0.5` (50%) | Balance coverage and volume |
| Production | `0.1` (10%) | Sufficient for monitoring at scale |

**Example configurations:**

```bash
# Development - capture all traces
SENTRY_TRACES_SAMPLE_RATE=1.0

# Production - 10% sampling for high-volume processing
SENTRY_TRACES_SAMPLE_RATE=0.1
```

### Environment Configuration

See [.env.example](.env.example) for a complete configuration reference with all Sentry variables and recommended values.

### Key Metrics to Monitor

Whether using Console or Sentry backend, monitor these key metrics:

- **Throughput**: Events processed per second, documents indexed per second
- **Latency**: Time spent in each processing stage (consume → process → load)
- **Kafka Consumer Lag**: Difference between latest offset and committed offset
- **Error Rates**: Failed batch processing, OpenSearch indexing errors
- **Span Performance**: Identify slow operations via distributed traces (Sentry only)

### Viewing Traces in Sentry

When Sentry backend is enabled:

1. Navigate to your Sentry project's Performance dashboard
2. Filter by `transaction:"search_indexer.*"` to see all indexer spans
3. View span hierarchies: `consume_batch` → `process_batch` → `bulk_operations`
4. Analyze slow traces to identify performance bottlenecks
5. Errors automatically link to their corresponding traces for full context

## Running

### Prerequisites

1. OpenSearch running at `OPENSEARCH_URL`
2. Kafka broker running at `KAFKA_BROKER`
3. `knowledge.edits` topic exists in Kafka

### Start the indexer

```bash
# With environment variables (enable auto index creation for local dev)
OPENSEARCH_URL=http://localhost:9200 \
KAFKA_BROKER=localhost:9092 \
cargo run --features search-indexer-repository/auto_index_creation

# Or with .env file
cp .env.example .env
# Edit .env with your configuration
cargo run --features search-indexer-repository/auto_index_creation

# For production builds (no auto index creation - use search-admin)
cargo build --release
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
# Unit tests
cargo test

# E2E tests with Kafka and Search API validation
cd tests/e2e-kafka-search-api
./run-test.sh
```

See [TESTING.md](TESTING.md) for comprehensive end-to-end testing documentation.

### Running locally

```bash
# Start dependencies
docker-compose -f ../hermes/docker-compose.yml up -d

# Run the indexer (with auto index creation for local dev)
cargo run --features search-indexer-repository/auto_index_creation
```

## Verifying the Indexer

After starting, verify the indexer is working:

```bash
# Check OpenSearch cluster health
curl "http://localhost:9200/_cluster/health?pretty"

# Check if the entities index exists
curl "http://localhost:9200/_cat/indices?v"

# Query indexed documents directly in OpenSearch
curl "http://localhost:9200/entities/_search?pretty" -H 'Content-Type: application/json' -d '{
  "query": { "match_all": {} },
  "size": 5
}'

# Query via the search API (requires API server running)
# Basic search
curl --compressed "http://localhost:3000/search?query=alice" | jq

# Search within a specific space
curl --compressed "http://localhost:3000/search?query=alice&scope=SPACE_SINGLE&space_id=00000000-0000-4000-8000-000000000001" | jq

# Filter by entity types
curl --compressed "http://localhost:3000/search?query=alice&type_ids=00000000-0000-0000-0000-000000000b01" | jq
```


## Troubleshooting

### Common issues

**Cannot connect to OpenSearch**
- Check `OPENSEARCH_URL` is correct
- Verify OpenSearch is running: `curl http://localhost:9200`

**Cannot connect to Kafka**
- Check `KAFKA_BROKER` is correct
- Verify Kafka is running and `knowledge.edits` topic exists
- For managed Kafka, ensure `KAFKA_USERNAME`, `KAFKA_PASSWORD`, and `KAFKA_SSL_CA_PEM` are set
- Check that `security.protocol` is correctly configured (automatically set to `SASL_SSL` when credentials are provided)

**High latency**
- Check OpenSearch cluster health
- Monitor Kafka consumer lag
- Consider increasing batch size in loader config

