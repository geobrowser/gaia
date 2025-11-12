# Kafka Producer with ZSTD Compression

A high-performance Kafka producer written in Rust with ZSTD compression and Protocol Buffers serialization.

## Features

- **Protocol Buffers** for compact, type-safe serialization
- **ZSTD compression** for optimal bandwidth and storage efficiency
- **Async/await** using Tokio runtime
- **Batching** for better compression ratios
- **Type-safe** event definitions generated from protobuf schemas
- **Environment-based** configuration

## Prerequisites

- Rust (latest stable version)
- Protocol Buffers compiler (`protoc`)
- Kafka broker running (default: localhost:9092)

## Schema Management

Protobuf schemas are centrally managed in `../schemas/`. To update schemas:

```bash
cd ../schemas
# Edit proto/user_event.proto
make validate  # Validate syntax
make sync      # Update k8s ConfigMap
```

The producer automatically uses the central schema via `build.rs` at compile time. See `../schemas/README.md` for details.

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

## Configuration

Set the following environment variables:

- `KAFKA_BROKER` - Kafka broker address (default: `localhost:9092`)

## Producer Settings

The producer is configured with:

- **Compression**: ZSTD (best compression ratio)
- **Batching**: Up to 10,000 messages per batch
- **Timeout**: 5 seconds message timeout
- **Buffer**: 1GB queue buffer

## Event Structure

Events are serialized using Protocol Buffers. The schema is defined in `../schemas/proto/user_event.proto`:

```protobuf
message UserEvent {
  string event_id = 1;
  int64 timestamp = 2;
  string user_id = 3;
  EventType event_type = 4;
  UserEventData data = 5;
}

enum EventType {
  UNKNOWN = 0;
  USER_REGISTERED = 1;
  USER_LOGIN = 2;
  USER_LOGOUT = 3;
}

message UserEventData {
  optional string email = 1;
  optional string username = 2;
  map<string, string> metadata = 3;
}
```

## Performance

ZSTD compression typically achieves:
- 60-70% size reduction on JSON payloads
- Better compression than gzip with less CPU overhead
- Optimal for high-throughput scenarios
