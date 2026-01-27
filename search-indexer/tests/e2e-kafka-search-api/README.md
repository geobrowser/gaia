# E2E Kafka Search API Test Tool

A command-line tool for generating test events that the search-indexer consumes from Kafka topics. This tool allows you to test the search indexer independently without relying on the full hermes-pipeline infrastructure.

## Features

- Generates a comprehensive test scenario with 15 entities, relations, and scores
- Creates entities with varying score profiles (positive, negative, zero, at/below thresholds)
- Tests type relation scenarios (multiple types, create/delete/recreate patterns)
- Produces entity, space, and perspective scores
- Uses fixed UUIDs for validation testing

## Prerequisites

- Rust toolchain (edition 2024)
- Local Kafka broker running (default: `localhost:9092`)
- Search indexer ready to consume from topics

## Installation

Build the tool from the `search-indexer/tests/e2e-kafka-search-api` directory:

```bash
cd search-indexer/tests/e2e-kafka-search-api
cargo build --release
```

The binary will be available at `target/release/e2e-kafka-search-api`.

## Quick Start

### Option 1: Use the Test Script (Recommended)

The fastest way to generate test events and validate the search API:

```bash
cd search-indexer/tests/e2e-kafka-search-api
./run-test.sh
```

This script will:
1. Build the event generator
2. Generate comprehensive test data (15 entities, relations, and scores)
3. Automatically run TypeScript validation tests if the search API is running

### Option 2: Manual Setup

#### 1. Start Kafka

```bash
cd hermes
docker-compose up -d kafka kafka-ui
```

#### 2. Generate Test Data

Run the tool to generate comprehensive test data:

```bash
cd search-indexer/tests/e2e-kafka-search-api
cargo run --release
```

This creates:
- 15 entities (7 Alice variants with different scores, Bob, Charlie, Acme Corp, Person type, Organization type, 3 property test entities)
- 15 type relation events (including creates, deletes, and recreates for testing typeIds)
- 11 entity scores (including negative and zero values)
- 1 space score
- 7 perspective scores
- 3 property operation test cases (2 unset tests, 1 mixed set/unset + LWW test)

#### 3. Start the Search Indexer

```bash
cd ../../..  # Back to repo root
KAFKA_BROKER=localhost:9092 \
OPENSEARCH_URL=http://localhost:9200 \
KAFKA_GROUP_ID=search-indexer-test-$(date +%s) \
RUST_LOG=debug,search_indexer=debug \
cargo run -p search-indexer --features search-indexer-repository/auto_index_creation
```

**Note:** The `--features search-indexer-repository/auto_index_creation` flag enables automatic OpenSearch index creation for local testing. This feature is disabled by default for production safety. In production deployments, indices must be created manually using the search-admin tool.

#### 4. (Optional) Run Validation Tests

If you have the search API running, you can validate the results:

```bash
cd search-indexer/tests/e2e-kafka-search-api/typescript
npm run validate
```

## Usage

Run the tool to generate comprehensive test data:

```bash
e2e-kafka-search-api [OPTIONS]

Options:
  -b, --broker <BROKER>  Kafka broker address [default: localhost:9092]
  -d, --debug            Enable debug logging
  -h, --help             Print help
```

### Example

Generate test data with custom Kafka broker:

```bash
e2e-kafka-search-api --broker kafka.example.com:9092 --debug
```

## Configuration

### Environment Variables

While not required, you can set default values using environment variables:

```bash
export KAFKA_BROKER=localhost:9092
export RUST_LOG=info
```

### Custom Kafka Broker

Use a different Kafka broker:

```bash
e2e-kafka-search-api --broker kafka.example.com:9092
```

The tool always uses the standard Kafka topics:
- `knowledge.edits` for entity and relation events
- `curation.scores` for score events

## Property IDs

The tool uses these well-known property IDs:

- **NAME_PROPERTY_ID**: `a126ca53-0c8e-48d5-b888-82c734c38935`
- **DESCRIPTION_PROPERTY_ID**: `9b1f76ff-9711-404c-861e-59dc3fa7d037`
- **AVATAR_PROPERTY_ID**: `1155beff-fad5-49b7-a2e0-da4777b8792c`
- **TYPE_RELATION_TYPE_ID**: `8f151ba4-de20-4e3c-9cb4-99ddf96f48f1`

## Kafka Topics

The tool uses these standard Kafka topics:
- **knowledge.edits**: Entity and relation events
- **curation.scores**: Score events

## Verifying Events

### Automated Validation (TypeScript)

The recommended way to validate search results is using the TypeScript validation script:

```bash
cd search-indexer/tests/e2e-kafka-search-api/typescript
npm run validate
```

This script:
- Uses the actual TypeScript types from `api/src/services/search/types.ts`
- Runs comprehensive validation tests:
  - Test 1: Basic Alice search (validates 7 entities ordered by score)
  - Test 2: Bob search (validates 1 entity with correct fields)
  - Test 3: Organization search (validates Acme Corp)
  - Test 4: Entity field validation (entityId, name, description, typeIds, scoring)
  - Test 5: Score-based ordering (high scores first, descending order)
  - Test 6: Response metadata (total count, execution time)
  - Test 7: Zero and negative score entities
  - Test 8: TypeIds field for different relation scenarios
  - Test 9: Empty query returns top ranked results
  - Test 10: Unset properties functionality (validates unset_values in UpdateEntity)
  - Test 11: Mixed set/unset + LWW behavior (validates Last-Writer-Wins semantics)
- Provides color-coded pass/fail reporting
- Exits with appropriate status codes for CI/CD integration

The validation script is type-safe and uses the same interfaces as the API:
- `SearchQuery` - for constructing queries
- `SearchResponse` - for validating responses
- `SearchResult` - for checking entity fields

### Using Kafka UI

Open http://localhost:8080 and navigate to:
- `knowledge.edits` topic for edit events
- `curation.scores` topic for score events

### Using Search Indexer Logs

The search indexer will log consumed events:

```
Processing entity event: Upsert for entity 00000000-0000-0000-0000-0000000000f1 in space 00000000-0000-4000-8000-000000000001
```

### Using OpenSearch

Query the search index directly:

```bash
# List all entities
curl -s "http://localhost:9200/entities/_search?pretty" | jq '.hits.hits[]._source.name'

# Search for specific entity
curl -s "http://localhost:9200/entities/_search?pretty" -H "Content-Type: application/json" -d '{
  "query": { "match": { "name": "Alice" } }
}'
```

### Using the Search API

Query indexed entities through the API:

```bash
# Basic search (defaults to GLOBAL scope)
curl --compressed "http://localhost:3000/search?query=alice" | jq

# Search with explicit scope
curl --compressed "http://localhost:3000/search?query=alice&scope=GLOBAL" | jq

# Search by space score (global search ranked by space importance)
curl --compressed "http://localhost:3000/search?query=alice&scope=GLOBAL_BY_SPACE_SCORE" | jq

# Search within a specific space
SPACE_ID="00000000-0000-4000-8000-000000000001"
curl --compressed "http://localhost:3000/search?query=alice&scope=SPACE_SINGLE&space_id=$SPACE_ID" | jq

# Filter by type IDs (comma-separated)
TYPE_ID_PERSON="00000000-0000-0000-0000-000000000b01"
TYPE_ID_ORG="00000000-0000-0000-0000-000000000b02"
curl --compressed "http://localhost:3000/search?query=alice&type_ids=$TYPE_ID_PERSON,$TYPE_ID_ORG" | jq

# Pagination
curl --compressed "http://localhost:3000/search?query=alice&limit=10&offset=0" | jq

# Combined filters
curl --compressed "http://localhost:3000/search?query=alice&scope=SPACE_SINGLE&space_id=$SPACE_ID&type_ids=$TYPE_ID_PERSON&limit=20" | jq
```

## Test Scenario Details

The tool generates a comprehensive test scenario with the following test data:

### Entities Created

**Alice Variants** (7 entities with different score profiles):
- Alice High (score: 0.95) - Tests high positive scores and multiple type relations
- Alice Medium (score: 0.65) - Tests create/delete/recreate type relation patterns
- Alice Low (score: 0.15) - Tests partial type removal (Org type added then deleted)
- Alice Zero (score: 0.0) - Tests exact zero score handling
- Alice Negative (score: -0.75) - Tests negative score handling (z-scores)
- Alice At Threshold (score: 0.50) - Tests typical threshold boundary
- Alice Below Threshold (score: 0.25) - Tests below threshold handling

**Other Entities**:
- Bob (score: 0.75) - Basic entity with single type
- Charlie (no score) - Tests default score behavior when entity has no global score
- Acme Corp (score: 0.90) - Organization entity
- Person Type (score: 0.70) - Type definition entity
- Organization Type (score: 0.65) - Type definition entity

### Type Relations

- All Alice variants, Bob, and Charlie have Person type
- Acme Corp has Organization type
- Alice High additionally has Organization type (multiple types test)
- Alice Medium has Organization type added, deleted, then recreated (relation lifecycle test)
- Alice Low has Organization type added then deleted (final state: only Person type)

### Scores Generated

- 11 entity scores (including negative and zero values)
- 1 space score
- 7 perspective scores (entity-space combinations)

All entities use fixed UUIDs for repeatable validation testing.

### Property Operation Test Cases

Three dedicated test entities verify property operations in UpdateEntity:

**Test Case 1** - Unset Single Property (ID: `00000000-0000-0000-0000-000000001111`):
- Creates entity with name and description
- Unsets the name property
- Expected result: name should be undefined/null, description should remain

**Test Case 2** - Unset Multiple Properties (ID: `00000000-0000-0000-0000-000000002222`):
- Creates entity with name, description, and avatar
- Unsets name and description properties
- Expected result: name and description should be undefined/null, avatar should remain

**Test Case 3** - Mixed Set/Unset + LWW (ID: `00000000-0000-0000-0000-000000003333`):
- Creates entity with initial name, description, and avatar
- Sends mixed operation: sets name="First Update", unsets description
- Sends second set: name="Second Update"
- Expected result: name="Second Update" (last write wins), description unset, avatar preserved
- Tests both mixed operations (set and unset different properties) and Last-Writer-Wins semantics

#### Verifying Property Operations

The TypeScript validation script automatically verifies property operations:

**Test 10** - Unset Properties (Test Cases 1 & 2):
- ✓ Verifies name is undefined/null for Test Case 1
- ✓ Verifies description is still present for Test Case 1
- ✓ Verifies name and description are undefined/null for Test Case 2
- ✓ Verifies avatar is still present for Test Case 2

**Test 11** - Mixed Set/Unset + LWW (Test Case 3):
- ✓ Verifies name equals "Second Update" (last write wins)
- ✓ Verifies description is unset (from mixed operation)
- ✓ Verifies avatar is preserved across operations

Run the validation script to test:
```bash
cd typescript
npm run validate
```

The property operations are handled by the search-indexer's `process_update_entity` function, which:
1. Processes UpdateEntity operations with both `set_properties` and `unset_values` (for different properties)
2. Creates separate `EntityEvent::unset_properties` and `EntityEvent::upsert` events
3. The OpenSearch provider flushes unset operations immediately to ensure proper ordering
4. Last-Writer-Wins semantics are achieved through sequential operations

## Troubleshooting

### Kafka Connection Issues

If you get connection errors:

1. Check Kafka is running:
   ```bash
   docker-compose ps kafka
   ```

2. Verify the broker address:
   ```bash
   e2e-kafka-search-api --broker localhost:9092 --debug
   ```

### Search Indexer Not Consuming

1. Check the consumer group ID - each run should use a unique group ID:
   ```bash
   KAFKA_GROUP_ID=search-indexer-test-$(date +%s)
   ```

2. Or reset the consumer group to start from the beginning:
   ```bash
   docker exec -it hermes-kafka-1 kafka-consumer-groups \
     --bootstrap-server localhost:9092 \
     --group search-indexer \
     --reset-offsets --to-earliest --execute \
     --topic knowledge.edits
   ```

### Missing Events

Enable debug logging to see exactly what's being sent:

```bash
e2e-kafka-search-api --debug
```

## Directory Structure

```
e2e-kafka-search-api/
├── src/                      # Rust source code
│   ├── generators/          # Event generation modules
│   │   ├── edits.rs        # Entity edit events
│   │   ├── relations.rs    # Relation events
│   │   ├── scores.rs       # Score events
│   │   └── mod.rs          # Module exports
│   ├── kafka.rs            # Kafka producer wrapper
│   └── main.rs             # CLI interface
├── typescript/              # TypeScript validation
│   ├── validate-search.ts  # Validation script
│   ├── package.json        # Node dependencies
│   ├── tsconfig.json       # TS configuration
│   └── TYPESCRIPT_VALIDATION.md  # TS docs
├── Cargo.toml              # Rust dependencies
├── README.md               # This file
├── run-test.sh             # Quick start script
├── examples.sh             # Example scenarios
└── .gitignore              # Git ignore rules
```

## Development

### Running Tests

```bash
cargo test
```

### Modifying the Test Scenario

To modify the test data generated:

1. Edit the test scenario code in `src/main.rs`
2. Update the generator functions in `src/generators/` as needed
3. Update this README and test documentation

## Related Documentation

- [TypeScript Validation Documentation](./typescript/TYPESCRIPT_VALIDATION.md) - Details on the TypeScript validation approach
- [TypeScript Directory README](./typescript/README.md) - Quick reference for validation scripts
- [Search Indexer Testing Guide](../../TESTING.md) - End-to-end testing documentation
- [Hermes Schema Proto Definitions](../../../hermes-schema/proto/) - Protobuf message definitions
- [Search Indexer README](../../README.md) - Main search-indexer documentation
