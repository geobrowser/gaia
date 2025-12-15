# 0004: Event Emission Implementation Plan

## Summary

This document outlines the plan to implement Phase 4 of the Atlas roadmap: emitting `CanonicalGraphUpdated` messages to Kafka when the canonical graph changes.

**Prerequisites**: 
- Phase 1 (Foundation): Core types, GraphState, mock substream
- Phase 2 (Transitive): TransitiveProcessor and cache
- Phase 3 (Canonical): CanonicalProcessor with change detection

## Current State

Atlas can:
- Apply space topology events to GraphState
- Compute transitive graphs (full and explicit-only)
- Compute canonical graphs with hash-based change detection
- Run a demo with deterministic topology

Atlas cannot yet:
- Serialize canonical graphs to protobuf
- Emit messages to Kafka
- Integrate with downstream consumers

## Goals

1. Define `CanonicalGraphUpdated` protobuf message in `hermes-schema`
2. Set up Kafka producer in Atlas
3. Serialize `CanonicalGraph` to protobuf
4. Emit to Kafka on canonical graph change

## Implementation Steps

### Step 4.1: Define Protobuf Message

Create `hermes-schema/proto/topology.proto` following existing patterns:

```protobuf
syntax = "proto3";

package topology;

import "blockchain_metadata.proto";

// Emitted when the canonical graph changes
message CanonicalGraphUpdated {
  // Root space this graph was computed from
  bytes root_id = 1;
  
  // Tree representation with edge metadata
  CanonicalTreeNode tree = 2;
  
  // Flat set of all canonical space IDs
  repeated bytes canonical_space_ids = 3;
  
  // Monotonically increasing sequence number
  uint64 sequence_number = 4;
  
  // Block metadata from the event that triggered this update
  blockchain_metadata.BlockchainMetadata meta = 5;
}

// A node in the canonical tree
message CanonicalTreeNode {
  // The space this node represents
  bytes space_id = 1;
  
  // How this node was reached from its parent
  EdgeType edge_type = 2;
  
  // If reached via topic edge, which topic (otherwise empty)
  bytes topic_id = 3;
  
  // Children of this node in the traversal
  repeated CanonicalTreeNode children = 4;
}

// The type of edge connecting a node to its parent
enum EdgeType {
  EDGE_TYPE_UNSPECIFIED = 0;
  EDGE_TYPE_ROOT = 1;
  EDGE_TYPE_VERIFIED = 2;
  EDGE_TYPE_RELATED = 3;
  EDGE_TYPE_TOPIC = 4;
}
```

**Tasks:**
1. Create `hermes-schema/proto/topology.proto`
2. Update `hermes-schema/build.rs` to compile the new proto (or generate manually)
3. Create `hermes-schema/src/pb/topology.rs` (generated)
4. Update `hermes-schema/src/pb/mod.rs` to export the new module

### Step 4.2: Set Up Kafka Producer in Atlas

Add Kafka producer capability to Atlas following the `hermes-producer` patterns.

**Dependencies to add to `atlas/Cargo.toml`:**
```toml
[dependencies]
rdkafka = { version = "0.36", features = ["cmake-build", "zstd"] }
prost = "0.13.5"
hermes-schema = { path = "../hermes-schema" }
```

**Producer Configuration:**
```rust
// atlas/src/kafka/producer.rs

use rdkafka::config::ClientConfig;
use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
use std::time::Duration;

pub struct AtlasProducer {
    producer: BaseProducer,
    topic: String,
}

impl AtlasProducer {
    pub fn new(broker: &str, topic: &str) -> Result<Self, rdkafka::error::KafkaError> {
        let producer = ClientConfig::new()
            .set("bootstrap.servers", broker)
            .set("client.id", "atlas-producer")
            .set("compression.type", "zstd")
            .set("message.timeout.ms", "5000")
            .set("queue.buffering.max.messages", "100000")
            .set("queue.buffering.max.kbytes", "1048576")
            .set("batch.num.messages", "10000")
            .create()?;

        Ok(Self {
            producer,
            topic: topic.to_string(),
        })
    }

    pub fn send(&self, key: &[u8], payload: &[u8]) -> Result<(), KafkaError> {
        let record = BaseRecord::to(&self.topic)
            .key(key)
            .payload(payload);

        self.producer.send(record).map_err(|(e, _)| e)?;
        Ok(())
    }

    pub fn flush(&self) -> Result<(), KafkaError> {
        self.producer.flush(Duration::from_secs(5))
    }
}
```

**Tasks:**
1. Add rdkafka and prost dependencies to `atlas/Cargo.toml`
2. Create `atlas/src/kafka/mod.rs` and `atlas/src/kafka/producer.rs`
3. Add environment variable configuration for `KAFKA_BROKER`

### Step 4.3: Serialize CanonicalGraph to Protobuf

Create conversion functions between Atlas types and protobuf types.

```rust
// atlas/src/kafka/serialize.rs

use crate::events::SpaceId;
use crate::graph::{CanonicalGraph, EdgeType, TreeNode};
use hermes_schema::pb::topology::{
    CanonicalGraphUpdated, CanonicalTreeNode, EdgeType as ProtoEdgeType,
};
use prost::Message;

impl From<EdgeType> for ProtoEdgeType {
    fn from(edge_type: EdgeType) -> Self {
        match edge_type {
            EdgeType::Root => ProtoEdgeType::Root,
            EdgeType::Verified => ProtoEdgeType::Verified,
            EdgeType::Related => ProtoEdgeType::Related,
            EdgeType::Topic => ProtoEdgeType::Topic,
        }
    }
}

fn tree_node_to_proto(node: &TreeNode) -> CanonicalTreeNode {
    CanonicalTreeNode {
        space_id: node.space_id.to_vec(),
        edge_type: ProtoEdgeType::from(node.edge_type) as i32,
        topic_id: node.topic_id.map(|t| t.to_vec()).unwrap_or_default(),
        children: node.children.iter().map(tree_node_to_proto).collect(),
    }
}

pub fn canonical_graph_to_proto(
    graph: &CanonicalGraph,
    sequence_number: u64,
    meta: BlockchainMetadata,
) -> CanonicalGraphUpdated {
    CanonicalGraphUpdated {
        root_id: graph.root.to_vec(),
        tree: Some(tree_node_to_proto(&graph.tree)),
        canonical_space_ids: graph.flat.iter().map(|id| id.to_vec()).collect(),
        sequence_number,
        meta: Some(meta.into()),
    }
}

pub fn encode_canonical_update(update: &CanonicalGraphUpdated) -> Vec<u8> {
    let mut buf = Vec::new();
    update.encode(&mut buf).expect("Failed to encode protobuf");
    buf
}
```

**Tasks:**
1. Create `atlas/src/kafka/serialize.rs`
2. Implement `From` traits for type conversions
3. Add serialization benchmarks

### Step 4.4: Emit to Kafka on Canonical Graph Change

Wire up the canonical processor to emit messages when changes are detected.

```rust
// atlas/src/kafka/emitter.rs

use crate::events::BlockMetadata;
use crate::graph::CanonicalGraph;
use crate::kafka::{producer::AtlasProducer, serialize::*};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct CanonicalGraphEmitter {
    producer: AtlasProducer,
    sequence: AtomicU64,
}

impl CanonicalGraphEmitter {
    pub fn new(producer: AtlasProducer) -> Self {
        Self {
            producer,
            sequence: AtomicU64::new(0),
        }
    }

    /// Emit a canonical graph update to Kafka
    pub fn emit(
        &self,
        graph: &CanonicalGraph,
        meta: &BlockMetadata,
    ) -> Result<(), EmissionError> {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
        
        let update = canonical_graph_to_proto(graph, sequence, meta.clone());
        let payload = encode_canonical_update(&update);
        
        // Key by root_id for partition locality
        self.producer.send(&graph.root, &payload)?;
        self.producer.flush()?;
        
        Ok(())
    }
}

#[derive(Debug)]
pub enum EmissionError {
    Kafka(rdkafka::error::KafkaError),
    Serialization(prost::EncodeError),
}
```

**Integration with main processing loop:**

```rust
// atlas/src/main.rs (conceptual)

fn process_event(
    event: &SpaceTopologyEvent,
    state: &mut GraphState,
    transitive: &mut TransitiveProcessor,
    canonical: &mut CanonicalProcessor,
    emitter: &CanonicalGraphEmitter,
) -> Result<(), ProcessingError> {
    // Update graph state
    state.apply_event(event);
    
    // Invalidate transitive cache
    transitive.handle_event(event, state);
    
    // Check if this event could affect canonical graph
    // (optimization: skip recomputation for non-canonical sources)
    if !canonical.affects_canonical(event, &get_canonical_set(canonical)) {
        return Ok(());
    }
    
    // Compute canonical graph (returns None if unchanged)
    if let Some(graph) = canonical.compute(state, transitive) {
        // Emit to Kafka
        emitter.emit(&graph, &event.meta)?;
    }
    
    Ok(())
}
```

**Tasks:**
1. Create `atlas/src/kafka/emitter.rs`
2. Implement sequence number tracking
3. Wire up emission in the main event processing loop
4. Add error handling and logging

## File Structure

```
atlas/
├── Cargo.toml                    # Add rdkafka, prost, hermes-schema deps
├── src/
│   ├── main.rs                   # Wire up emitter
│   ├── lib.rs                    # Export kafka module
│   ├── events.rs
│   ├── graph/
│   │   ├── mod.rs
│   │   ├── canonical.rs
│   │   ├── transitive.rs
│   │   ├── state.rs
│   │   ├── memory.rs
│   │   ├── tree.rs
│   │   └── hash.rs
│   └── kafka/                    # NEW
│       ├── mod.rs
│       ├── producer.rs
│       ├── serialize.rs
│       └── emitter.rs
└── benches/
    ├── canonical.rs
    ├── transitive.rs
    └── emission.rs               # NEW

hermes-schema/
├── proto/
│   ├── blockchain_metadata.proto
│   ├── knowledge.proto
│   ├── space.proto
│   └── topology.proto            # NEW
├── src/
│   ├── lib.rs
│   └── pb/
│       ├── mod.rs                # Update to export topology
│       ├── blockchain_metadata.rs
│       ├── knowledge.rs
│       ├── space.rs
│       └── topology.rs           # NEW (generated)
└── build.rs                      # Update to compile topology.proto
```

## Benchmarks

### Phase 4 Benchmarks

| Benchmark | Description | Target |
|-----------|-------------|--------|
| Protobuf serialization (100 nodes) | Serialize small canonical graph | < 100us |
| Protobuf serialization (1K nodes) | Serialize medium canonical graph | < 1ms |
| Protobuf serialization (10K nodes) | Serialize large canonical graph | < 10ms |
| Message size (100 nodes) | Serialized protobuf size | < 10KB |
| Message size (1K nodes) | Serialized protobuf size | < 100KB |
| Message size (10K nodes) | Serialized protobuf size | < 1MB |
| Kafka send latency | Time to send and flush | < 10ms |
| End-to-end latency | Event received to message emitted | < 50ms (p95) |

### Benchmark Implementation

```rust
// atlas/benches/emission.rs

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

fn bench_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("protobuf_serialization");
    
    for (name, nodes) in [("small", 100), ("medium", 1_000), ("large", 10_000)] {
        let graph = generate_canonical_graph(nodes);
        let meta = make_block_meta();
        
        group.bench_with_input(
            BenchmarkId::new("serialize", name),
            &(&graph, &meta),
            |b, (graph, meta)| {
                b.iter(|| {
                    let update = canonical_graph_to_proto(graph, 0, (*meta).clone());
                    encode_canonical_update(&update)
                });
            },
        );
    }
    
    group.finish();
}

fn bench_message_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_size");
    
    for (name, nodes) in [("small", 100), ("medium", 1_000), ("large", 10_000)] {
        let graph = generate_canonical_graph(nodes);
        let meta = make_block_meta();
        let update = canonical_graph_to_proto(&graph, 0, meta);
        let payload = encode_canonical_update(&update);
        
        println!("{}: {} bytes", name, payload.len());
    }
    
    group.finish();
}

criterion_group!(benches, bench_serialization, bench_message_size);
criterion_main!(benches);
```

## Testing Strategy

### Unit Tests

1. **Serialization tests** (`kafka/serialize.rs`)
   - Edge type conversion
   - Tree node serialization (preserves structure)
   - Empty tree edge cases
   - Round-trip encoding/decoding

2. **Producer tests** (`kafka/producer.rs`)
   - Configuration validation
   - Error handling for connection failures

3. **Emitter tests** (`kafka/emitter.rs`)
   - Sequence number increments
   - Error propagation

### Integration Tests

1. **Kafka integration** (requires running Kafka)
   - Send message and verify receipt
   - Message ordering by partition key
   - Compression verification

2. **End-to-end flow**
   - Event → GraphState → Canonical → Emit → Consume
   - Verify emitted message matches computed graph

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `KAFKA_BROKER` | `localhost:9092` | Kafka bootstrap server |
| `KAFKA_TOPIC_CANONICAL` | `topology.canonical` | Output topic for canonical updates |
| `ATLAS_ROOT_SPACE_ID` | (required) | Root space ID for canonical graph |

### Kafka Topic Configuration

Create the output topic with appropriate settings:

```bash
kafka-topics.sh --create \
  --topic topology.canonical \
  --partitions 1 \
  --replication-factor 1 \
  --config retention.ms=604800000 \
  --config compression.type=zstd
```

Single partition is sufficient since there's only one canonical graph (keyed by root_id).

## Implementation Order

1. **4.1 Protobuf definition** (no dependencies)
   - Create `topology.proto`
   - Generate Rust bindings
   - Update hermes-schema exports

2. **4.2 Kafka producer** (depends on 4.1)
   - Add dependencies
   - Create producer module
   - Test connection

3. **4.3 Serialization** (depends on 4.1)
   - Implement type conversions
   - Add serialization benchmarks
   - Write unit tests

4. **4.4 Emitter integration** (depends on 4.2, 4.3)
   - Create emitter
   - Wire into main loop
   - End-to-end testing

## Success Criteria

| Milestone | Criteria |
|-----------|----------|
| 4.1 complete | Protobuf compiles, Rust types generated |
| 4.2 complete | Atlas can connect to Kafka and send test messages |
| 4.3 complete | CanonicalGraph serializes correctly, benchmarks pass |
| 4.4 complete | Canonical graph changes emit to Kafka, e2e test passes |

## Open Questions

1. **Message Versioning**: Should we include a schema version in the message for forward compatibility?
2. **Backpressure**: How should Atlas handle Kafka producer buffer full scenarios?
3. **Monitoring**: What metrics should we expose (messages sent, latency, errors)?

## Related Documents

- [0000: Atlas Implementation Roadmap](./0000-atlas-implementation-roadmap.md)
- [0001: Canonical Graph Implementation Plan](./0001-canonical-graph-implementation-plan.md)
- [0002: Transitive Graph Implementation Plan](./0002-transitive-graph-implementation-plan.md)
