# Hermes Pipeline

A transformer binary that consumes space-related events from Amp and publishes them to Kafka topics.

## Overview

This transformer is part of the Hermes architecture (see `docs/hermes-architecture.md`). It:

1. Connects to the Amp Flight SQL stream
2. Subscribes to actions emitted by the Space Registry contract
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

### Data Source Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `USE_MOCK` | Set to "true" or "1" to use mock data | `false` |
| `AMP_FLIGHT_URL` | Amp Flight SQL URL | `http://localhost:1602` |
| `AMP_DATASET` | Amp dataset | `geo/actions` |
| `AMP_START_BLOCK` | Block number to start from | `82655` |
| `AMP_END_BLOCK` | Block number to stop at | - |
| `AMP_ACTIONS_ADDRESS` | Actions contract address | `SPACE_REGISTRY_ADDRESS_HEX` |
| `AMP_RECONNECT_DELAY_SECS` | Reconnect delay in seconds | `2` |

### Kafka Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `KAFKA_BROKER` | Kafka broker address | `localhost:9092` |
| `KAFKA_USERNAME` | SASL username for managed Kafka | - |
| `KAFKA_PASSWORD` | SASL password for managed Kafka | - |
| `KAFKA_SSL_CA_PEM` | Custom CA cert for SSL (PEM format) | - |

### Telemetry Environment Variables

If `SENTRY_DSN` is set, telemetry is exported to Sentry. Otherwise, logs are written to the console.

| Variable | Description | Default |
|----------|-------------|---------|
| `SENTRY_DSN` | Sentry DSN / ingest URL | - |
| `SENTRY_TRACES_SAMPLE_RATE` | Sampling rate (0.0 - 1.0) | `1.0` |
| `SENTRY_SEND_DEFAULT_PII` | Include PII (IP, headers) | `false` |
| `SENTRY_ENVIRONMENT` | Environment tag | - |
| `SENTRY_RELEASE` | Release name | - |
| `SENTRY_DEBUG` | Also emit spans to stdout | `false` |

## Usage

### Local Development (Live Data)

```bash
# Start local Kafka (see hermes/docker-compose.yaml)
docker-compose -f hermes/docker-compose.yaml up -d

# Run with live Amp data (default)
AMP_FLIGHT_URL=http://localhost:1602 \
AMP_START_BLOCK=81809 \
KAFKA_BROKER=localhost:9092 \
cargo run --package hermes-pipeline
```

### Local Development (Mock Data)

```bash
# Run with mock data for testing
USE_MOCK=true \
KAFKA_BROKER=localhost:9092 \
cargo run --package hermes-pipeline
```

### Docker

```bash
# Build
docker build -f hermes-pipeline/Dockerfile -t hermes-pipeline .

# Run with live data
docker run \
  -e AMP_FLIGHT_URL=http://localhost:1602 \
  -e KAFKA_BROKER=localhost:9092 \
  hermes-pipeline
```

## Architecture

```
┌─────────────────┐     ┌─────────────────┐
│       Amp       │────▶│ hermes-pipeline │
│  (actions log)  │     │  (transformer)  │
└─────────────────┘     └────────┬────────┘
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

Amp streams raw action logs. Since the pipeline needs multiple event types, we:

- Subscribe to the actions log
- Filter client-side using action type constants from `hermes_codec::actions`

## Benchmarks

The pipeline includes benchmarks for measuring decode and transformation performance.

### Running Benchmarks

```bash
# Run all benchmarks
cargo bench -p hermes-pipeline

# Run only decode benchmarks
cargo bench -p hermes-pipeline --bench decode_bench

# Run only pipeline benchmarks
cargo bench -p hermes-pipeline --bench pipeline_bench

# Run specific benchmark group
cargo bench -p hermes-pipeline -- 'decode_proposal_created'
```

### Decode Performance

| Function | Time | Notes |
|----------|------|-------|
| Selector matching | ~2 ns | 4-byte compare |
| `decode_address_arg` | ~18 ns | Slice extraction |
| `decode_proposal_voted` | ~32 ns | Fixed tuple decode |
| `decode_vote_data` | ~37 ns | Fixed tuple decode |
| `decode_topic_declared` | ~18 ns | Fixed bytes16 decode |
| `decode_flag_data` | ~68 ns | String decode + UTF-8 validation |
| `decode_proposal_created` (1 action) | ~204 ns | Dynamic array decode |
| `decode_proposal_created` (50 actions) | ~5.7 µs | ~113 ns/action linear scaling |

### Pipeline Transform Performance

| Pipeline | Single Event | 10 Events | Per-event cost |
|----------|--------------|-----------|----------------|
| `membership` | ~110 ns | ~1.8 µs | ~90 ns/event |
| `trust` | ~101 ns | ~848 ns | ~85 ns/event |
| `spaces` | ~125 ns | ~1.1 µs | ~111 ns/event |
| `voting` | ~195 ns | ~1.5 µs | ~150 ns/event |
| `governance` (votes) | - | ~1.5 µs | ~145 ns/vote |
| `governance` (proposal) | ~440 ns | - | Includes ABI decode |

### Key Findings

1. **ABI decoding is the bottleneck** - The `alloy` ABI decoder dominates cost for governance/voting pipelines. Simple byte extraction (membership, trust, spaces) is ~2x faster per event.

2. **Linear scaling** - All pipelines scale linearly with action count. No O(n²) behavior.

3. **Action filtering is negligible** - ~1.4 ns per match. Not a concern.

4. **Throughput** - At ~130 ns/action average, pipelines can process **~7.7M actions/second** single-threaded.

5. **Real-world performance**:
   - Mixed block (11 actions): ~2.1 µs
   - Large block (150 actions): ~19 µs

The pipeline transform is not a bottleneck. Network I/O (Amp, Kafka, IPFS) dominates real-world latency.

## Future Work

- **Cursor persistence**: Add PostgreSQL/Redis storage for cursor to resume from last processed block
- **Metrics**: Add Prometheus metrics for monitoring
