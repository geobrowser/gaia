# Running Search-Indexer with Hermes Events

## Problem

The `search-indexer` needs to consume events from the `knowledge.edits` Kafka topic that are produced by `hermes-pipeline`. However, `hermes-pipeline` processes all mock events and then exits, so you need to ensure:

1. `hermes-pipeline` has run and published events to Kafka
2. `search-indexer` is running to consume those events

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

## Solution

### Option 1: Run with cargo (Recommended)

1. **Start Kafka:**
   ```bash
   cd hermes
   docker-compose up -d kafka kafka-ui
   ```

2. **In one terminal, run hermes-pipeline with mock data:**
   ```bash
   RUST_LOG=debug USE_MOCK=true cargo run --bin hermes-pipeline
   ```
   You should see logs like:
   ```
   Edit published name=Create Persons space_id=... ops_count=2
   Block processed spaces=11 trust_added=14 trust_removed=0 governance=0 edits=9
   ```

3. **Verify events are in Kafka:**
   - Open http://localhost:8080 (Kafka UI)
   - Navigate to the `knowledge.edits` topic
   - You should see 9 edit messages

4. **In another terminal, run search-indexer:**
   ```bash
   KAFKA_BROKER=localhost:9092 \
   OPENSEARCH_URL=http://localhost:9200 \
   KAFKA_GROUP_ID=search-indexer-test-$(date +%s) \
   RUST_LOG=debug,search_indexer=debug \
   cargo run -p search-indexer
   ```

### Option 2: Run hermes-pipeline in Docker

1. **Start Kafka and hermes-pipeline:**
   ```bash
   cd hermes
   docker-compose up -d kafka kafka-ui hermes-pipeline
   ```

2. **Wait for hermes-pipeline to finish** (it processes all mock events and exits):
   ```bash
   docker-compose logs -f hermes-pipeline
   ```

3. **Run search-indexer:**
   ```bash
   KAFKA_BROKER=localhost:9092 \
   OPENSEARCH_URL=http://localhost:9200 \
   KAFKA_GROUP_ID=search-indexer-test-$(date +%s) \
   RUST_LOG=debug,search_indexer=debug \
   cargo run -p search-indexer
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
   - Alice -> Person type
   - Alice -> Organization type (she is a founder, so has 2 types)
   - Bob → Person type
   - Acme Corp → Organization type
   - Project Alpha → Project type (will be deleted)
   - Project Alpha → Organization type (secondary, survives the delete)
5. `QmRootEdit5DeleteTypeRelation` - Deletes the Project type from Project Alpha (Organization type remains)
6. `QmSpaceAEdit1CreateOrg` - Creates "Acme Corp" organization
7. `QmSpaceAEdit2CreateRelations` - Creates relations between persons and org (BELONGS_TO, not type relations)
8. `QmSpaceBEdit1CreateDoc` - Creates "Project Alpha" and "Technical Specification"
9. `QmSpaceCEdit1CreateTopic` - Creates "Blockchain Technology" topic

These events contain:
- Entities with `name` and `description` properties
- Type relations (CreateRelation with `TYPE_RELATION_TYPE_ID`) that the search-indexer indexes into `type_relations`
- A DeleteRelation operation to test type relation removal

### Expected Entities After Processing

After all 9 events are processed, the search index should contain **11 documents**. 

Note: Documents are keyed by `(entity_id, space_id)`, so the same entity can have multiple documents if it's referenced in different spaces. Type relations created in the root space create separate documents from the entity properties created in other spaces.

| Entity ID | Space | Name | Description | Type Relations |
|-----------|-------|------|-------------|----------------|
| `...f1` | Root (`...01`) | Alice | A software developer | -> Person (`...b1`), -> Organization (`...b2`) |
| `...f2` | Root (`...01`) | Bob | A project manager | → Person (`...b1`) |
| `...f3` | Root (`...01`) | - | - | → Organization (`...b2`) |
| `...f3` | Space A (`...0a`) | Acme Corp | A technology company | - |
| `...f4` | Root (`...01`) | - | - | → Organization (`...b2`) *(Project type was deleted)* |
| `...f4` | Space B (`...0b`) | Project Alpha | A groundbreaking project | - |
| `...f5` | Space B (`...0b`) | Technical Specification | - | - |
| `...f6` | Space C (`...0c`) | Blockchain Technology | Distributed ledger technology | - |
| `...b1` | Root (`...01`) | Person | A human being | - |
| `...b2` | Root (`...01`) | Organization | A structured group of people | - |
| `...b3` | Root (`...01`) | Project | A planned endeavor | - |

### Verify entities are indexed

List all indexed entities:
```bash
curl -s "http://localhost:9200/entities/_search?pretty" | jq '.hits.hits[]._source.name'
```

Search for a specific entity by name:
```bash
curl -s "http://localhost:9200/entities/_search?pretty" -H "Content-Type: application/json" -d '{
  "query": { "match": { "name": "Alice" } }
}'
```

### Verify Type Relations

Type relations are stored on the entity document in the **root space** (where the type relation was created), not on the entity document in the space where properties were set.

Check that "Project Alpha" (`f4`) in root space has Organization type but NOT Project type:
```bash
# Get Project Alpha's type relations from root space
curl -s "http://localhost:9200/entities/_search?pretty" -H "Content-Type: application/json" -d '{
  "query": {
    "bool": {
      "must": [
        { "term": { "entity_id": "00000000-0000-0000-0000-0000000000f4" } },
        { "term": { "space_id": "00000000-0000-4000-8000-000000000001" } }
      ]
    }
  }
}' | jq '.hits.hits[]._source.type_relations'
```

Expected result: Should show Organization type (`...b2`), NOT Project type (`...b3`).

Verify Alice has 2 type relations (Person and Organization):
```bash
curl -s "http://localhost:9200/entities/_search?pretty" -H "Content-Type: application/json" -d '{
  "query": { "match": { "name": "Alice" } }
}' | jq '.hits.hits[]._source.type_relations'
```

Expected result: Should show 2 type relations - Person (`...b1`) and Organization (`...b2`).

Get all entities with type relations:
```bash
curl -s "http://localhost:9200/entities/_search?pretty" -H "Content-Type: application/json" -d '{
  "query": { "exists": { "field": "type_relations" } }
}' | jq '.hits.hits[]._source | {name, entity_id, type_relations}'
```
