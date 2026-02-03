# Graph Diff Emission

This document describes how Atlas emits incremental graph diffs to Kafka instead of full tree snapshots.

## Overview

Rather than emitting the complete canonical tree on every change, Atlas emits diffs that describe what changed. This reduces message size and allows consumers to incrementally update their local state.

## Diff Message Format

```protobuf
message CanonicalGraphDiff {
  bytes root_id = 1;
  repeated NodeChange changes = 2;
  BlockchainMetadata meta = 3;
}

message NodeChange {
  bytes space_id = 1;
  ChangeType type = 2;              // ADDED, REMOVED
  optional uint32 distance = 3;     // minimum hops from root (for ADDED)
  optional EdgeInfo parent_edge = 4; // how this node was reached (for ADDED)
}

message EdgeInfo {
  bytes source = 1;                 // parent node
  EdgeType edge_type = 2;           // VERIFIED, RELATED, TOPIC, EDITOR, MEMBER
  optional bytes topic_id = 3;      // present only for TOPIC edges
}

enum ChangeType {
  ADDED = 0;
  REMOVED = 1;
}
```

## Design Decisions

### Single Canonical Edge Per Node

Each node in the canonical tree has exactly one parent edge - the edge through which it was first discovered during BFS traversal (shortest path). Alternative edges that also reach the node are **not** emitted.

**Implications:**
- No cycles in the diff (tree structure)
- Distances are unambiguous
- Simple to reason about

**Limitation (Lossy):**
If multiple edges reach the same node via different edge types, only the shortest path edge is emitted. Consumers cannot reconstruct edge-type-specific traversals (e.g., "show me only verified edges").

Example of lost information:
```
Root ──verified──> A ──verified──> B   (distance 2)
Root ──related───> B                    (distance 1, canonical path)
```

Only `Root → B (related)` is emitted. The `A → B (verified)` edge exists but is not included in diffs.

### Moves: Explicit MOVED

When a node's position in the tree changes (different parent, distance, edge type, or topic_id), emit a `MOVED` change that carries the new position (same payload as `ADDED`). This applies to the node and potentially its descendants if their distances change.

**Batching and ordering:** A diff event represents a complete batch of changes for a single graph update and should be applied atomically. Changes are emitted in a deterministic order (sorted by `space_id`).

## Consumer Types

Different consumers use different parts of the diff:

| Consumer Need | Uses |
|---------------|------|
| Membership only (is X canonical?) | `space_id` + `type` (ignore distance/edge) |
| Membership + distance | `space_id` + `type` + `distance` |
| Full tree reconstruction | All fields including `parent_edge` |

## Edge Types

The system supports these edge types:

| Edge Type | Description |
|-----------|-------------|
| `VERIFIED` | Strong explicit trust relationship |
| `RELATED` | Weaker explicit trust relationship |
| `TOPIC` | Dynamic membership via shared topic (includes `topic_id`) |
| `EDITOR` | Membership edge - editor role |
| `MEMBER` | Membership edge - member role |

## Examples

### Example Tree Structure

```
Root (d=0, topic: T_ROOT)
│
├── A (verified, d=1, topic: T_A)
│   │
│   ├── B (verified, d=2, topic: T_B)
│   │   ├── C (verified, d=3)
│   │   ├── D (related, d=3)
│   │   └── E (topic:T_B, d=3)  ← E announced T_B, B has topic edge to T_B
│   │       └── F (verified, d=4)
│   │
│   └── G (related, d=2, topic: T_SHARED)
│       └── H (verified, d=3)
│
├── I (verified, d=1, topic: T_SHARED)
│   ├── J (verified, d=2)
│   │   └── K (verified, d=3)
│   │       └── L (related, d=4)
│   │           └── M (verified, d=5)
│   │
│   └── N (topic:T_SHARED, d=2)  ← N announced T_SHARED, I has topic edge
│       └── O (verified, d=3)
│           └── P (related, d=4)
│
├── Q (related, d=1)
│   ├── R (verified, d=2)
│   │   └── S (verified, d=3)
│   │       └── T (verified, d=4)
│   │           └── U (verified, d=5)
│   │
│   └── V (editor, d=2)  ← membership edge
│       └── W (member, d=3)  ← membership edge
│
└── X (topic:T_ROOT, d=1)  ← X announced T_ROOT, Root has topic edge
    └── Y (verified, d=2)
        └── Z (related, d=3)
```

**Legend:**
- `(verified, d=N)` - reached via Verified edge at distance N
- `(related, d=N)` - reached via Related edge
- `(topic:T, d=N)` - reached via Topic edge (space announced topic T)
- `(editor, d=N)` - reached via Editor membership edge
- `(member, d=N)` - reached via Member membership edge

---

### Scenario 1: Shorter Path Found - Subtree Cascade

**Event:** New edge `Root → J` (verified) added

J was at distance 2 via I, now reachable at distance 1 directly from Root. This affects J and all descendants.

**Before:**
```
├── I (verified, d=1)
│   ├── J (verified, d=2)
│   │   └── K (verified, d=3)
│   │       └── L (related, d=4)
│   │           └── M (verified, d=5)
```

**After:**
```
├── I (verified, d=1)
│
├── J (verified, d=1)  ← was d=2 under I
│   └── K (verified, d=2)  ← was d=3
│       └── L (related, d=3)  ← was d=4
│           └── M (verified, d=4)  ← was d=5
```

**Diff:**
```json
{
  "changes": [
    { "space_id": "J", "type": "REMOVED" },
    { "space_id": "K", "type": "REMOVED" },
    { "space_id": "L", "type": "REMOVED" },
    { "space_id": "M", "type": "REMOVED" },
    { "space_id": "J", "type": "ADDED", "distance": 1, "parent_edge": { "source": "Root", "edge_type": "VERIFIED" } },
    { "space_id": "K", "type": "ADDED", "distance": 2, "parent_edge": { "source": "J", "edge_type": "VERIFIED" } },
    { "space_id": "L", "type": "ADDED", "distance": 3, "parent_edge": { "source": "K", "edge_type": "RELATED" } },
    { "space_id": "M", "type": "ADDED", "distance": 4, "parent_edge": { "source": "L", "edge_type": "VERIFIED" } }
  ]
}
```

Note: I is unchanged. N (under I via topic edge) is also unchanged.

---

### Scenario 2: New Node Joins via Topic Edge

**Event:** New space `AA` announces topic `T_B`. B has a topic edge to `T_B`.

**Before:**
```
├── A (verified, d=1)
│   ├── B (verified, d=2)
│   │   ├── C (verified, d=3)
│   │   ├── D (related, d=3)
│   │   └── E (topic:T_B, d=3)
│   │       └── F (verified, d=4)
```

**After:**
```
├── A (verified, d=1)
│   ├── B (verified, d=2)
│   │   ├── C (verified, d=3)
│   │   ├── D (related, d=3)
│   │   ├── E (topic:T_B, d=3)
│   │   │   └── F (verified, d=4)
│   │   └── AA (topic:T_B, d=3)  ← NEW
```

**Diff:**
```json
{
  "changes": [
    { "space_id": "AA", "type": "ADDED", "distance": 3, "parent_edge": { "source": "B", "edge_type": "TOPIC", "topic_id": "T_B" } }
  ]
}
```

---

### Scenario 3: Parent Changes, Same Distance

**Event:** Edge `Q → R` removed, alternate path `A → R` (related) exists

R was under Q at distance 2. Now under A at distance 2 (same distance, different parent).

**Before:**
```
├── A (verified, d=1)
│   ├── B (verified, d=2)
│   └── G (related, d=2)
│
├── Q (related, d=1)
│   ├── R (verified, d=2)
│   │   └── S (verified, d=3)
│   │       └── T (verified, d=4)
│   │           └── U (verified, d=5)
│   └── V (editor, d=2)
```

**After:**
```
├── A (verified, d=1)
│   ├── B (verified, d=2)
│   ├── G (related, d=2)
│   └── R (related, d=2)  ← moved from Q
│       └── S (verified, d=3)
│           └── T (verified, d=4)
│               └── U (verified, d=5)
│
├── Q (related, d=1)
│   └── V (editor, d=2)
```

**Diff:**
```json
{
  "changes": [
    { "space_id": "R", "type": "REMOVED" },
    { "space_id": "R", "type": "ADDED", "distance": 2, "parent_edge": { "source": "A", "edge_type": "RELATED" } }
  ]
}
```

S, T, U don't appear - their parent (R) and distances are unchanged.

---

### Scenario 4: Subtree Disconnected Entirely

**Event:** Edge `I → N` topic edge removed, no alternate path to N exists

**Before:**
```
├── I (verified, d=1)
│   ├── J (verified, d=2)
│   └── N (topic:T_SHARED, d=2)
│       └── O (verified, d=3)
│           └── P (related, d=4)
```

**After:**
```
├── I (verified, d=1)
│   └── J (verified, d=2)
```

**Diff:**
```json
{
  "changes": [
    { "space_id": "N", "type": "REMOVED" },
    { "space_id": "O", "type": "REMOVED" },
    { "space_id": "P", "type": "REMOVED" }
  ]
}
```

---

### Scenario 5: Membership Edge Removed

**Event:** `EDITOR_REMOVED` for V from Q, no alternate path to V exists

**Before:**
```
├── Q (related, d=1)
│   ├── R (verified, d=2)
│   └── V (editor, d=2)
│       └── W (member, d=3)
```

**After:**
```
├── Q (related, d=1)
│   └── R (verified, d=2)
```

**Diff:**
```json
{
  "changes": [
    { "space_id": "V", "type": "REMOVED" },
    { "space_id": "W", "type": "REMOVED" }
  ]
}
```

---

### Scenario 6: Edge Type Changes (Shorter Path via Different Type)

**Event:** Add edge `Root → E` (verified)

E was at distance 3 via topic edge from B. Now distance 1 via verified edge from Root.

**Before:**
```
├── A (verified, d=1)
│   └── B (verified, d=2)
│       └── E (topic:T_B, d=3)
│           └── F (verified, d=4)
```

**After:**
```
├── A (verified, d=1)
│   └── B (verified, d=2)
│
├── E (verified, d=1)  ← was d=3 via topic, now d=1 via verified
│   └── F (verified, d=2)  ← was d=4
```

**Diff:**
```json
{
  "changes": [
    { "space_id": "E", "type": "REMOVED" },
    { "space_id": "F", "type": "REMOVED" },
    { "space_id": "E", "type": "ADDED", "distance": 1, "parent_edge": { "source": "Root", "edge_type": "VERIFIED" } },
    { "space_id": "F", "type": "ADDED", "distance": 2, "parent_edge": { "source": "E", "edge_type": "VERIFIED" } }
  ]
}
```

Note: E's edge type changed from TOPIC to VERIFIED, and distance changed from 3 to 1.

---

## Summary: What Triggers Diff Entries

| Scenario | Nodes in Diff | Reason |
|----------|---------------|--------|
| Shorter path found | Node + all descendants | All distances change |
| Parent changes, same distance | Only the reparented node | Descendants unchanged |
| Node leaves canonical set | Node + all descendants | All leave canonical set |
| New node joins | Just the new node | New to canonical set |
| Edge type changes | Node (+ descendants if distance changes) | Parent edge changed |

## Related Documents

- [Implementation Plan](./agents/plans/0006-live-substream-membership-diffing-plan.md) - Full implementation plan
- [Algorithm Overview](./algorithm-overview.md)
- [Graph Concepts](./graph-concepts.md)
- [Canonical Graph Implementation](./canonical-graph-implementation.md)
