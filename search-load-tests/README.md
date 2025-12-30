# Search Load Tests

End-to-end load testing for the search service covering HTTP API queries and Kafka ingestion.

## Quick Start

### Prerequisites

**Option 1: Local k6 with Kafka extension**

```bash
# Install xk6 (Go required)
go install go.k6.io/xk6/cmd/xk6@latest

# Build k6 with kafka extension
xk6 build --with github.com/mostafa/xk6-kafka@latest

# Move to PATH
sudo mv k6 /usr/local/bin/k6
```

**Option 2: Docker (recommended)**

```bash
# Build the k6 Docker image
docker compose build k6
```

---

## Before Running Tests

Different tests require different services to be running. Make sure the required services are up before executing tests.

### For HTTP Load Tests

The search API must be running and accessible.

**Local development:**
```bash
# Start the search API (from the project root)
# Example - adjust based on your actual setup
cd ../api && bun run dev

# Or if using the search-indexer service
cd ../search-indexer && cargo run

# Verify it's running
curl http://localhost:3000/health
```

**Against staging/production:**
- No local setup needed - just use the appropriate `API_URL`

### For Kafka Load Tests

Kafka, OpenSearch, and the search-indexer must all be running. The flow is:
```
k6 → Kafka → search-indexer → OpenSearch
```

**Using Docker Compose (recommended for local testing):**
```bash
# Start local Kafka for load tests
cd search-load-tests
docker compose up -d kafka

# Verify Kafka is running
docker compose ps kafka

# Start OpenSearch (from project root - adjust path as needed)
cd ../search-indexer-deploy
docker compose up -d opensearch

# Start the search-indexer to consume Kafka events
# NOTE: Use port 9094 (external listener) for host-to-Docker connections
# NOTE: search-indexer uses KAFKA_BROKER (singular), not KAFKA_BROKERS
cd ../search-indexer
KAFKA_BROKER=localhost:9094 \
OPENSEARCH_URL=http://localhost:9200 \
cargo run
```

**Important: Kafka Port Mapping**
- `kafka:9092` - Use this when connecting FROM Docker containers (k6, etc.)
- `localhost:9094` - Use this when connecting FROM host machine (search-indexer, etc.)

**Using external Kafka:**
- Set `KAFKA_BROKERS` to your Kafka cluster address
- If using SASL authentication, also set `KAFKA_USERNAME` and `KAFKA_PASSWORD`

### For Combined Load Tests

All services must be running (Kafka, OpenSearch, search-indexer, and search API):

```bash
# Terminal 1: Start Kafka
cd search-load-tests
docker compose up -d kafka

# Terminal 2: Start OpenSearch
cd ../search-indexer-deploy
docker compose up -d opensearch

# Terminal 3: Start the search-indexer
# NOTE: Use port 9094 for host-to-Docker Kafka connection
# NOTE: search-indexer uses KAFKA_BROKER (singular)
cd ../search-indexer
KAFKA_BROKER=localhost:9094 \
OPENSEARCH_URL=http://localhost:9200 \
cargo run

# Terminal 4: Start the search API (adjust path as needed)
cd ../api && bun run start

# Terminal 5: Run the combined test
cd search-load-tests
docker compose run --rm k6 run combined-load-test.js \
  -e API_URL=http://host.docker.internal:3000 \
  -e KAFKA_BROKERS=kafka:9092 \
  -e HTTP_RPS=100 \
  -e KAFKA_EPS=200 \
  -e DURATION=1m
```

### For Index Seeding

Kafka, OpenSearch, and the search-indexer need to be running:

```bash
# Terminal 1: Start Kafka
cd search-load-tests
docker compose up -d kafka

# Terminal 2: Start OpenSearch
cd ../search-indexer-deploy
docker compose up -d opensearch

# Terminal 3: Start the search-indexer to consume events
# NOTE: Use port 9094 for host-to-Docker Kafka connection
# NOTE: search-indexer uses KAFKA_BROKER (singular)
cd ../search-indexer
KAFKA_BROKER=localhost:9094 \
OPENSEARCH_URL=http://localhost:9200 \
cargo run

# Terminal 4: Run seeding
cd search-load-tests
docker compose run --rm k6 run seed-index.js \
  -e KAFKA_BROKERS=kafka:9092 \
  -e TARGET_DOCS=100000

# Wait for search-indexer to process all events before running load tests
# Check consumer lag in Kafka UI or logs
```

### Service Checklist

| Test Type | Kafka | OpenSearch | Search Indexer | Search API |
|-----------|-------|------------|----------------|------------|
| HTTP Load Test | ❌ | ✅ | ❌ | ✅ |
| Kafka Load Test | ✅ | ✅ | ✅ | ❌ |
| Combined Test | ✅ | ✅ | ✅ | ✅ |
| Index Seeding | ✅ | ✅ | ✅ | ❌ |

**Data flow:**
- **HTTP tests:** Search API → OpenSearch (queries only)
- **Kafka tests:** k6 → Kafka → Search Indexer → OpenSearch (indexing)

---

## Running Tests

### HTTP Load Test

Tests the search API endpoint with realistic query patterns.

**Local k6:**

```bash
# Light load (10 RPS, 1 minute)
k6 run http-load-test.js -e API_URL=http://localhost:3000 -e PROFILE=light

# Moderate load (100 RPS, 5 minutes)
k6 run http-load-test.js -e API_URL=http://localhost:3000 -e PROFILE=moderate

# Heavy load (500 RPS, 10 minutes)
k6 run http-load-test.js -e API_URL=http://search.staging:3000 -e PROFILE=heavy

# Stress test (1000 RPS, 15 minutes)
k6 run http-load-test.js -e API_URL=http://search.staging:3000 -e PROFILE=stress
```

**Docker:**

```bash
# Run HTTP test against host machine
docker compose run --rm k6 run http-load-test.js \
  -e API_URL=http://host.docker.internal:3000 \
  -e PROFILE=moderate

# Run against external API
docker compose run --rm k6 run http-load-test.js \
  -e API_URL=https://search.staging.geo.browser \
  -e PROFILE=heavy
```

---

### Kafka Load Test

Produces HermesEdit messages to test the search indexer ingestion pipeline.

**Local k6:**

```bash
# Light load (50 events/sec, 1 minute)
k6 run kafka-load-test.js \
  -e KAFKA_BROKERS=localhost:9092 \
  -e KAFKA_PROFILE=light

# Moderate load (200 events/sec, 5 minutes)
k6 run kafka-load-test.js \
  -e KAFKA_BROKERS=localhost:9092 \
  -e KAFKA_PROFILE=moderate

# Heavy load (1000 events/sec, 10 minutes)
k6 run kafka-load-test.js \
  -e KAFKA_BROKERS=kafka.staging:9092 \
  -e KAFKA_PROFILE=heavy

# With SASL authentication
k6 run kafka-load-test.js \
  -e KAFKA_BROKERS=kafka.prod:9092 \
  -e KAFKA_USERNAME=myuser \
  -e KAFKA_PASSWORD=mypassword \
  -e KAFKA_PROFILE=moderate
```

**Docker:**

```bash
# Start local Kafka first
docker compose up -d kafka

# Run Kafka test (uses internal kafka:9092)
docker compose run --rm k6 run kafka-load-test.js \
  -e KAFKA_BROKERS=kafka:9092 \
  -e KAFKA_PROFILE=moderate

# Run against external Kafka
docker compose run --rm k6 run kafka-load-test.js \
  -e KAFKA_BROKERS=kafka.staging:9092 \
  -e KAFKA_PROFILE=heavy
```

---

### Combined Load Test

Runs both HTTP and Kafka load simultaneously.

**Local k6:**

```bash
k6 run combined-load-test.js \
  -e API_URL=http://localhost:3000 \
  -e KAFKA_BROKERS=localhost:9092 \
  -e HTTP_RPS=100 \
  -e KAFKA_EPS=200 \
  -e DURATION=5m
```

**Docker:**

```bash
docker compose up -d kafka

docker compose run --rm k6 run combined-load-test.js \
  -e API_URL=http://host.docker.internal:3000 \
  -e KAFKA_BROKERS=kafka:9092 \
  -e HTTP_RPS=500 \
  -e KAFKA_EPS=1000 \
  -e DURATION=10m
```

---

### Index Seeding

Pre-populate the search index with documents before load testing.

**Local k6:**

```bash
# Seed 10,000 docs (small)
k6 run seed-index.js -e TARGET_DOCS=10000 -e KAFKA_BROKERS=localhost:9092

# Seed 100,000 docs (medium)
k6 run seed-index.js -e TARGET_DOCS=100000 -e KAFKA_BROKERS=localhost:9092

# Seed 1,000,000 docs (large)
k6 run seed-index.js -e TARGET_DOCS=1000000 -e KAFKA_BROKERS=localhost:9092
```

**Docker:**

```bash
docker compose up -d kafka

# Seed via Docker
docker compose run --rm k6 run seed-index.js \
  -e TARGET_DOCS=1000000 \
  -e KAFKA_BROKERS=kafka:9092
```

---

## Docker Compose Setup

### Start Local Infrastructure

```bash
# Start Kafka only
docker compose up -d kafka

# Start Kafka + monitoring (Grafana, InfluxDB, Kafka UI)
docker compose --profile monitoring up -d

# View Kafka UI
open http://localhost:8080

# View Grafana dashboards
open http://localhost:3001  # admin/admin
```

### Run Tests with Metrics

```bash
# Start monitoring stack
docker compose --profile monitoring up -d

# Run test with InfluxDB output
docker compose run --rm k6 run http-load-test.js \
  -e API_URL=http://host.docker.internal:3000 \
  -e PROFILE=heavy \
  --out influxdb=http://influxdb:8086/k6
```

### Cleanup

```bash
# Stop all containers
docker compose --profile monitoring down

# Remove volumes too
docker compose --profile monitoring down -v
```

---

## Test Profiles

### HTTP Profiles

| Profile    | RPS   | Duration | Use Case                |
|------------|-------|----------|-------------------------|
| `light`    | 10    | 1m       | Development testing     |
| `moderate` | 100   | 5m       | Integration testing     |
| `heavy`    | 500   | 10m      | Staging/pre-prod        |
| `stress`   | 1000  | 15m      | Find breaking points    |

### Kafka Profiles

| Profile    | EPS   | Duration | Use Case                |
|------------|-------|----------|-------------------------|
| `light`    | 50    | 1m       | Development testing     |
| `moderate` | 200   | 5m       | Integration testing     |
| `heavy`    | 1000  | 10m      | Staging/pre-prod        |
| `stress`   | 5000  | 15m      | Find breaking points    |

### Index Sizes

| Size     | Documents   | Use Case                    |
|----------|-------------|-----------------------------|
| `small`  | 10,000      | Quick iteration             |
| `medium` | 100,000     | Integration testing         |
| `large`  | 1,000,000   | Production-like load        |
| `xlarge` | 10,000,000  | Stress testing              |

---

## Query Types

The HTTP test generates queries with realistic distribution:

| Type        | Weight | Description                        |
|-------------|--------|------------------------------------|
| `simple`    | 35%    | Single word: "blockchain"          |
| `multiWord` | 25%    | 2-3 words: "decentralized network" |
| `typos`     | 15%    | Typos: "blockchan" (fuzzy test)    |
| `long`      | 10%    | 4-6 words phrase                   |
| `edge`      | 10%    | Edge cases: empty, unicode, etc.   |
| `prefix`    | 5%     | Partial words: "block"             |

---

## Document Variations

The Kafka test generates documents with varied characteristics:

**Name Lengths:**
- Short (5-20 chars): 40%
- Medium (20-100 chars): 40%
- Long (100-300 chars): 20%

**Description Lengths:**
- None: 20%
- Short (10-100 chars): 30%
- Medium (100-500 chars): 30%
- Long (500-2000 chars): 20%

---

## Environment Variables

### HTTP Test

| Variable           | Default                  | Description                    |
|--------------------|--------------------------|--------------------------------|
| `API_URL`          | `http://localhost:3000`  | Search API base URL            |
| `PROFILE`          | `moderate`               | Load profile                   |
| `TEST_SPACE_ID`    | (none)                   | Space ID for scoped searches   |
| `SPACE_SCOPED_RATIO` | `0.3`                  | Ratio of space-scoped queries  |

### Kafka Test

| Variable         | Default              | Description                    |
|------------------|----------------------|--------------------------------|
| `KAFKA_BROKERS`  | `localhost:9092`     | Kafka broker addresses         |
| `KAFKA_TOPIC`    | `knowledge.edits`    | Topic to produce to            |
| `KAFKA_PROFILE`  | `moderate`           | Load profile                   |
| `KAFKA_USERNAME` | (none)               | SASL username                  |
| `KAFKA_PASSWORD` | (none)               | SASL password                  |

### Seeding

| Variable        | Default            | Description                    |
|-----------------|-------------------|--------------------------------|
| `TARGET_DOCS`   | `10000`           | Number of docs to seed         |
| `BATCH_SIZE`    | `100`             | Batch size for producing       |
| `KAFKA_BROKERS` | `localhost:9092`  | Kafka broker addresses         |

---

## Thresholds

Tests will fail if these thresholds are exceeded:

### HTTP

- p95 latency < 500ms
- p99 latency < 1000ms
- Error rate < 1%

### Kafka

- p95 produce latency < 100ms
- p99 produce latency < 200ms
- Error rate < 0.1%

---

## Metrics & Monitoring

### Output to InfluxDB

```bash
k6 run http-load-test.js --out influxdb=http://localhost:8086/k6
```

### Output to JSON

```bash
k6 run http-load-test.js --out json=results/test-results.json
```

### Output to CSV

```bash
k6 run http-load-test.js --out csv=results/test-results.csv
```

---

## Troubleshooting

### k6 not found

```bash
# Verify k6 is installed with kafka extension
k6 version
# Should show xk6-kafka in extensions
```

### Kafka connection failed

```bash
# Check Kafka is running
docker compose ps kafka

# Check Kafka logs
docker compose logs kafka

# Test connectivity
docker compose exec kafka kafka-broker-api-versions.sh --bootstrap-server localhost:9092
```

### Docker host.docker.internal not working

On Linux, add this to your `/etc/hosts`:
```
172.17.0.1 host.docker.internal
```

Or use the actual host IP address in `API_URL`.

---

## File Structure

```
search-load-tests/
├── config/
│   └── profiles.js           # Load profiles and thresholds
├── lib/
│   ├── queries.js            # Query generation utilities
│   ├── documents.js          # Document generation utilities
│   └── protobuf.js           # HermesEdit protobuf encoding
├── grafana/
│   └── provisioning/         # Grafana config
├── scripts/
│   └── run-test.sh           # Helper script
├── http-load-test.js         # HTTP search load test
├── kafka-load-test.js        # Kafka ingestion load test
├── combined-load-test.js     # Combined load test
├── seed-index.js             # Index seeding script
├── Dockerfile                # k6 with kafka extension
├── docker-compose.yaml       # Docker infrastructure
└── README.md                 # This file
```

