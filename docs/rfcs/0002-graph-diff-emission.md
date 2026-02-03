# Graph Diff Emission (RFC)

*Date: 2026-01-21*

## Summary

Instead of emitting full graph snapshots every time something changes, we want to emit incremental diffs. This RFC defines the diff message format, how we batch and order changes, and the algorithm for computing diffs—including how we handle nodes that move within the graph.

## Goals

- Emit compact diffs for canonical and transitive graphs via Kafka
- Let consumers replay diffs to reconstruct the canonical tree
- Produce deterministic, stable output for testing and downstream processing
- Keep it fast for 100k+ node graphs with small change rates

## Non-Goals

- Changing how canonical and transitive graphs are computed
- Emitting multiple edge paths to the same node (diffs stay tree-like and lossy)
- Tracking changes incrementally during traversal (we compute full diffs after the fact)

## Current State

Today, Atlas emits full canonical trees whenever it detects a change. It computes the canonical graph via BFS with topic-edge attachments, and uses tree hashes to detect changes. Earlier exploration lives in:
- `atlas/docs/graph-diff-emission.md`
- `atlas/docs/graph-diff-algorithm-exploration.md`

This RFC formalizes the format and algorithm we'll use going forward.

## Diff Message Schema

We introduce two diff message types—one for canonical graphs, one for transitive:

```
message CanonicalGraphDiff {
  uint32 schema_version = 1;
  bytes root_id = 2;
  repeated NodeChange changes = 3;
  blockchain_metadata.BlockchainMetadata meta = 4;
}

message TransitiveGraphDiff {
  uint32 schema_version = 1;
  bytes root_id = 2;
  repeated NodeChange changes = 3;
  blockchain_metadata.BlockchainMetadata meta = 4;
}

message NodeChange {
  bytes space_id = 1;
  ChangeType type = 2;
  optional uint32 distance = 3;     // required for ADDED / MOVED
  optional EdgeInfo parent_edge = 4; // required for ADDED / MOVED
}

message EdgeInfo {
  bytes source = 1;
  oneof edge_type {
    VerifiedEdge verified = 2;
    RelatedEdge related = 3;
    TopicEdge topic = 4;   // includes topic_id
    EditorEdge editor = 5;
    MemberEdge member = 6;
  }
}

message TopicEdge {
  bytes topic_id = 1;
}

enum ChangeType {
  ADDED = 0;
  REMOVED = 1;
  MOVED = 2;
}
```

## Batching and Ordering

Each diff event represents a complete batch of changes for a single graph update. Consumers should apply the whole batch atomically. Within a batch, changes are sorted by `space_id` for determinism.

## Move Encoding

When a node's position changes (new parent, different distance, different edge type), we emit an explicit `MOVED` change rather than a remove+add pair.

**Why?**
- No ordering dependencies between operations
- Clear semantics for reparenting, distance changes, and edge-type changes
- Since Atlas has no external consumers yet, we can pick the cleanest model for future clients

**The three change types:**
- `ADDED`: Node newly entered the canonical set
- `REMOVED`: Node left the canonical set
- `MOVED`: Node stayed canonical but changed position (same payload as `ADDED`)

## Canonical Edge Semantics

Each node has exactly one canonical parent edge—the first one discovered by BFS at minimum distance. This means diffs are lossy: we don't emit alternative edges or longer paths to the same node.

The root node is implicit and never appears in diffs. Distance values are authoritative for consumers that need hop counts.

## Diff Computation Approach

We'll use a **sorted vector merge**:

1. Build a `Position` map for old and new graphs via BFS (first-seen per space_id)
2. Convert to sorted vectors of `(space_id, position)`
3. Merge-join to emit added/removed/changed entries

**Why this approach?**
- Deterministic output
- Simple to implement and operate
- Fast enough in practice for 100k nodes with small change rates

**Benchmark results** (mocked 100k nodes, ~1% add/remove/move):
- Sorted merge diff-only: ~0.5–0.7ms
- Build+diff (including data construction): ~70–90ms
- Roaring/BTree are competitive but add complexity (need UUID→integer mapping)

## Open Questions

- Do we want a schema_version field, or rely on topic versioning?
- Should we publish a compatibility guarantee for consumer replays?

## Alternatives Considered

- **HashMap comparison**: Simple but slower due to random access patterns
- **RoaringTreemap with UUID→u64 mapping**: Competitive performance but adds persistent mapping complexity
- **Event-driven diffs**: Faster in typical cases but significantly more complex to implement

## Migration Plan

1. Add diff messages to schema (including `MOVED`)
2. Emit diffs to new Kafka topics (canonical and transitive)
3. Optionally keep full snapshots during transition
