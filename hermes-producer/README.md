# Hermes Producer

A mock Kafka producer for testing the Hermes event stream service. Emits randomized HermesEdit messages to simulate real-world Geo knowledge graph edits.

## Features

- **Mock Data Generation** - Creates random HermesEdit messages with realistic structure
- **Continuous Streaming** - Emits messages every 3 seconds
- **Protocol Buffers** for compact, type-safe serialization
- **ZSTD compression** for optimal bandwidth and storage efficiency
- **Randomized Content** - Random entities, properties, and relations for testing
- **Environment-based** configuration

## Prerequisites

- Rust (latest stable version)
- Protocol Buffers compiler (`protoc`)
- Kafka broker running (default: localhost:9092)

## Mock Data Generation

The producer generates randomized HermesEdit messages with:
- Random UUIDs for entity/property/relation IDs
- Random space IDs (space-1 through space-5)
- Random block numbers (1,000,000 - 9,999,999)
- Random author addresses (32 bytes)
- 1-4 random operations per edit
- 80% probability of being canonical
- Sequential naming (Random Edit #1, #2, etc.)

## Building

```bash
cargo build --release
```

**Note:** The build process automatically generates Rust code from the protobuf schema.

## Running

```bash
# Use default broker (localhost:9092)
cargo run

# Use custom broker
KAFKA_BROKER=kafka.example.com:9092 cargo run
```

The producer will run continuously, emitting a random HermesEdit message every 3 seconds. Press Ctrl+C to stop.

## Configuration

- `KAFKA_BROKER` - Kafka broker address (default: `localhost:9092`)

## Producer Settings

The producer is configured with:

- **Compression**: ZSTD (best compression ratio)
- **Batching**: Up to 10,000 messages per batch
- **Timeout**: 5 seconds message timeout
- **Buffer**: 1GB queue buffer

## Event Structure

Events are serialized using Protocol Buffers. The HermesEdit schema is defined in `../hermes-schema/proto/knowledge.proto`:

```protobuf
message HermesEdit {
  bytes id = 1;
  string name = 2;
  repeated grc20.Op ops = 3;
  repeated bytes authors = 4;
  optional bytes language = 5;
  string space_id = 6;
  bool is_canonical = 7;
  uint64 created_at = 8;
  bytes created_by = 9;
  uint64 block_number = 10;
  string cursor = 11;
}
```

Each message contains 1-4 random operations (Op) which can be:
- **Entity updates** - Random entity with values
- **Property creation** - Random property definitions
- **Relation creation** - Random relations between entities

## Performance

ZSTD compression typically achieves:
- 60-70% size reduction on JSON payloads
- Better compression than gzip with less CPU overhead
- Optimal for high-throughput scenarios
