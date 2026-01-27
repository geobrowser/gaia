# Testing the Search Indexer

This guide covers how to test the search-indexer end-to-end.

## Recommended Testing Approach: E2E Kafka Search API

**The e2e-kafka-search-api is the recommended way to perform end-to-end testing of the search-indexer.** It provides:

- ✅ Fast, focused testing without full pipeline overhead
- ✅ Comprehensive test scenarios with known entity IDs
- ✅ Automated TypeScript validation against the search API
- ✅ Precise control over test data for specific scenarios
- ✅ Easy debugging and iteration

### Quick E2E Test

This is the fastest way to test search functionality end-to-end:

1. **Start Kafka:**
   ```bash
   cd hermes
   docker-compose up -d kafka kafka-ui
   ```

2. **Generate test events and validate:**
   ```bash
   cd search-indexer/tests/e2e-kafka-search-api
   ./run-test.sh
   ```

   This script:
   - Generates comprehensive test data (11 entities, relations, scores)
   - Uses fixed entity IDs for reliable validation
   - Automatically validates search API responses if running
   - Tests zero and negative scores, type relations, and ordering

3. **Start the search-indexer:**
   ```bash
   cd ../../..  # Back to repo root
   KAFKA_BROKER=localhost:9092 \
   OPENSEARCH_URL=http://localhost:9200 \
   KAFKA_GROUP_ID=search-indexer-test-$(date +%s) \
   RUST_LOG=debug,search_indexer=debug \
   INDEX_CREATION_ENABLED=true \
   ENTITIES_INDEX_VERSION=0 \
   cargo run -p search-indexer
   ```

4. **Run validation manually (if API is running):**
   ```bash
   cd search-indexer/tests/e2e-kafka-search-api/typescript
   npm run validate
   ```

### What Gets Tested

The e2e-kafka-search-api test scenario validates:

- **Entity creation**: 11 entities including Alice (7 variants), Bob, Acme Corp, and type entities
- **Type relations**: Multiple types per entity, partial removal, create-delete-create patterns
- **Scores**: Zero scores (0.0), negative scores (-0.75), positive scores (0.05 to 0.95)
- **Score ordering**: Entities returned in descending order by global score
- **Search API**: Name matching, type filtering, pagination, field validation
- **TypeScript types**: Validates actual API types match expected interfaces

See [Test Event Generation](tests/TEST_EVENTS.md) and [e2e-kafka-search-api README](tests/e2e-kafka-search-api/README.md) for detailed documentation.

## Prerequisites

### Clean up old OpenSearch indices (if needed)

If you previously ran the search-indexer with an old index format, you may need to delete the existing indices:

```bash
# Delete the index and alias
curl -X DELETE "http://localhost:9200/entities_v0"
curl -X DELETE "http://localhost:9200/entities"

# Verify deletion
curl "http://localhost:9200/_cat/indices?v"
```

## Alternative: Testing with Hermes Pipeline

While e2e-kafka-search-api is recommended for search-indexer testing, you can also test with the full hermes-pipeline for broader integration testing.

### Option A: Run hermes-pipeline with cargo

1. **Start Kafka:**
   ```bash
   cd hermes
   docker-compose up -d kafka kafka-ui
   ```

2. **Run hermes-pipeline with mock data:**
   ```bash
   RUST_LOG=debug USE_MOCK=true cargo run --bin hermes-pipeline
   ```

3. **Run search-indexer:**
   ```bash
   KAFKA_BROKER=localhost:9092 \
   OPENSEARCH_URL=http://localhost:9200 \
   KAFKA_GROUP_ID=search-indexer-test-$(date +%s) \
   RUST_LOG=debug,search_indexer=debug \
   INDEX_CREATION_ENABLED=true \
   ENTITIES_INDEX_VERSION=0 \
   cargo run -p search-indexer
   ```

### Option B: Run hermes-pipeline in Docker

1. **Start all services:**
   ```bash
   cd hermes
   docker-compose up -d kafka kafka-ui hermes-pipeline
   ```

2. **Wait for hermes-pipeline to finish:**
   ```bash
   docker-compose logs -f hermes-pipeline
   ```

3. **Run search-indexer:**
   ```bash
   KAFKA_BROKER=localhost:9092 \
   OPENSEARCH_URL=http://localhost:9200 \
   KAFKA_GROUP_ID=search-indexer-test-$(date +%s) \
   RUST_LOG=debug,search_indexer=debug \
   INDEX_CREATION_ENABLED=true \
   ENTITIES_INDEX_VERSION=0 \
   cargo run -p search-indexer
   ```

### Hermes Pipeline Mock Events

The hermes-pipeline generates 15 edit events with entities like Alice, Bob, Acme Corp, and various test scenarios. However, **e2e-kafka-search-api provides more focused and controllable test data specifically designed for search testing**.

## Configuration

### Kafka Connection

- **Outside Docker**: Connect to `localhost:9092`
- **Inside Docker**: Connect to `kafka:29092`

### Topic Configuration

- **knowledge.edits**: Entity and relation events
- **curation.scores**: Score events
- **Consumer group**: Configurable via `KAFKA_GROUP_ID` (use unique IDs for fresh consumption)

## Verifying Results

### Check OpenSearch Index

List all indexed entities:
```bash
curl -s "http://localhost:9200/entities/_search?pretty" | jq '.hits.hits[]._source.name'
```

Search for a specific entity:
```bash
curl -s "http://localhost:9200/entities/_search?pretty" -H "Content-Type: application/json" -d '{
  "query": { "match": { "name": "Alice" } }
}'
```

### Query via Search API

Basic search:
```bash
curl --compressed "http://localhost:3000/search?query=alice" | jq
```

Search with filters:
```bash
# Filter by type
TYPE_ID_PERSON="00000000-0000-0000-0000-000000000b01"
curl --compressed "http://localhost:3000/search?query=alice&type_ids=$TYPE_ID_PERSON" | jq

# Search in specific space
SPACE_ID="00000000-0000-4000-8000-000000000001"
curl --compressed "http://localhost:3000/search?query=alice&scope=SPACE_SINGLE&space_id=$SPACE_ID" | jq

# Pagination
curl --compressed "http://localhost:3000/search?query=alice&limit=10&offset=0" | jq
```

### Verify Type Relations

Check type relations for an entity:
```bash
curl -s "http://localhost:9200/entities/_search?pretty" -H "Content-Type: application/json" -d '{
  "query": { "match": { "name": "Alice" } }
}' | jq '.hits.hits[]._source.type_relations'
```

## Troubleshooting

### Reset Kafka Consumer Group

To re-process all events from the beginning:

```bash
docker exec -it hermes-kafka-1 kafka-consumer-groups \
  --bootstrap-server localhost:9092 \
  --group search-indexer \
  --reset-offsets --to-earliest --execute \
  --topic knowledge.edits

docker exec -it hermes-kafka-1 kafka-consumer-groups \
  --bootstrap-server localhost:9092 \
  --group search-indexer-scores \
  --reset-offsets --to-earliest --execute \
  --topic curation.scores
```

### View Events in Kafka UI

Open http://localhost:8080 to inspect messages in the Kafka topics.

### Enable Debug Logging

For e2e-kafka-search-api:
```bash
cd search-indexer/tests/e2e-kafka-search-api
cargo run
```

For search-indexer:
```bash
RUST_LOG=debug,search_indexer=trace cargo run -p search-indexer
```

## Related Documentation

- [e2e-kafka-search-api README](tests/e2e-kafka-search-api/README.md) - Detailed CLI documentation
- [Test Event Generation](tests/TEST_EVENTS.md) - Overview of test tools
- [TypeScript Validation](tests/e2e-kafka-search-api/typescript/TYPESCRIPT_VALIDATION.md) - Validation approach
