# Running Search-Indexer with Hermes Events

## Problem

The `search-indexer` needs to consume events from the `knowledge.edits` Kafka topic that are produced by `hermes-pipeline`. However, `hermes-pipeline` processes all mock events and then exits, so you need to ensure:

1. `hermes-pipeline` has run and published events to Kafka
2. `search-indexer` is running to consume those events

## Solution

### Option 1: Run search-indexer after hermes-pipeline (Recommended)

1. **Start Kafka and hermes-pipeline:**
   ```bash
   cd hermes
   docker-compose up -d kafka kafka-ui hermes-pipeline
   ```

2. **Wait for hermes-pipeline to finish** (it processes all mock events and exits):
   ```bash
   docker-compose logs -f hermes-pipeline
   ```
   You should see logs like:
   ```
   Edit published name=Create Persons space_id=... ops_count=2
   Block processed spaces=11 trust_added=14 trust_removed=0 governance=0 edits=6
   ```

3. **Verify events are in Kafka:**
   - Open http://localhost:8080 (Kafka UI)
   - Navigate to the `knowledge.edits` topic
   - You should see 6 edit messages

4. **Run search-indexer** (outside Docker, or in a separate container):
   ```bash
   cd ../search-indexer
   KAFKA_BROKER=localhost:9092 \
   OPENSEARCH_URL=http://localhost:9200 \
   cargo run
   ```

### Option 2: Run search-indexer concurrently

If you want search-indexer to pick up events as they're produced:

1. **Start Kafka:**
   ```bash
   cd hermes
   docker-compose up -d kafka kafka-ui
   ```

2. **In one terminal, run hermes-pipeline:**
   ```bash
   # locally with cargo (from repo root):
   cd hermes
   RUST_LOG=debug cargo run --bin hermes-pipeline
   ```

3. **In another terminal, run search-indexer:**
   ```bash
   cd search-indexer
   KAFKA_BROKER=localhost:9092 \
   OPENSEARCH_URL=http://localhost:9200 \
   RUST_LOG=debug \
   cargo run
   ```

### Option 3: Run search-indexer in Docker

You can also run search-indexer in Docker alongside the hermes services. Add this to `hermes/docker-compose.yaml`:

```yaml
  search-indexer:
    build:
      context: ..
      dockerfile: search-indexer/Dockerfile
    environment:
      KAFKA_BROKER: kafka:29092
      KAFKA_TOPIC: knowledge.edits
      KAFKA_GROUP_ID: search-indexer
      OPENSEARCH_URL: ${OPENSEARCH_URL:-http://opensearch:9200}
      INDEX_ALIAS: entities
      ENTITIES_INDEX_VERSION: 0
    depends_on:
      kafka:
        condition: service_healthy
```

Then run:
```bash
docker-compose up -d
```

## Configuration Details

### Kafka Connection

- **hermes-pipeline** (in Docker): Connects to `kafka:29092` (internal Docker network)
- **search-indexer** (outside Docker): Connects to `localhost:9092` (external port)
- **search-indexer** (in Docker): Should connect to `kafka:29092` (internal Docker network)

Both addresses point to the same Kafka broker, just different network contexts.

### Topic Configuration

- **Topic name**: `knowledge.edits` (default for both services)
- **Message format**: `HermesEdit` protobuf messages
- **Consumer group**: `search-indexer` (configurable via `KAFKA_GROUP_ID`)

### Mock Events Generated

The hermes-pipeline generates 9 edit events from the test topology:
1. `QmRootEdit1CreatePersons` - Creates "Alice" and "Bob" entities
2. `QmRootEdit2AddDescriptions` - Adds descriptions to persons
3. `QmRootEdit3CreateTypes` - Creates type entities ("Person", "Organization", "Project")
4. `QmRootEdit4CreateTypeRelations` - Creates type relations using `TYPE_RELATION_TYPE_ID`:
   - Alice → Person type
   - Bob → Person type
   - Acme Corp → Organization type
   - Project Alpha → Project type
   - Project Alpha → Organization type (secondary, survives the delete)
5. `QmRootEdit5DeleteTypeRelation` - Deletes the Project type from Project Alpha (Organization type remains)
6. `QmSpaceAEdit1CreateOrg` - Creates "Acme Corp" organization
7. `QmSpaceAEdit2CreateRelations` - Creates relations between persons and org (BELONGS_TO, not type relations)
8. `QmSpaceBEdit1CreateDoc` - Creates "Project Alpha" and "Technical Specification"
9. `QmSpaceCEdit1CreateTopic` - Creates "Blockchain Technology" topic

These events contain:
- Entities with `name`, `description`, and `avatar` properties
- Type relations (CreateRelation with `TYPE_RELATION_TYPE_ID`) that the search-indexer indexes into `type_relations`
- A DeleteRelation operation to test type relation removal

### Verify entities are indexed:

   ```bash
   curl "http://localhost:9200/entities/_search?q=entity"
   ```

