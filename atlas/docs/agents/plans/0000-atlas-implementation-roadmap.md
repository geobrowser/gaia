# 0000: Atlas Implementation Roadmap

## Overview

Atlas is the topology processor for the Gaia system. It consumes space topology events from Kafka, maintains graph state, computes transitive and canonical graphs, and emits graph updates back to Kafka.

This document provides a high-level implementation roadmap, connecting the individual component plans.

## Components

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              Atlas                                       │
│                                                                          │
│  ┌──────────────┐    ┌─────────────┐    ┌─────────────────────────────┐ │
│  │    Kafka     │    │             │    │        Processors           │ │
│  │   Consumer   │───▶│ GraphState  │───▶│  ┌───────────┐ ┌─────────┐  │ │
│  │              │    │             │    │  │Transitive │▶│Canonical│  │ │
│  └──────────────┘    └─────────────┘    │  └───────────┘ └─────────┘  │ │
│                             │           └─────────────────────────────┘ │
│                             ▼                         │                  │
│                      ┌─────────────┐                  ▼                  │
│                      │ PostgreSQL  │           ┌──────────────┐          │
│                      │ Persistence │           │    Kafka     │          │
│                      └─────────────┘           │   Producer   │          │
│                                                └──────────────┘          │
└─────────────────────────────────────────────────────────────────────────┘
```

## Implementation Phases

### Phase 1: Foundation

**Goal**: Set up the crate structure, core types, and Kafka consumer.

| Step | Description | Dependency |
|------|-------------|------------|
| 1.1 | Create `atlas` crate with workspace integration | - |
| 1.2 | Define core types (`SpaceId`, `TopicId`, `EdgeType`, `TreeNode`) | 1.1 |
| 1.3 | Implement `GraphState` struct and event application | 1.2 |
| 1.4 | Set up Kafka consumer for `HermesCreateSpace` and `HermesSpaceTrustExtension` | 1.1 |
| 1.5 | Wire consumer to `GraphState` updates | 1.3, 1.4 |

**Deliverable**: Atlas consumes events and builds in-memory graph state.

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
| 2.6 | Wire cache invalidation to `GraphState` events | 1.5, 2.5 |

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

**Deliverable**: `CanonicalProcessor` computes canonical graph and detects changes.

### Phase 4: Event Emission

**Goal**: Emit `CanonicalGraphUpdated` messages to Kafka when graph changes.

| Step | Description | Dependency |
|------|-------------|------------|
| 4.1 | Define `CanonicalGraphUpdated` protobuf message in `hermes-schema` | - |
| 4.2 | Set up Kafka producer in Atlas | 1.1 |
| 4.3 | Serialize `CanonicalGraph` to protobuf | 3.1, 4.1 |
| 4.4 | Emit to Kafka on canonical graph change | 3.5, 4.2, 4.3 |

**Deliverable**: Downstream consumers receive canonical graph updates via Kafka.

### Phase 5: Persistence

**Goal**: Persist graph state and canonical graph for durability and fast restart.

| Step | Description | Dependency |
|------|-------------|------------|
| 5.1 | Define PostgreSQL schema for `canonical_graph` table | - |
| 5.2 | Define PostgreSQL schema for `topology_state` table | - |
| 5.3 | Implement canonical graph persistence on change | 3.5, 5.1 |
| 5.4 | Implement topology state snapshot persistence | 1.5, 5.2 |
| 5.5 | Implement startup recovery from PostgreSQL snapshot | 5.4 |
| 5.6 | Implement Kafka offset tracking for resumption | 1.4, 5.4 |

**Deliverable**: Atlas survives restarts without reprocessing all events.

### Phase 6: Testing and Benchmarking

**Goal**: Validate correctness and performance.

| Step | Description | Dependency |
|------|-------------|------------|
| 6.1 | Unit tests for `GraphState` event application | 1.3 |
| 6.2 | Unit tests for transitive graph computation | 2.2, 2.3 |
| 6.3 | Unit tests for canonical graph computation | 3.3 |
| 6.4 | Integration tests with Kafka | 4.4 |
| 6.5 | Benchmarks for transitive computation | 2.4 |
| 6.6 | Benchmarks for canonical computation | 3.5 |
| 6.7 | End-to-end latency benchmarks | 4.4, 5.3 |

**Deliverable**: Confidence in correctness and performance characteristics.

## Dependency Graph

```
Phase 1: Foundation
    1.1 ─┬─▶ 1.2 ───▶ 1.3 ───┐
         │                    │
         └─▶ 1.4 ────────────┴──▶ 1.5
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
5. **1.4 → 1.5 → 2.5 → 2.6 → 3.5**: Wire up event consumption and invalidation
6. **5.1 → 5.2 → 5.3 → 5.4 → 5.5 → 5.6**: Add persistence
7. **6.x**: Testing and benchmarking throughout

## Success Criteria

| Milestone | Criteria |
|-----------|----------|
| Phase 1 complete | Events consumed, graph state updated in memory |
| Phase 2 complete | Transitive graphs computed correctly for any space |
| Phase 3 complete | Canonical graph computed correctly with change detection |
| Phase 4 complete | `CanonicalGraphUpdated` messages emitted to Kafka |
| Phase 5 complete | Atlas restarts from snapshot, resumes from Kafka offset |
| Phase 6 complete | Tests pass, benchmarks meet targets |

## Related Documents

- [Algorithm Overview](../algorithm-overview.md)
- [0001: Canonical Graph Implementation Plan](./0001-canonical-graph-implementation-plan.md)
- [0002: Transitive Graph Implementation Plan](./0002-transitive-graph-implementation-plan.md)
- [Graph Concepts](../graph-concepts.md)
- [Storage Design](../storage.md)
