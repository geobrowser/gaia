# Hermes Spaces Transformer

A transformer binary that consumes space-related events from `hermes-substream` via `hermes-relay` and publishes them to Kafka topics.

## Overview

This transformer is part of the Hermes architecture (see `docs/hermes-architecture.md`). It:

1. Connects to the blockchain data source via `hermes-relay`
2. Subscribes to `HermesModule::Actions` to receive all raw actions
3. Filters client-side for space-related events
4. Transforms events into Hermes protobuf messages
5. Publishes to Kafka topics for downstream consumers

## Event Types

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `SPACE_REGISTERED` | New space registrations | `space.creations` |
| `SUBSPACE_ADDED` | Trust extensions (verified/related/subtopic) | `space.trust.extensions` |
| `SUBSPACE_REMOVED` | Trust revocations | `space.trust.extensions` |

## Configuration

### Required Environment Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `SUBSTREAMS_ENDPOINT` | Substreams gRPC endpoint URL | `https://mainnet.eth.streamingfast.io` |

### Optional Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `SUBSTREAMS_API_TOKEN` | Auth token for substreams | - |
| `KAFKA_BROKER` | Kafka broker address | `localhost:9092` |
| `KAFKA_USERNAME` | SASL username for managed Kafka | - |
| `KAFKA_PASSWORD` | SASL password for managed Kafka | - |
| `KAFKA_SSL_CA_PEM` | Custom CA cert for SSL (PEM format) | - |
| `START_BLOCK` | Block number to start from | `0` |
| `END_BLOCK` | Block number to stop at (0 = live streaming) | `0` |

## Usage

### Local Development

```bash
# Start local Kafka (see hermes/docker-compose.yaml)
docker-compose -f hermes/docker-compose.yaml up -d

# Run the transformer
SUBSTREAMS_ENDPOINT=https://mainnet.eth.streamingfast.io \
SUBSTREAMS_API_TOKEN=your-token \
KAFKA_BROKER=localhost:9092 \
cargo run --package hermes-spaces
```

### Docker

```bash
# Build
docker build -f hermes-spaces/Dockerfile -t hermes-spaces .

# Run
docker run \
  -e SUBSTREAMS_ENDPOINT=https://mainnet.eth.streamingfast.io \
  -e SUBSTREAMS_API_TOKEN=your-token \
  -e KAFKA_BROKER=localhost:9092 \
  hermes-spaces
```

## Architecture

```
┌─────────────────┐     ┌──────────────┐     ┌─────────────────┐
│ hermes-substream│────▶│ hermes-relay │────▶│  hermes-spaces  │
│  (blockchain)   │     │   (stream)   │     │  (transformer)  │
└─────────────────┘     └──────────────┘     └────────┬────────┘
                                                      │
                                                      ▼
                                             ┌─────────────────┐
                                             │      Kafka      │
                                             │  ┌───────────┐  │
                                             │  │ space.    │  │
                                             │  │ creations │  │
                                             │  └───────────┘  │
                                             │  ┌───────────┐  │
                                             │  │ space.    │  │
                                             │  │ trust.    │  │
                                             │  │extensions │  │
                                             │  └───────────┘  │
                                             └─────────────────┘
```

## Why Client-Side Filtering?

The substreams protocol only supports consuming a single output module per stream in production mode. Since the spaces transformer needs multiple event types (space registrations AND subspace changes), we:

- Subscribe to `map_actions` (all raw actions)
- Filter client-side using action type constants from `hermes_relay::actions`

See `hermes-relay/docs/decisions/0001-multiple-substreams-modules-consumers.md` for more details.

## Future Work

- **Cursor persistence**: Add PostgreSQL/Redis storage for cursor to resume from last processed block
- **Metrics**: Add Prometheus metrics for monitoring
- **Trust type decoding**: Decode the `data` field to distinguish between verified/related/subtopic extensions
