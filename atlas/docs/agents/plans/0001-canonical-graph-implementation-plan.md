# 0001: Canonical Graph Implementation Plan

## Summary

This document outlines the plan to implement push-based canonical graph computation for Atlas, integrated with the Hermes event streaming infrastructure.

**Prerequisite**: This plan depends on [0002: Transitive Graph Implementation](./0002-transitive-graph-implementation-plan.md), which provides the foundational `TransitiveProcessor` and `TransitiveCache` used for efficient canonical graph computation.

## Background

### Current State

Atlas is currently in documentation phase. The conceptual design exists in `atlas/docs/`:
- `graph-concepts.md` - Core graph concepts and algorithms
- `storage.md` - Storage structure design
- `topics-and-edits.md` - Event model for communities and topics

Hermes infrastructure is operational with:
- Kafka broker and producer
- Protocol buffer messages for space creation and trust extensions (`HermesCreateSpace`, `HermesSpaceTrustExtension`)
- Message serialization and emission working

### Problem

The current design is pull-based: canonical graphs are computed on-demand at query time. We need to make this push-based so that:
1. Canonical graph changes are emitted as messages to the event stream
2. Downstream systems can consume these messages to stay up-to-date
3. Distance from root is preserved in the emitted tree structure (important for downstream consumers)

## Understanding

### Core Concepts

**Spaces and Topics:**
- A Space is created via `HermesCreateSpace` and announces a topic (`topic_id`) at creation
- Spaces can point to other topics via `SubtopicExtension`, creating topic edges
- Topic resolution: find all spaces that announced a given topic

**Edge Types:**
- **Explicit Edges**: Direct space-to-space connections (`VerifiedExtension`, `RelatedExtension`)
- **Topic Edges**: Indirect connections through topic membership (`SubtopicExtension`)

**Graph Types (from original docs):**

1. **Global Graph**: Complete graph with all nodes and edges (source of truth)
2. **Local Graph**: Per-node, immediate children only (one-hop view)
3. **Transitive Graph**: Per-node, all reachable nodes as a DAG
4. **Canonical Graph**: Single global graph from root with trust boundary rules

**Canonical Graph:**
- Computed from a known root node
- Uses two-phase BFS algorithm:
  - Phase 1: Traverse explicit edges only to establish canonical (trusted) nodes
  - Phase 2: Add topic edges, but only connecting nodes already in the canonical set
- Trust flows only through explicit edges; topic edges cannot grant trust

**Transitive Graph:**
- Computed per-space
- Standard BFS traversal (explicit + topic edges)
- No trust boundary restrictions

**Key Design Decisions:**
- Single global canonical graph with a known root
- Per-space transitive graphs
- Emit full tree structure on each change (not deltas)
- Tree structure matters because downstream systems care about distance from root
- Persist graphs durably to PostgreSQL
- Only emit when graph actually changes (hash-based change detection)
- Separate processors for different graph types (Canonical, Transitive)

### Message Mapping

| Hermes Message | Graph Effect |
|----------------|--------------|
| `HermesCreateSpace { space_id, topic_id }` | Add node, associate space with topic |
| `VerifiedExtension { source, target }` | Add explicit edge (verified/trust) |
| `RelatedExtension { source, target }` | Add explicit edge (related) |
| `SubtopicExtension { source, target_topic_id }` | Add topic edge (source space → topic) |

### Topic Edge Resolution

When a space points to a topic via `SubtopicExtension`:
1. Look up all spaces that announced that topic (via `HermesCreateSpace`)
2. Filter to spaces already in the canonical set
3. Add those spaces and their transitive subtrees to the tree

## Implementation Plan

### 1. Protobuf Message Definition

Create `hermes-schema/proto/topology.proto`:

```protobuf
syntax = "proto3";
package topology;

message CanonicalGraphUpdated {
  bytes root_id = 1;
  CanonicalTreeNode tree = 2;
  repeated bytes canonical_space_ids = 3;
  uint64 sequence_number = 4;
  uint64 timestamp = 5;
}

message CanonicalTreeNode {
  bytes space_id = 1;
  EdgeType edge_type = 2;
  bytes topic_id = 3;  // Only set when edge_type is TOPIC
  repeated CanonicalTreeNode children = 4;
}

enum EdgeType {
  EDGE_TYPE_UNSPECIFIED = 0;
  EDGE_TYPE_VERIFIED = 1;
  EDGE_TYPE_RELATED = 2;
  EDGE_TYPE_TOPIC = 3;
}
```

### 2. Atlas Architecture

Atlas is the topology system. It maintains the global graph state and runs separate processors for different graph computations.

**Shared Graph State:**

The global graph state is shared across all processors:

```rust
struct GraphState {
    // Nodes
    spaces: HashMap<SpaceId, SpaceMetadata>,

    // Space → announced topic (1:1 from creation)
    space_topic: HashMap<SpaceId, TopicId>,

    // Topic → spaces that announced it (reverse index)
    topic_spaces: HashMap<TopicId, HashSet<SpaceId>>,

    // Explicit edges: source → target → edge type
    explicit_edges: HashMap<SpaceId, HashMap<SpaceId, EdgeType>>,

    // Topic edges: source → topic_id (from SubtopicExtension)
    topic_edges: HashMap<SpaceId, HashSet<TopicId>>,
}
```

**Processors:**

Each processor subscribes to graph state changes and computes its specific graph type:

1. **Canonical Processor**
   - Computes single global canonical graph from root
   - Two-phase BFS algorithm (explicit edges first, then topic edges)
   - Emits `CanonicalGraphUpdated` on changes

2. **Transitive Processor** (future)
   - Computes per-space transitive graphs
   - Standard BFS (explicit + topic edges)
   - Could be push-based (emit on change) or pull-based (compute on request)
   - Emits `TransitiveGraphUpdated` per affected space

**Processing Flow:**

```
Kafka (HermesCreateSpace, HermesSpaceTrustExtension)
    ↓
Atlas::process_event()
    ↓
Update GraphState
    ↓
Notify Processors
    ↓
┌─────────────────────────────────────────────┐
│  Canonical Processor    Transitive Processor │
│         ↓                      ↓             │
│  Recompute graph       Recompute affected    │
│  Hash & compare        graphs per space      │
│         ↓                      ↓             │
│  If changed: emit      If changed: emit      │
└─────────────────────────────────────────────┘
    ↓
Persist to PostgreSQL + Emit to Kafka
```

### 3. Canonical Graph Algorithm

The canonical graph algorithm leverages pre-computed transitive graphs from `TransitiveProcessor` (see [0002](./0002-transitive-graph-implementation-plan.md)).

**Phase 1 - Establish Canonical Set:**
1. Get root's explicit-only transitive graph from `TransitiveCache`
2. The canonical set is the flat set from this transitive graph
3. The initial tree structure comes from this transitive graph

```rust
let root_transitive = transitive_cache.get_explicit_only(root, state);
let canonical_set = root_transitive.flat.clone();
let mut tree = root_transitive.tree.clone();
```

**Phase 2 - Topic Edge Addition:**
1. Collect all topic edges from canonical nodes
2. For each topic edge (source → topic_id):
   - Resolve topic to spaces that announced it
   - Filter to spaces in canonical set
   - For each canonical member:
     - Get member's pre-computed full transitive graph from `TransitiveCache`
     - Filter subtree to only canonical descendants
     - Attach filtered subtree to source node
3. Result: complete tree with topic edges included

```rust
for (source, topic_id) in topic_edges_from_canonical {
    for member in resolve_topic(topic_id) {
        if canonical_set.contains(&member) {
            let member_transitive = transitive_cache.get_full(member, state);
            let filtered = filter_to_canonical(&member_transitive.tree, &canonical_set);
            attach_subtree(&mut tree, source, filtered, topic_id);
        }
    }
}
```

**Implementation Details:**
- Phase 1 is O(1) lookup instead of O(edges) BFS
- Phase 2 clones and filters pre-computed subtrees instead of re-traversing
- Deterministic child ordering (sort by SpaceId) for consistent hashing

### Event Filtering

Not every topology event affects the canonical graph. Before recomputing, check if the event can possibly change the canonical set or tree structure:

```rust
impl CanonicalProcessor {
    /// Check if an event can affect the canonical graph
    fn affects_canonical(&self, event: &SpaceTopologyEvent, canonical_set: &HashSet<SpaceId>) -> bool {
        match &event.payload {
            // New spaces are not canonical until reached via explicit edges from root
            SpaceTopologyPayload::SpaceCreated(_) => false,

            SpaceTopologyPayload::TrustExtended(extended) => {
                // Only events from canonical sources can affect the canonical graph
                canonical_set.contains(&extended.source_space_id)
            }
        }
    }
}
```

**Event Analysis:**

| Event | Affects Canonical? | Reason |
|-------|-------------------|--------|
| `SpaceCreated` | No | New spaces aren't canonical until reached via explicit edges |
| `VerifiedExtension` from canonical source | Yes | Target becomes canonical |
| `RelatedExtension` from canonical source | Yes | Target becomes canonical |
| `SubtopicExtension` from canonical source | Yes | Tree structure changes (adds topic edge connections) |
| Any extension from non-canonical source | No | Cannot affect canonical set or tree |

**Benefits:**
- Skip recomputation entirely for events outside the canonical graph
- O(1) check using the flat canonical set
- No hash comparison needed - if source isn't canonical, nothing changed

**Note:** This replaces the previous hash-based change detection approach. Since we can determine from the event alone whether the canonical graph is affected, we don't need to recompute and compare hashes.

### 4. PostgreSQL Schema

```sql
-- Current canonical graph (single row, updated on change)
CREATE TABLE canonical_graph (
    id INTEGER PRIMARY KEY DEFAULT 1,
    root_id BYTEA NOT NULL,
    tree JSONB NOT NULL,
    canonical_space_ids BYTEA[] NOT NULL,
    sequence_number BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT single_row CHECK (id = 1)
);

-- Topology state snapshot for fast startup
CREATE TABLE topology_state (
    id INTEGER PRIMARY KEY DEFAULT 1,
    state JSONB NOT NULL,
    last_cursor TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### 5. Kafka Topics

- **Input**: Existing topic(s) with `HermesCreateSpace` and `HermesSpaceTrustExtension` messages
- **Output**: New topic for `CanonicalGraphUpdated` messages

## File Structure

```
gaia/
├── hermes-schema/
│   └── proto/
│       └── topology.proto              # Graph update messages
└── atlas/
    ├── Cargo.toml
    ├── src/
    │   ├── main.rs                     # Kafka consumer/producer setup
    │   ├── lib.rs
    │   ├── state.rs                    # GraphState struct and event handling
    │   ├── processor/
    │   │   ├── mod.rs                  # Processor trait
    │   │   ├── canonical.rs            # Canonical graph processor (two-phase BFS)
    │   │   └── transitive.rs           # Transitive graph processor (future)
    │   ├── hash.rs                     # Tree hashing for change detection
    │   └── persistence.rs              # PostgreSQL read/write
    └── docs/
        ├── graph-concepts.md
        ├── storage.md
        ├── topics-and-edits.md
        └── agents/
            └── plans/
                └── 0001-canonical-graph-implementation-plan.md
```

## Future Optimizations

### Incremental Canonical Graph Updates

Instead of full recomputation on every event, track what changed and update incrementally.

**Data Structures:**

```rust
struct IncrementalState {
    // Current canonical set
    canonical_set: HashSet<SpaceId>,

    // Current tree structure
    tree: TreeNode,

    // Reverse index: which spaces does each topic edge affect?
    // topic_id → set of spaces that have SubtopicExtension pointing to this topic
    topic_edge_sources: HashMap<TopicId, HashSet<SpaceId>>,
}
```

**Incremental Update Rules:**

1. **New space created (`HermesCreateSpace`)**
   - Add to `topic_spaces[topic_id]`
   - If any canonical space has a topic edge to this topic:
     - Check if new space is canonical (reachable via explicit edges)
     - If yes, it was already added in Phase 1 when it became canonical
     - Topic edges only add connections, not new canonical nodes

2. **New explicit edge added (`VerifiedExtension`, `RelatedExtension`)**
   - If source is canonical and target is not yet canonical:
     - Add target (and its explicit-edge descendants) to canonical set
     - Re-evaluate topic edges: any topic edge pointing to topics announced by newly-canonical spaces may now resolve to more members
   - If both source and target already canonical:
     - Tree structure changes (new edge), but canonical set unchanged
     - May affect depth of target in tree

3. **New topic edge added (`SubtopicExtension`)**
   - Look up `topic_spaces[target_topic_id]`
   - Filter to canonical members
   - Add edges from source to canonical members (with their subtrees)
   - Does not expand canonical set

4. **Space removed / Edge removed**
   - More complex: need to check if removed edge was the only path to some nodes
   - May need full recomputation, or maintain reference counts

**When to Fall Back to Full Recomputation:**

- Removal events (edge or space deleted)
- When incremental update touches > 50% of canonical set
- Periodic full recomputation to correct any drift

**Trade-offs:**

| Approach | Pros | Cons |
|----------|------|------|
| Full recomputation | Simple, always correct | O(canonical_set) per event |
| Incremental | Fast for small changes | Complex, edge cases, removal is hard |

**Recommendation:**

Start with full recomputation + subtree reuse. Add incremental updates later if profiling shows recomputation is a bottleneck. The canonical set is likely small relative to event frequency, making full recomputation acceptable initially.

## Benchmarking

### Goals

Establish baseline performance metrics and identify bottlenecks before optimizing. Benchmarks should answer:

1. How long does full canonical graph computation take at various graph sizes?
2. How does topic edge resolution scale with topic membership size?
3. What is the end-to-end latency from event receipt to emission?

### Benchmark Scenarios

#### 1. Graph Computation Benchmarks

Test canonical graph computation with synthetic graphs of varying characteristics:

| Scenario | Nodes | Explicit Edges | Topic Edges | Topics | Members/Topic |
|----------|-------|----------------|-------------|--------|---------------|
| Small | 100 | 200 | 20 | 10 | 5 |
| Medium | 1,000 | 5,000 | 200 | 50 | 20 |
| Large | 10,000 | 50,000 | 2,000 | 200 | 50 |
| Wide Topics | 1,000 | 2,000 | 100 | 10 | 100 |
| Deep Tree | 1,000 | 999 | 100 | 50 | 20 |

**Metrics to capture:**
- Phase 1 duration (explicit edge BFS)
- Phase 2 duration (topic edge resolution)
- Total computation time
- Memory allocation
- Tree node count in output

#### 2. Event Filtering Benchmarks

Measure the `affects_canonical` check performance:

| Scenario | Description |
|----------|-------------|
| Canonical source | Event from a space in the canonical set |
| Non-canonical source | Event from a space outside the canonical set |

**Metrics to capture:**
- HashSet lookup time
- Events filtered per second

#### 3. Topic Resolution Benchmarks

Isolate topic edge resolution performance:

| Scenario | Canonical Set Size | Topic Members | Canonical Members in Topic |
|----------|-------------------|---------------|---------------------------|
| Sparse | 1,000 | 100 | 10 |
| Dense | 1,000 | 100 | 90 |
| Large Topic | 1,000 | 1,000 | 500 |

**Metrics to capture:**
- Lookup time (`topic_spaces[topic_id]`)
- Filter time (canonical set intersection)
- Subtree attachment time

#### 4. End-to-End Latency Benchmarks

Measure full processing pipeline:

```
Event received → affects_canonical? → GraphState updated → Canonical computed → Persisted → Emitted
```

**Metrics to capture:**
- Total latency (p50, p95, p99)
- Breakdown by stage
- Throughput (events/second)

### Benchmark Implementation

```rust
// atlas/benches/canonical.rs

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};

fn bench_canonical_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("canonical_computation");

    for (name, nodes, edges, topic_edges) in [
        ("small", 100, 200, 20),
        ("medium", 1_000, 5_000, 200),
        ("large", 10_000, 50_000, 2_000),
    ] {
        let state = generate_graph_state(nodes, edges, topic_edges);

        group.bench_with_input(
            BenchmarkId::new("full_computation", name),
            &state,
            |b, state| {
                b.iter(|| {
                    let processor = CanonicalProcessor::new(root_id);
                    processor.compute(state)
                });
            },
        );
    }

    group.finish();
}

fn bench_event_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_filtering");

    let (state, canonical_set) = generate_canonical_scenario(1_000);

    group.bench_function("affects_canonical_hit", |b| {
        let canonical_space = *canonical_set.iter().next().unwrap();
        let event = make_verified_event(canonical_space, some_target);
        b.iter(|| processor.affects_canonical(&event, &canonical_set));
    });

    group.bench_function("affects_canonical_miss", |b| {
        let non_canonical_space = make_space_id(9999);
        let event = make_verified_event(non_canonical_space, some_target);
        b.iter(|| processor.affects_canonical(&event, &canonical_set));
    });

    group.finish();
}

fn bench_topic_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("topic_resolution");

    for (name, canonical_size, topic_members, overlap) in [
        ("sparse", 1_000, 100, 10),
        ("dense", 1_000, 100, 90),
        ("large_topic", 1_000, 1_000, 500),
    ] {
        let (state, canonical_set, topic_id) =
            generate_topic_scenario(canonical_size, topic_members, overlap);

        group.bench_with_input(
            BenchmarkId::new("resolve_topic", name),
            &(state, canonical_set, topic_id),
            |b, (state, canonical_set, topic_id)| {
                b.iter(|| {
                    resolve_topic_members(state, canonical_set, *topic_id)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_canonical_computation,
    bench_change_detection,
    bench_topic_resolution,
);
criterion_main!(benches);
```

### File Structure Addition

```
atlas/
├── benches/
│   ├── canonical.rs          # Canonical graph computation benchmarks
│   ├── event_filtering.rs    # affects_canonical check benchmarks
│   └── helpers.rs            # Synthetic graph generation utilities
```

### Success Criteria

Initial implementation should meet these targets (to be validated/adjusted after first benchmarks):

| Metric | Target |
|--------|--------|
| Full computation (1K nodes) | < 10ms |
| Full computation (10K nodes) | < 100ms |
| Event filtering (`affects_canonical`) | < 100ns |
| End-to-end latency (p95) | < 50ms |
| Throughput | > 100 events/second |

### Benchmark Schedule

1. **Before optimization**: Run full benchmark suite to establish baseline
2. **After each optimization**: Re-run to measure improvement
3. **Before release**: Full regression suite to ensure no performance degradation

## Open Questions

1. **Root Configuration**: How is the root space ID provided? Environment variable? Database config?
2. **Startup Behavior**: On startup, should we rebuild state from Kafka or load snapshot from PostgreSQL?
3. **Removal Events**: Are there events for removing spaces or edges? Current messages only cover creation/extension.

## Next Steps

1. Define protobuf messages in `hermes-schema/proto/topology.proto`
2. Set up `atlas` crate with basic structure
3. Implement `GraphState` and event processing
4. **Implement `TransitiveProcessor` first** (see [0002](./0002-transitive-graph-implementation-plan.md))
5. Implement `CanonicalProcessor` using `TransitiveCache`
6. Add tree hashing and change detection
7. Add PostgreSQL persistence
8. Add Kafka consumer/producer integration
9. Testing and integration
