# ADR 0002: Graph Diff Emission

## Status
Proposed

## Date
2026-01-21

## Context
Atlas should emit incremental changes for canonical and transitive graphs instead of full snapshots. We need deterministic, replayable updates for consumers, while keeping computation fast for large graphs. This ADR sets the intended message shape, batching/ordering semantics, and the diff computation approach.

## Decision
- Emit canonical and transitive graph diffs instead of full snapshots.
- Use the following diff message schema:

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

- A diff event is a complete batch for a single graph update and must be applied atomically.
- Changes are emitted in a deterministic order: sorted by `space_id`.
- Use explicit `MOVED` to represent reparenting or distance changes.
  - `MOVED` carries the same payload as `ADDED` (new position).
  - `REMOVED` indicates the node left the canonical set.
  - `ADDED` indicates the node newly entered the canonical set.
- Canonical edge semantics are lossy: each node has exactly one canonical parent edge (first discovered by BFS at minimum distance). Root is implicit and not emitted in diffs.
- Diff computation uses sorted vector merge:
  1) Build a `Position` map for old and new graphs via BFS (first-seen per `space_id`).
  2) Convert to sorted vectors of `(space_id, position)`.
  3) Merge-join to emit added/removed/changed entries.

## Consequences
- Diffs are tree-like and cannot reconstruct alternate paths or edge-type-specific traversals.
- Consumers can rely on deterministic ordering and distances for replay.
- Schema changes add `MOVED` and require schema versioning or topic versioning.

## Alternatives Considered
- HashMap comparison: simpler but slower due to random access.
- RoaringTreemap with UUID→u64 mapping: competitive but adds persistent mapping complexity.
- Event-driven diffs: faster in typical cases but higher implementation complexity.

## Open Questions
- Do we want to enforce a schema_version field or rely on topic versioning?
- Should we publish a compatibility guarantee for consumer replays?

## Migration Plan
- Add diff messages to schema (including `MOVED`).
- Emit diffs to new Kafka topics (canonical and transitive).
- Keep full snapshots during transition (optional).
