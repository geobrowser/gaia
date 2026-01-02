# Actions Indexer

This crate provides the main application for indexing actions. It orchestrates the entire process of consuming action events from either a Substreams endpoint or a Hermes Kafka stream, processing them through registered handlers, and persisting the results to a PostgreSQL database.

## Overview

The `actions-indexer` application follows a consumer-processor-loader architecture and is responsible for:

- **Consuming**: Reading action events from Substreams or Kafka (Hermes)
- **Processing**: Handling actions through registered handlers (currently supports vote actions)
- **Loading**: Persisting processed actions to a PostgreSQL database
- **Orchestrating**: Coordinating the data flow through the entire pipeline

## Architecture

The application is built on three main components:

- **ActionsConsumer**: Consumes actions using either `SubstreamsStreamProvider` or `KafkaStreamProvider`
- **ActionsProcessor**: Processes actions through registered handlers (e.g., `VoteHandler` for vote actions)
- **ActionsLoader**: Persists processed actions using the `PostgresActionsRepository`

### Data Sources

The indexer supports two data sources, configurable via the `DATA_SOURCE` environment variable:

| Data Source | Description | Use Case |
|-------------|-------------|----------|
| `kafka` | Consumes from Hermes Kafka stream | Production (recommended) |
| `substreams` | Consumes directly from blockchain via Substreams | Legacy/fallback |

## Supported Actions

Currently, the indexer supports the following action types:

- **Vote Actions**: Handles voting with values Up (0), Down (1), and Remove (2)

## Actions Mapping Spec

The [Actions Interface](https://github.com/defi-wonderland/geo-actions/blob/63bb7507bcdff9d71c4edbff698536d8cf2e7d28/src/interfaces/IActions.sol#L9) defines the structure for action events. Each action contains the following fields:

### Action Event Fields

| Field | Description | Type |
|-------|-------------|------|
| `kind` | Type of action being performed | `uint16` |
| `version` | Version identifier for the action schema | `uint16` |
| `objectType` | Identifier for the target object type | `uint8` |
| `spacePOV` | Unique identifier for the space context | `bytes16` |
| `groupId` | Group identifier for organizing actions | `bytes16` |
| `objectId` | Unique identifier for the target object | `bytes16` |
| `payload` | Action-specific data payload | `bytes` |

### Event Type Mappings

| Value | Event Type | Description |
|-------|------------|-------------|
| `0` | Voting | User voting actions (up/down/remove) |

### Object Type Mappings

| Value | Object Type | Description |
|-------|-------------|-------------|
| `0` | Entity | Actions targeting entities |
| `1` | Relation | Actions targeting relations |

### Payload Structure

#### Voting Events (event_type = 0, version = 1)

For voting events, the payload is a single byte indicating the vote type:

| Value | Vote Type | Description |
|-------|-----------|-------------|
| `0x00` | Upvote | Positive vote |
| `0x01` | Downvote | Negative vote |
| `0x02` | Remove Vote | Remove existing vote |

### Example

**Scenario**: Upvote on entity `3138715a-62a7-4b9f-b2a9-13bedf987a1b` within space `9b4f7ccf-6a7c-4ef4-9a63-b2b818e2a1d3`

```
kind: 0 (voting)
version: 1
objectType: 0 (entity)
spacePOV: 0x9b4f7ccf6a7c4ef49a63b2b818e2a1d3
groupId: 0
objectId: 0x3138715a62a74b9fb2a913bedf987a1b
payload: 0x00 (upvote)
```

## Configuration

### Environment Variables

Copy `.env.example` to `.env` and configure the required variables.

#### Required Variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL database connection string |
| `DATA_SOURCE` | Data source selection: `kafka` (recommended) or `substreams` |

#### Kafka Configuration (when `DATA_SOURCE=kafka`)

| Variable | Required | Description |
|----------|----------|-------------|
| `KAFKA_BROKER` | Yes | Kafka broker address (e.g., `localhost:9092`) |
| `KAFKA_CONSUMER_GROUP` | Yes | Consumer group ID - must be stable across restarts |
| `KAFKA_TOPIC` | Yes | Topic to consume vote events from (e.g., `curation.votes`) |
| `KAFKA_USERNAME` | No | SASL username for managed Kafka (e.g., Confluent Cloud) |
| `KAFKA_PASSWORD` | No | SASL password for managed Kafka |
| `KAFKA_SSL_CA_PEM` | No | Custom CA certificate for SSL (PEM format) |

#### Substreams Configuration (when `DATA_SOURCE=substreams`)

| Variable | Required | Description |
|----------|----------|-------------|
| `SUBSTREAMS_ENDPOINT` | Yes | The Substreams API endpoint URL |
| `SUBSTREAMS_API_TOKEN` | Yes | Authentication token for Substreams API access |

### Example Configuration

```bash
# Common
DATABASE_URL=postgresql://user:password@localhost:5432/actions_indexer
DATA_SOURCE=kafka

# Kafka (recommended for production)
KAFKA_BROKER=localhost:9092
KAFKA_CONSUMER_GROUP=actions-indexer
KAFKA_TOPIC=curation.votes

# Or Substreams (legacy)
# DATA_SOURCE=substreams
# SUBSTREAMS_ENDPOINT=https://mainnet.eth.streamingfast.io:443
# SUBSTREAMS_API_TOKEN=your-api-token
```

### Substreams Package

When using Substreams as the data source, the application uses a packaged Substreams module located at:
- **Package**: `./geo-actions-v0.1.0.spkg`
- **Module**: `map_actions`

## Database Setup

Before running the application, you need to create the required database tables. You have several options:

```bash
# Extract connection details from DATABASE_URL or use it directly
psql $DATABASE_URL -f ../actions-indexer-repository/src/postgres/migrations/0000_init_actions.sql
```

The migration will create the following tables:
- `raw_actions` - Stores processed blockchain actions
- `user_votes` - Individual voting records  
- `votes_count` - Aggregated vote tallies per entity/space

## Build and Run

To build the `actions-indexer` application:

```bash
cargo build
```

To run the application:

```bash
cargo run
```