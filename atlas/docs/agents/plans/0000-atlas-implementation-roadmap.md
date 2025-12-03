# 0000: Atlas Implementation Roadmap

## Overview

Atlas is the topology processor for the Gaia system. It consumes space topology events from blockchain substreams (via gRPC), maintains graph state, computes transitive and canonical graphs, and emits graph updates to Kafka (Hermes).

This document provides a high-level implementation roadmap, connecting the individual component plans.

## Components

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              Atlas                                       │
│                                                                          │
│  ┌──────────────┐    ┌─────────────┐    ┌─────────────────────────────┐ │
│  │  Substreams  │    │             │    │        Processors           │ │
│  │   Consumer   │───▶│ GraphState  │───▶│  ┌───────────┐ ┌─────────┐  │ │
│  │   (gRPC)     │    │             │    │  │Transitive │▶│Canonical│  │ │
│  └──────────────┘    └─────────────┘    │  └───────────┘ └─────────┘  │ │
│                             │           └─────────────────────────────┘ │
│                             ▼                         │                  │
│                      ┌─────────────┐                  ▼                  │
│                      │ PostgreSQL  │           ┌──────────────┐          │
│                      │ Persistence │           │    Kafka     │          │
│                      └─────────────┘           │   (Hermes)   │          │
└─────────────────────────────────────────────────────────────────────────┘
```

## Event Flow

```
Blockchain ──▶ Substreams RPC ──▶ Atlas (gRPC client) ──▶ Kafka (Hermes)
                                        │
                                        ├──▶ GraphState
                                        ├──▶ TransitiveProcessor
                                        ├──▶ CanonicalProcessor
                                        └──▶ PostgreSQL
```

Atlas follows the existing indexer pattern in the codebase (see `actions-indexer-pipeline`):
- Connects to Substreams RPC endpoint via gRPC
- Streams `BlockScopedData` containing space topology events
- Persists cursor after processing each block for resumability
- Handles `BlockUndoSignal` for blockchain reorganizations

## Implementation Phases

### Phase 1: Foundation

**Goal**: Set up the crate structure, core types, and substreams consumer.

| Step | Description | Dependency |
|------|-------------|------------|
| 1.1 | Create `atlas` crate with workspace integration | - |
| 1.2 | Define core types (`SpaceId`, `TopicId`, `EdgeType`, `TreeNode`) | 1.1 |
| 1.3 | Implement `GraphState` struct and event application | 1.2 |
| 1.4 | Set up substreams consumer (gRPC client using `stream` crate) | 1.1 |
| 1.5 | Define/reuse substream for space topology events | 1.4 |
| 1.6 | Wire consumer to `GraphState` updates | 1.3, 1.5 |

**Benchmarks**:
- `GraphState` event application latency (single event)
- `GraphState` batch event application (100, 1K, 10K events)
- Memory usage per node/edge in `GraphState`

**Deliverable**: Atlas consumes blockchain events via substreams and builds in-memory graph state.

### Phase 2: Transitive Graph Processor

**Goal**: Implement per-space transitive graph computation with caching.

See [0002: Transitive Graph Implementation Plan](./0002-transitive-graph-implementation-plan.md)

| Step | Description | Dependency |
|------|-------------|------------|
| 2.1 | Implement `TransitiveGraph` struct (tree + flat set + hash) | 1.2 |
| 2.2 | Implement BFS algorithm for full transitive graph | 2.1 |
| 2.3 | Implement BFS algorithm for explicit-only transitive graph | 2.1 |
| 2.4 | Implement `TransitiveCache` with lazy computation | 2.2, 2.3 |
| 2.5 | Implement reverse dependency index for invalidation | 2.4 |
| 2.6 | Wire cache invalidation to `GraphState` events | 1.6, 2.5 |

**Benchmarks**:
- Single transitive graph computation (100, 1K, 10K nodes)
- Full vs explicit-only transitive comparison
- Topic edge resolution latency (sparse vs dense topics)
- Cache hit/miss latency comparison
- Invalidation cascade size and latency
- Memory usage per cached transitive graph

**Deliverable**: `TransitiveProcessor` computes and caches per-space transitive graphs.

### Phase 3: Canonical Graph Processor

**Goal**: Implement canonical graph computation using transitive cache.

See [0001: Canonical Graph Implementation Plan](./0001-canonical-graph-implementation-plan.md)

| Step | Description | Dependency |
|------|-------------|------------|
| 3.1 | Implement `CanonicalGraph` struct | 1.2 |
| 3.2 | Implement Phase 1: get canonical set from root's explicit-only transitive | 2.4 |
| 3.3 | Implement Phase 2: add topic edges with filtered subtrees | 2.4, 3.2 |
| 3.4 | Implement tree hashing for change detection | 3.3 |
| 3.5 | Wire canonical computation to run after transitive invalidation | 2.6, 3.4 |

**Benchmarks**:
- Phase 1 latency (canonical set from transitive cache)
- Phase 2 latency (topic edge resolution and subtree filtering)
- Full canonical computation (100, 1K, 10K canonical nodes)
- Subtree filtering latency (by subtree size)
- Tree hashing latency (by tree size)
- Change detection overhead (hash comparison)

**Deliverable**: `CanonicalProcessor` computes canonical graph and detects changes.

### Phase 4: Event Emission

**Goal**: Emit `CanonicalGraphUpdated` messages to Kafka when graph changes.

| Step | Description | Dependency |
|------|-------------|------------|
| 4.1 | Define `CanonicalGraphUpdated` protobuf message in `hermes-schema` | - |
| 4.2 | Set up Kafka producer in Atlas | 1.1 |
| 4.3 | Serialize `CanonicalGraph` to protobuf | 3.1, 4.1 |
| 4.4 | Emit to Kafka on canonical graph change | 3.5, 4.2, 4.3 |

**Benchmarks**:
- Protobuf serialization latency (by tree size)
- Serialized message size (by tree size)
- Kafka producer latency (message send time)
- End-to-end latency: event received → message emitted

**Deliverable**: Downstream consumers receive canonical graph updates via Kafka.

### Phase 5: Persistence

**Goal**: Persist graph state and canonical graph for durability and fast restart.

| Step | Description | Dependency |
|------|-------------|------------|
| 5.1 | Define PostgreSQL schema for `canonical_graph` table | - |
| 5.2 | Define PostgreSQL schema for `topology_state` table | - |
| 5.3 | Implement canonical graph persistence on change | 3.5, 5.1 |
| 5.4 | Implement topology state snapshot persistence | 1.6, 5.2 |
| 5.5 | Implement startup recovery from PostgreSQL snapshot | 5.4 |
| 5.6 | Implement cursor tracking for substream resumption | 1.4, 5.4 |

**Benchmarks**:
- Canonical graph persistence latency (by tree size)
- Topology state snapshot persistence latency
- Startup recovery time from snapshot (by state size)
- Cursor persistence overhead per block

**Deliverable**: Atlas survives restarts without reprocessing all events.

### Phase 6: Testing and Integration

**Goal**: Validate correctness and end-to-end behavior.

| Step | Description | Dependency |
|------|-------------|------------|
| 6.1 | Unit tests for `GraphState` event application | 1.3 |
| 6.2 | Unit tests for transitive graph computation | 2.2, 2.3 |
| 6.3 | Unit tests for canonical graph computation | 3.3 |
| 6.4 | Integration tests (substreams + Kafka) | 4.4 |
| 6.5 | End-to-end latency profiling | 4.4, 5.3 |

**Deliverable**: Confidence in correctness and end-to-end behavior.

## Benchmarking Strategy

Benchmarks are integrated into each phase rather than deferred to the end. This enables:
- Early detection of performance bottlenecks
- Informed optimization decisions
- Granular understanding of where time is spent

### Benchmark Infrastructure

```
atlas/
├── benches/
│   ├── graph_state.rs      # Phase 1 benchmarks
│   ├── transitive.rs       # Phase 2 benchmarks
│   ├── canonical.rs        # Phase 3 benchmarks
│   ├── emission.rs         # Phase 4 benchmarks
│   ├── persistence.rs      # Phase 5 benchmarks
│   └── helpers.rs          # Synthetic graph generation
```

### Benchmark Summary by Phase

| Phase | Key Metrics |
|-------|-------------|
| 1. Foundation | Event application latency, memory per node/edge |
| 2. Transitive | BFS latency, cache hit/miss, invalidation cascade |
| 3. Canonical | Phase 1/2 latency, subtree filtering, hashing overhead |
| 4. Emission | Serialization latency, message size, Kafka send time |
| 5. Persistence | Write latency, snapshot size, recovery time |

### Continuous Benchmarking

- Run benchmarks after each phase completion
- Compare against previous runs to detect regressions
- Profile hotspots using `perf` or `flamegraph` when bottlenecks appear

## Dependency Graph

```
Phase 1: Foundation
    1.1 ─┬─▶ 1.2 ───▶ 1.3 ───┐
         │                    │
         └─▶ 1.4 ──▶ 1.5 ────┴──▶ 1.6
                                   │
Phase 2: Transitive                ▼
    1.2 ──▶ 2.1 ─┬─▶ 2.2 ─┬─▶ 2.4 ──▶ 2.5 ──▶ 2.6
                 │        │                    │
                 └─▶ 2.3 ─┘                    │
                                               │
Phase 3: Canonical                             ▼
    1.2 ──▶ 3.1          2.4 ──▶ 3.2 ──▶ 3.3 ──▶ 3.4 ──▶ 3.5
                                                          │
Phase 4: Emission                                         │
    4.1 ──┬──────────────────────────────────────────────┐│
          │                                               ▼▼
    1.1 ──▶ 4.2 ──────────────────────────────────▶ 4.3 ──▶ 4.4
                                                            │
Phase 5: Persistence                                        │
    5.1 ────────────────────────────────────────────────▶ 5.3
    5.2 ──▶ 5.4 ──┬─▶ 5.5                                   │
                  │                                         │
    1.4 ──────────┴─▶ 5.6                                   │
                                                            │
Phase 6: Testing                                            ▼
    1.3 ──▶ 6.1                                         (all)
    2.2 ──▶ 6.2                                           │
    3.3 ──▶ 6.3                                           ▼
    4.4 ──▶ 6.4 ──────────────────────────────────────▶ 6.7
    2.4 ──▶ 6.5
    3.5 ──▶ 6.6
```

## Suggested Implementation Order

For fastest path to a working system:

1. **1.1 → 1.2 → 1.3**: Core types and graph state
2. **2.1 → 2.2 → 2.3 → 2.4**: Transitive computation (no caching yet)
3. **3.1 → 3.2 → 3.3 → 3.4**: Canonical computation
4. **4.1 → 4.2 → 4.3 → 4.4**: Kafka emission
5. **1.4 → 1.5 → 1.6 → 2.5 → 2.6 → 3.5**: Wire up substreams consumption and invalidation
6. **5.1 → 5.2 → 5.3 → 5.4 → 5.5 → 5.6**: Add persistence
7. **6.x**: Testing and benchmarking throughout

## Success Criteria

| Milestone | Criteria |
|-----------|----------|
| Phase 1 complete | Substreams events consumed, graph state updated in memory |
| Phase 2 complete | Transitive graphs computed correctly for any space |
| Phase 3 complete | Canonical graph computed correctly with change detection |
| Phase 4 complete | `CanonicalGraphUpdated` messages emitted to Kafka |
| Phase 5 complete | Atlas restarts from snapshot, resumes from cursor |
| Phase 6 complete | Tests pass, benchmarks meet targets |

## Related Documents

- [Algorithm Overview](../algorithm-overview.md)
- [0001: Canonical Graph Implementation Plan](./0001-canonical-graph-implementation-plan.md)
- [0002: Transitive Graph Implementation Plan](./0002-transitive-graph-implementation-plan.md)
- [Graph Concepts](../graph-concepts.md)
- [Storage Design](../storage.md)
