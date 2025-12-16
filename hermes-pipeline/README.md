# Hermes Pipeline

A transformer binary that consumes space-related events from `hermes-substream` via `hermes-relay` and publishes them to Kafka topics.

## Overview

This transformer is part of the Hermes architecture (see `docs/hermes-architecture.md`). It:

1. Connects to the blockchain data source via `hermes-relay`
2. Subscribes to `HermesModule::Actions` to receive all raw actions
3. Filters client-side for space-related events
4. Transforms events into Hermes protobuf messages
5. Publishes to Kafka topics for downstream consumers

## Event Types

### Space Lifecycle

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `SPACE_REGISTERED` | New space registrations | `space.creations` |

### Trust & Topology

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `SUBSPACE_VERIFIED` | Verified trust extensions | `space.trust.extensions` |
| `SUBSPACE_RELATED` | Related trust extensions | `space.trust.extensions` |
| `SUBSPACE_TOPIC_DECLARED` | Topic-based trust extensions | `space.trust.extensions` |
| `SUBSPACE_REMOVED` | Trust revocations | `space.trust.extensions` |

### Membership

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `EDITOR_ADDED` | Editor granted to space | `space.membership` |
| `EDITOR_REMOVED` | Editor revoked from space | `space.membership` |
| `MEMBER_ADDED` | Member added to space | `space.membership` |
| `MEMBER_REMOVED` | Member removed from space | `space.membership` |
| `SPACE_LEFT` | Member voluntarily left space | `space.membership` |

### Moderation

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `EDITOR_FLAGGED` | Editor flagged in space | `space.moderation` |
| `EDITOR_UNFLAGGED` | Editor unflagged in space | `space.moderation` |
| `FLAGGED` | Content flagged | `space.moderation` |
| `UNFLAGGED` | Content unflagged | `space.moderation` |

### Topics

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `TOPIC_DECLARED` | Topic declared by space | `space.topics` |

### Governance

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `PROPOSAL_CREATED` | Governance proposal created | `space.governance` |
| `PROPOSAL_VOTED` | Vote cast on proposal | `space.governance` |
| `PROPOSAL_EXECUTED` | Proposal executed | `space.governance` |

### Social Voting (Permissionless)

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `UPVOTED` | Object upvoted | `curation.votes` |
| `DOWNVOTED` | Object downvoted | `curation.votes` |
| `UNVOTED` | Vote removed | `curation.votes` |

### Knowledge

| Event | Description | Kafka Topic |
|-------|-------------|-------------|
| `EDITS_PUBLISHED` | Edit publications (fetched from IPFS) | `knowledge.edits` |

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

### Telemetry Environment Variables

If `OTEL_URL` is set, telemetry is exported via OTLP HTTP. Otherwise, logs are written to the console.

| Variable | Description | Default |
|----------|-------------|---------|
| `OTEL_URL` | OTLP HTTP endpoint | - |
| `OTEL_TOKEN` | Bearer token for authentication | - |
| `OTEL_DATASET` | Dataset name (sent as `X-Axiom-Dataset` header) | - |
| `OTEL_DEBUG` | Also emit spans to stdout | `false` |

## Usage

### Local Development

```bash
# Start local Kafka (see hermes/docker-compose.yaml)
docker-compose -f hermes/docker-compose.yaml up -d

# Run the transformer
SUBSTREAMS_ENDPOINT=https://mainnet.eth.streamingfast.io \
SUBSTREAMS_API_TOKEN=your-token \
KAFKA_BROKER=localhost:9092 \
cargo run --package hermes-pipeline
```

### Docker

```bash
# Build
docker build -f hermes-pipeline/Dockerfile -t hermes-pipeline .

# Run
docker run \
  -e SUBSTREAMS_ENDPOINT=https://mainnet.eth.streamingfast.io \
  -e SUBSTREAMS_API_TOKEN=your-token \
  -e KAFKA_BROKER=localhost:9092 \
  hermes-pipeline
```

## Architecture

```
┌─────────────────┐     ┌──────────────┐     ┌─────────────────┐
│ hermes-substream│────▶│ hermes-relay │────▶│ hermes-pipeline │
│  (blockchain)   │     │   (stream)   │     │  (transformer)  │
└─────────────────┘     └──────────────┘     └────────┬────────┘
                                                      │
                                                      ▼
                                             ┌─────────────────┐
                                             │      Kafka      │
                                             │  ┌───────────┐  │
                                             │  │ space.    │  │
                                             │  │ creations │  │
                                             │  ├───────────┤  │
                                             │  │ space.    │  │
                                             │  │membership │  │
                                             │  ├───────────┤  │
                                             │  │ space.    │  │
                                             │  │ trust.    │  │
                                             │  │extensions │  │
                                             │  ├───────────┤  │
                                             │  │ space.    │  │
                                             │  │moderation │  │
                                             │  ├───────────┤  │
                                             │  │ space.    │  │
                                             │  │  topics   │  │
                                             │  ├───────────┤  │
                                             │  │ space.    │  │
                                             │  │governance │  │
                                             │  ├───────────┤  │
                                             │  │ curation. │  │
                                             │  │  votes    │  │
                                             │  ├───────────┤  │
                                             │  │knowledge. │  │
                                             │  │  edits    │  │
                                             │  └───────────┘  │
                                             └─────────────────┘
```

## Pipeline Modules

The pipeline is organized into modules that handle specific action categories:

| Module | Actions | Output Topic |
|--------|---------|--------------|
| `spaces` | `SPACE_REGISTERED` | `space.creations` |
| `membership` | `EDITOR_ADDED/REMOVED`, `MEMBER_ADDED/REMOVED`, `SPACE_LEFT` | `space.membership` |
| `trust` | `SUBSPACE_VERIFIED/RELATED/TOPIC_DECLARED/REMOVED` | `space.trust.extensions` |
| `moderation` | `EDITOR_FLAGGED/UNFLAGGED`, `FLAGGED/UNFLAGGED` | `space.moderation` |
| `topics` | `TOPIC_DECLARED` | `space.topics` |
| `governance` | `PROPOSAL_CREATED/VOTED/EXECUTED` | `space.governance` |
| `voting` | `UPVOTED/DOWNVOTED/UNVOTED` | `curation.votes` |
| `edits` | `EDITS_PUBLISHED` | `knowledge.edits` |

## Processing Order

Events are emitted to Kafka in a specific order to maintain consistency:

1. **Spaces** - Must be emitted first since all other events reference spaces
2. **Membership** - Who can do what in spaces
3. **Trust** - Defines the space topology
4. **Moderation** - Flagging events
5. **Topics** - Topic declarations
6. **Governance** - Proposals reference spaces
7. **Voting** - Social layer
8. **Edits** - Last, as they may reference entities across trusted spaces

## Performance Optimization

The edits pipeline involves async IPFS fetching, which is the slowest operation. To optimize:

- The edits transform is kicked off **first** (before sync transforms)
- IPFS network I/O happens in parallel with the sync transforms
- The edits result is awaited at the end, just before emitting

## Why Client-Side Filtering?

The substreams protocol only supports consuming a single output module per stream in production mode. Since the pipeline needs multiple event types, we:

- Subscribe to `map_actions` (all raw actions)
- Filter client-side using action type constants from `hermes_relay::actions`

See `hermes-relay/docs/decisions/0001-multiple-substreams-modules-consumers.md` for more details.

## Future Work

- **Cursor persistence**: Add PostgreSQL/Redis storage for cursor to resume from last processed block
- **Metrics**: Add Prometheus metrics for monitoring
- **Data decoding**: Decode the `data` field for richer event content
