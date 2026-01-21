# Graph Diff Emission (RFC)

## Summary
Emit incremental graph diffs for canonical and transitive graphs instead of full snapshots. This RFC defines the diff message shape, batching/ordering semantics, and the diff computation approach, including an explicit move encoding.

## Goals
- Emit compact diffs for canonical and transitive graphs via Kafka.
- Support consumer replay to reconstruct the canonical tree and node membership.
- Provide deterministic, stable output for testing and downstream processing.
- Keep computation fast for 100k+ node graphs with small change rates.

## Non-Goals
- Changing how canonical and transitive graphs are computed.
- Providing multiple edge paths to the same node (diffs remain tree-like and lossy).
- Introducing incremental graph mutation in Atlas (tracking changes during traversal).

## Current State
Atlas emits full canonical trees on change detection. The canonical graph is computed via BFS and topic-edge attachments, and changes are detected using tree hashes. Diffs are described in:
- `atlas/docs/graph-diff-emission.md`
- `atlas/docs/graph-diff-algorithm-exploration.md`

This RFC formalizes the emission format and algorithm choice.

## Proposed Diff Messages (Schema)
We introduce canonical and transitive diff messages:

```
message CanonicalGraphDiff {
  bytes root_id = 1;
  repeated NodeChange changes = 2;
  blockchain_metadata.BlockchainMetadata meta = 3;
  uint32 schema_version = 4;
}

message TransitiveGraphDiff {
  bytes root_id = 1;
  repeated NodeChange changes = 2;
  blockchain_metadata.BlockchainMetadata meta = 3;
  uint32 schema_version = 4;
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
- A diff event represents a complete batch of changes for a single graph update and should be applied atomically.
- Changes are emitted in a deterministic order: sorted by `space_id`.

## Move Encoding
We will use an explicit `MOVED` change type for nodes whose position changes.

### Rationale
- Avoids ordering dependencies between add/remove.
- Clear semantics for reparenting, distance changes, and edge-type changes.
- Atlas currently has no external consumers, so we can choose the least error-prone model for future clients.

### Behavior
- `MOVED` carries the same payload as `ADDED` (new position).
- `REMOVED` indicates the node left the canonical set.
- `ADDED` indicates a node newly entered the canonical set.

## Canonical Edge Semantics
- Each node has exactly one canonical parent edge (first discovered by BFS at minimum distance).
- Diffs are lossy: alternative edges or longer paths are not emitted.
- Root is implicit and is not emitted in diffs.
- Distance is authoritative for consumers that need hop counts.

## Diff Computation Approach
We will use **sorted vector merge** (Approach #2):
1) Build a `Position` map for old and new graphs via BFS (first-seen per space_id).
2) Convert to sorted vectors of `(space_id, position)`.
3) Merge-join to emit added/removed/changed entries.

Rationale:
- Deterministic output.
- Simple implementation and operational model.
- Fast in practice for 100k nodes with small change rates.

Benchmark summary (mocked 100k nodes, ~1% add/remove/move):
- Sorted merge diff-only: ~0.5–0.7ms
- Build+diff (including data construction): ~70–90ms
- Roaring/BTree are competitive but add complexity (UUID→integer mapping).

## Open Questions
- Do we want to enforce a schema_version field or rely on topic versioning?
- Should we publish a compatibility guarantee for consumer replays?

## Alternatives Considered
- HashMap comparison: simple but slower due to random access.
- RoaringTreemap with UUID→u64 mapping: competitive but adds persistent mapping complexity.
- Event-driven diffs: faster in typical cases but higher implementation complexity.

## Migration Plan (High-Level)
- Add diff messages to schema (including MOVED).
- Emit diffs to new Kafka topics (canonical and transitive).
- Keep full snapshots during transition (optional).
