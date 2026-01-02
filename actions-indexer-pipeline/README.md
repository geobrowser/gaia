# Actions Indexer Pipeline

This crate defines the core traits and modules for processing actions within the indexer. It establishes the pipeline components for consuming, loading, processing, and orchestrating action events.

## Overview

The `actions-indexer-pipeline` crate consists of the following key modules:

- **Consumer:** Responsible for ingesting raw action events from a data source (Substreams or Kafka).
- **Processor:** Handles the business logic and transformations of raw action events into structured action data.
- **Loader:** Manages the persistence of processed action data into the repository.
- **Orchestrator:** Coordinates the flow between the consumer, processor, and loader, ensuring a seamless data pipeline.

## Consumer Providers

The consumer module supports multiple data source providers through the `ConsumeActionsStream` trait:

### SubstreamsStreamProvider

Consumes action events directly from blockchain via Substreams. This is the legacy approach.

### KafkaStreamProvider

Consumes action events from the Hermes Kafka stream. This is the recommended approach for production.

```rust
use actions_indexer_pipeline::consumer::kafka::{ConsumerConfig, KafkaStreamProvider};

// Create configuration
let config = ConsumerConfig::from_env("localhost:9092", "my-consumer-group", "curation.votes");

// Create provider
let provider = KafkaStreamProvider::new(config);
```

#### Features

- **Automatic offset management** via Kafka consumer groups
- **Manual commit after processing** ensures at-least-once delivery
- **Error handling** with retry for transient errors, skip for permanent errors
- **Exponential backoff** for Kafka connection issues
- **Protobuf decoding** of `HermesVoteCast` messages
- **ABI decoding** of vote data fields

#### Message Conversion

The `KafkaStreamProvider` converts `HermesVoteCast` protobuf messages to `ActionRaw`:

| HermesVoteCast Field | ActionRaw Field | Notes |
|---------------------|-----------------|-------|
| `voter_id` | `user_id` | UUID (16 bytes) |
| `object_id` | `object_id` | UUID (16 bytes) |
| `object_type` | `object_type` | "entity" or "relation" |
| `direction` | `metadata[0]` | 0=Up, 1=Down, 2=None |
| `data` (ABI-encoded) | `action_version`, `group_id`, `space_pov` | `(uint16, bytes16, bytes16)` |
| `meta.block_number` | `block_number` | Block number |
| `meta.created_at` | `block_timestamp` | Timestamp |

## Usage

This crate provides the foundational interfaces and structures for building an action indexing pipeline. It is typically used as a dependency by the `actions-indexer` application, which ties these components together into a functional system.

To include this crate in your project, add the following to your `Cargo.toml`:

```toml
[dependencies]
actions-indexer-pipeline = { path = "../actions-indexer-pipeline" }
```

## Testing

Run unit tests:

```bash
cargo test -p actions-indexer-pipeline
```

Run Kafka integration tests (requires running Kafka broker):

```bash
cargo test -p actions-indexer-pipeline --test kafka_integration -- --ignored
```
