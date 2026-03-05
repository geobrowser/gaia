# Search Indexer

Main binary for the Geo Knowledge Graph search indexer. Creates an orchestrator that handles consuming entity events from Kafka and indexing them into OpenSearch for full-text search across the Knowledge Graph.

## Quick Start

```bash
# 1. Start Kafka
cd ../hermes && docker-compose up -d kafka

# 2. Run the indexer (with auto index creation for local dev)
cd ../search-indexer
ENVIRONMENT=staging \
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
| `ENVIRONMENT` | **Required.** `staging` or `production`. Controls Kafka topic prefix. | - |
| `OPENSEARCH_URL` | OpenSearch server URL | `http://localhost:9200` |
| `INDEX_ALIAS` | Index alias name | `entities` |
| `ENTITIES_INDEX_VERSION` | Index version number | `0` |
| `KAFKA_BROKER` | Kafka broker address | `localhost:9092` |
| `KAFKA_GROUP_EDITS_ID` | Consumer group ID for entity events | `search-indexer-group-edits` |
| `KAFKA_GROUP_SCORES_ID` | Consumer group ID for score events | `search-indexer-group-scores` |
| `KAFKA_TOPIC` | Kafka topic to consume | `knowledge.edits` |
| `KAFKA_BATCH_SIZE` | Messages to batch before sending (entities consumer) | `10` |
| `KAFKA_BATCH_TIMEOUT_MS` | Max wait time before flushing batch (entities consumer, ms) | `1000` |
| `SCORES_BATCH_SIZE` | Messages to batch before sending (scores consumer) | `10` |
| `SCORES_BATCH_TIMEOUT_MS` | Max wait time before flushing batch (scores consumer, ms) | `1000` |
| `CHANNEL_BUFFER_SIZE` | Max batches in flight per channel | `5` |
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
ENVIRONMENT=staging \
OPENSEARCH_URL=http://localhost:9200 \
KAFKA_BROKER=localhost:9092 \
cargo run --features search-indexer-repository/auto_index_creation

# Or with .env file
cp .env.example .env
# Edit .env with your configuration (must include ENVIRONMENT=staging or ENVIRONMENT=production)
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
# With retry mode (default) - staging environment
docker run -e ENVIRONMENT=staging \
           -e OPENSEARCH_URL=http://opensearch:9200 \
           -e KAFKA_BROKER=kafka:29092 \
           -e OPENSEARCH_CONNECTION_MODE=retry \
           search-indexer

# With fail-fast mode - production environment
docker run -e ENVIRONMENT=production \
           -e OPENSEARCH_URL=http://opensearch:9200 \
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
ENVIRONMENT=staging cargo run --features search-indexer-repository/auto_index_creation
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


## GRC-20 Support

The search indexer consumes `HermesEdit` messages from Kafka and decodes the GRC-20 v2 payload using the `grc-20` crate (v0.3.0). Only operations relevant to search indexing are processed.

### Handled Operations

| Operation | Handling | Notes |
|-----------|----------|-------|
| `CreateEntity` | ✓ Indexed | Creates entity document with `name`, `description`, `avatar` properties from initial values. |
| `UpdateEntity` | ✓ Indexed | Extracts `name`, `description`, `avatar` properties. Handles `unset_values` to clear properties. |
| `CreateRelation` | ✓ Indexed | Only processes **type relations** (where `relation_type == TYPE_RELATION_TYPE_ID`). Adds type IDs to entities for type filtering. |
| `DeleteRelation` | ✓ Indexed | Removes type relations from entities. |
| `DeleteEntity` | ✓ Indexed | Soft delete - sets `deleted=true` on the entity document. Deleted entities are excluded from search results. |
| `RestoreEntity` | ✓ Indexed | Restores a soft-deleted entity by setting `deleted=false`. The entity will reappear in search results. |

### Not Yet Implemented

| Operation | Notes |
|-----------|-------|
| `UpdateRelation` | Could support updating type relations. |
| `RestoreRelation` | Would restore deleted type relations. |
| `CreateValueRef` | Could index value reference metadata. |

### Soft Delete and Restore Behavior

When a `DeleteEntity` operation is processed:
1. The entity document is updated with `deleted=true`
2. The OpenSearch query filters exclude `deleted=true` documents from search results
3. Subsequent updates to the deleted entity are ignored (tombstone dominance)

When a `RestoreEntity` operation is processed:
1. The entity document is updated with `deleted=false`
2. The entity will reappear in search results
3. Subsequent updates to the entity will be applied normally

**Tombstone dominance:** Per the GRC-20 spec, updates to deleted entities are ignored. This is enforced at the OpenSearch level using Painless scripts that check the `deleted` status before applying updates. Type relation additions/removals are also skipped for deleted entities. Only explicit delete (`deleted=true`) or restore (`deleted=false`) operations can modify deleted entities.

### Message Format

The indexer expects `HermesEdit` protobuf messages on the `knowledge.edits` Kafka topic:

```protobuf
message HermesEdit {
  bytes id = 1;           // Edit UUID (16 bytes)
  string name = 2;        // Human-readable edit name
  bytes payload = 3;      // GRC-20 v2 encoded bytes (GRC2 or GRC2Z)
  repeated bytes authors = 4;
  bytes language = 5;     // Optional
  bytes space_id = 6;     // Space UUID (16 bytes)
  bool is_canonical = 7;
  BlockchainMetadata meta = 8;
}
```

The `payload` field contains GRC-20 v2 wire format bytes, decoded using `grc_20::decode_edit()`.

## Memory

When reprocessing large Kafka backlogs (e.g., starting with a new consumer group), the indexer can accumulate significant data in memory. Understanding the memory model helps avoid OOM errors.

### Memory Architecture

```
Kafka Broker
     │
     ▼
┌─────────────────────────────────────┐
│   rdkafka Internal Queue            │  ← 64 MiB per consumer (rdkafka default)
│   (pre-fetched messages)            │
└─────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────┐
│   Application Channels              │  ← CHANNEL_BUFFER_SIZE × KAFKA_BATCH_SIZE
│   (EntityProcessingBatch, etc.)     │
└─────────────────────────────────────┘
     │
     ▼
   OpenSearch
```

### Channel Memory Usage

| Channel | Contents | Max Items | Memory Formula |
|---------|----------|-----------|----------------|
| `entities_processor` | `EntityProcessingBatch` | `CHANNEL_BUFFER_SIZE` | `CHANNEL_BUFFER_SIZE` × `KAFKA_BATCH_SIZE` × avg_msg_size |
| `scores_processor` | `ScoreProcessingBatch` | `CHANNEL_BUFFER_SIZE` | `CHANNEL_BUFFER_SIZE` × `SCORES_BATCH_SIZE` × avg_msg_size |
| `space_topics_processor` | `SpaceTopicProcessingBatch` | `CHANNEL_BUFFER_SIZE` | `CHANNEL_BUFFER_SIZE` × `SPACE_TOPICS_BATCH_SIZE` × avg_msg_size |
| `topology_processor` | `TopologyProcessingBatch` | `CHANNEL_BUFFER_SIZE` | `CHANNEL_BUFFER_SIZE` × `TOPOLOGY_BATCH_SIZE` × avg_msg_size |
| `loader` | `ProcessedBatch` | `CHANNEL_BUFFER_SIZE` | `CHANNEL_BUFFER_SIZE` × max(`KAFKA_BATCH_SIZE`, `SCORES_BATCH_SIZE`) × avg_processed_size |
| `*_ack` channels | `StreamMessage` (offsets only) | `CHANNEL_BUFFER_SIZE` | Negligible (~1 KiB each) |

### rdkafka Internal Queue Memory

| Queue | Memory |
|-------|--------|
| entities consumer | 64 MiB |
| scores consumer | 64 MiB |
| space_topics consumer | 64 MiB |
| topology consumer | 64 MiB |
| **Total** | **256 MiB** |

This is controlled by rdkafka's `queued.max.messages.kbytes` default (65,536 KiB per consumer).

### Topology State Memory

The canonical graph is held entirely in memory for O(1) lookups. It uses four data structures (`HashSet` for membership, two `HashMap`s for parent/distance, and nested `HashMap<HashSet>` for children). Per canonical space, this costs ~300 bytes due to hash table overhead.

| Canonical Spaces | Topology Memory |
|------------------|-----------------|
| 10,000 | ~3 MiB |
| 100,000 | ~30 MiB |
| 500,000 | ~150 MiB |

The topology state is also persisted to disk as JSON (see `TOPOLOGY_STATE_PATH`). During persistence, `snapshot()` temporarily allocates a copy of the node list (~36 bytes/node).

### Disk Storage

The topology state is persisted to a JSON file at `TOPOLOGY_STATE_PATH` (default `/data/topology_state.json`). Each node stores two hex-encoded UUIDs (space_id, parent_id) and a distance value, costing ~130 bytes per node on disk.

| Canonical Spaces | File Size |
|------------------|-----------|
| 10,000 | ~1.3 MB |
| 100,000 | ~13 MB |
| 500,000 | ~65 MB |
| ~8,000,000 | ~1 Gi |

The Kubernetes StatefulSet provisions a 1 Gi PersistentVolumeClaim at `/data`, which is sufficient for millions of spaces.

### Total Memory Formula

```
Total Memory ≈
    256 MiB                                                                          # rdkafka queues (4 consumers, fixed)
  + (CHANNEL_BUFFER_SIZE × KAFKA_BATCH_SIZE × avg_msg_size)                          # entities_processor
  + (CHANNEL_BUFFER_SIZE × SCORES_BATCH_SIZE × avg_msg_size)                         # scores_processor
  + (CHANNEL_BUFFER_SIZE × max(KAFKA_BATCH_SIZE, SCORES_BATCH_SIZE) × avg_proc_size) # loader
  + topology_state                                                                   # ~300 bytes × canonical_spaces
  + overhead                                                                         # ~100 MiB
```

### Example: Typical Case

With production settings (`CHANNEL_BUFFER_SIZE=5`, `KAFKA_BATCH_SIZE=10`, `SCORES_BATCH_SIZE=10`, avg entity message ~500 KiB, 500K canonical spaces):

| Component | Calculation | Memory |
|-----------|-------------|--------|
| rdkafka queues | 64 MiB × 4 consumers | 256 MiB |
| entities_processor | 5 batches × 10 msgs × 500 KiB | 25 MiB |
| scores_processor | 5 batches × 10 msgs × 50 bytes | <1 MiB |
| space_topics_processor | 5 batches × 10 msgs × 32 bytes | <1 MiB |
| topology_processor | 5 batches × 10 msgs × 40 KiB | 2 MiB |
| loader | 5 batches × 10 msgs × 300 KiB (processed) | 15 MiB |
| Topology state | 500K spaces × ~300 bytes | 150 MiB |
| Overhead | Runtime, heap fragmentation | 100 MiB |
| **Total** | | **~550 MiB** |

### Example: Worst Case

With worst-case entity messages at 10 MB, topology diffs with 1,000 changes each, 500K canonical spaces:

| Component | Calculation | Memory |
|-----------|-------------|--------|
| rdkafka queues | 64 MiB × 4 consumers | 256 MiB |
| entities_processor | 5 batches × 10 msgs × 10 MB | 500 MiB |
| scores_processor | 5 batches × 10 msgs × 50 bytes | <1 MiB |
| space_topics_processor | 5 batches × 10 msgs × 32 bytes | <1 MiB |
| topology_processor | 5 batches × 10 msgs × 40 KiB | 2 MiB |
| loader | 5 batches × 10 msgs × 10 MB | 500 MiB |
| Topology state | 500K spaces × ~300 bytes | 150 MiB |
| Overhead | Runtime, heap fragmentation | 100 MiB |
| **Total** | | **~1,510 MiB** |

Note: rdkafka's 64 MiB per-consumer queue limit throttles intake, so sustained worst-case entity messages cannot fully saturate all channel slots simultaneously. Realistic peak is closer to ~1.2–1.3 GiB.
## Error Recovery

### Reprocessing All Events

If you need to reprocess all events from the beginning (e.g., after fixing a bug, schema changes, or data corruption), change the consumer group IDs to new values:

```bash
# Use new consumer group IDs to reprocess from the beginning
KAFKA_GROUP_EDITS_ID=search-indexer-group-edits-v3 \
KAFKA_GROUP_SCORES_ID=search-indexer-group-scores-v3 \
ENVIRONMENT=staging \
OPENSEARCH_URL=http://localhost:9200 \
KAFKA_BROKER=localhost:9092 \
cargo run --features search-indexer-repository/auto_index_creation
```

**Warning:** This will reprocess ALL events from the very first Kafka message. For large topics, this may take significant time.

**Notes:**
- A new consumer group has no committed offsets, so `auto.offset.reset=earliest` starts from offset 0
- Update consumer group IDs once (e.g., `...-v2` → `...-v3`), then keep using those values
- Consider incrementing `ENTITIES_INDEX_VERSION` to index into a fresh index (use `search-admin` to create the new index first)

## Operational Observability (Structured Logs)

The indexer emits a structured `indexer.stats` log line every 10 seconds with fields that let you diagnose performance and health from logs alone, without Prometheus or Grafana.

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `events_processed` | cumulative | Total Kafka events consumed since startup |
| `documents_indexed` | cumulative | Total documents successfully indexed |
| `events_per_sec` | rate | Kafka events consumed per second (this interval) |
| `docs_per_sec` | rate | Documents indexed per second (this interval) |
| `ops_per_sec` | rate | Individual OpenSearch operations per second |
| `bulk_calls_per_sec` | rate | OpenSearch HTTP bulk/update_by_query calls per second |
| `avg_bulk_ms` | rate | Average wall-clock ms per OpenSearch call (this interval) |
| `failed_ops` | delta | Failed operations in this interval |
| `updates` | cumulative | Upsert operations (entity index + add relation) |
| `deletes` | cumulative | Delete operations |
| `unsets` | cumulative | Unset-property operations |
| `remove_relations` | cumulative | Remove-relation-by-ID operations |
| `score_updates` | cumulative | Score updates (entity global + space + entity-space) |
| `topic_updates` | cumulative | Space topic entity ID updates |
| `rss_mb` | snapshot | Process resident memory in MB (Linux only, `n/a` on macOS) |

### Diagnosing Common Issues

**"Where is the bottleneck?"**
- Low `events_per_sec` with idle `ops_per_sec` → Kafka consumption is the bottleneck (consumer lag, slow network, large messages).
- High `events_per_sec` but high `avg_bulk_ms` (>200ms) → OpenSearch is slow. Check cluster health, disk I/O, or index shard count.
- `events_per_sec` and `ops_per_sec` are both healthy but `docs_per_sec` is low → most events are score/topic updates (not document indexes).

**"What's the error rate?"**
- `failed_ops > 0` in an interval means OpenSearch rejected some operations. Check the error-level logs above the stats line for details (entity_id, operation_type, error message).

**"Is memory growing?"**
- Watch `rss_mb` over time. Steady growth suggests a leak or unbounded cache. Flat is healthy. See the Memory section above for expected baseline.

**"What kind of work is the indexer doing?"**
- Compare `updates` vs `score_updates` vs `topic_updates`. During a score backfill, `score_updates` will dominate. During normal entity ingestion, `updates` will dominate.

**"Is OpenSearch keeping up?"**
- `bulk_calls_per_sec` × `avg_bulk_ms` gives total ms spent in OpenSearch per second. If this approaches 1000ms, OpenSearch is saturated and you may need to scale it or reduce batch frequency.

### Example Log Line

```
INFO indexer.stats events_processed=152340 documents_indexed=148200 events_per_sec=1520.3 docs_per_sec=1480.1 ops_per_sec=1520.3 bulk_calls_per_sec=15.2 avg_bulk_ms=42.3 failed_ops=0 updates=148200 deletes=12 unsets=340 remove_relations=5 score_updates=3780 topic_updates=3 rss_mb=285
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

## Environment Isolation

The search-indexer supports staging and production environments on shared infrastructure (Kafka, OpenSearch, Kubernetes) through automatic prefixing controlled by the `ENVIRONMENT` variable.

### Resource Isolation Table

| Resource | Production | Staging |
|----------|------------|---------|
| **K8s Namespace** | `search` | `search-staging` |
| **Kafka Topics** | `knowledge.edits`, `curation.scores` | `staging.knowledge.edits`, `staging.curation.scores` |
| **Consumer Groups** | `search-indexer-group-edits-v2`, `search-indexer-group-scores-v2` | `staging-search-indexer-group-edits-v2`, `staging-search-indexer-group-scores-v2` |
| **OpenSearch Alias** | `entities` | `staging_entities` |
| **OpenSearch Indices** | `entities_v0`, `entities_v1`, ... | `staging_entities_v0`, `staging_entities_v1`, ... |

### How Prefixing Works

```
ENVIRONMENT=staging
    │
    ├─► Topic Prefix: "staging." (via hermes-kafka)
    │   └─► Topics: staging.knowledge.edits, staging.curation.scores
    │
    ├─► Index Prefix: "staging_" (via search-indexer-shared)
    │   └─► Alias: staging_entities
    │   └─► Indices: staging_entities_v0, staging_entities_v1, ...
    │
    └─► Consumer Group Prefix: "staging-" (applied to KAFKA_GROUP_EDITS_ID and KAFKA_GROUP_SCORES_ID)
        └─► Entities: staging-search-indexer-group-edits-v2
        └─► Scores: staging-search-indexer-group-scores-v2
```

### Deployment Files

- **Production:** `search-indexer-deploy/k8s/production/`
- **Staging:** `search-indexer-deploy/k8s/staging/`
- **Migration Jobs:** See `search-indexer-deploy/k8s/jobs/README.md`

