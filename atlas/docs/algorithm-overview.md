# Atlas Algorithm Overview

This document provides a high-level summary of how the transitive and canonical graph algorithms work together to process topology events.

## Data Flow on Graph Change Event

```
1. Event Received (e.g., HermesSpaceTrustExtension)
   ↓
2. Update GraphState (nodes, edges, topic memberships)
   ↓
3. Invalidate affected TransitiveCache entries
   ↓
4. Recompute Canonical Graph (using TransitiveCache)
   ↓
5. Hash and compare with previous
   ↓
6. If changed: Persist to PostgreSQL + Emit CanonicalGraphUpdated to Kafka
```

## Step-by-Step Example

**Event**: `VerifiedExtension { source: A, target: B }`

### Step 1-2: Update GraphState

```rust
explicit_edges[A].insert(B, Verified)
```

### Step 3: Invalidate TransitiveCache

```
- Invalidate A's transitive graphs (A now reaches more nodes)
- Invalidate all spaces whose transitive graph contains A (reverse deps)
- Invalidate B's transitive graphs (if edges changed)
```

### Step 4: Recompute Canonical Graph

**Phase 1 - Get canonical set:**

```rust
// O(1) lookup (recomputes if invalidated)
root_transitive = transitive_cache.get_explicit_only(root)
canonical_set = root_transitive.flat  // {Root, A, B, C, ...}
tree = root_transitive.tree
```

**Phase 2 - Add topic edges:**

```rust
for each topic_edge (source → topic_id) where source is canonical:
    members = topic_spaces[topic_id]  // spaces that announced this topic

    for each member in members:
        if member in canonical_set:
            // O(1) lookup (recomputes if invalidated)
            member_transitive = transitive_cache.get_full(member)

            // Filter to only canonical nodes
            filtered_subtree = filter(member_transitive.tree, canonical_set)

            // Attach to tree
            tree[source].children.add(filtered_subtree)
```

### Step 5-6: Change Detection and Emission

```rust
new_hash = hash(tree)
if new_hash != current_hash:
    persist_to_postgres(tree, canonical_set)
    emit_to_kafka(CanonicalGraphUpdated { tree, canonical_set })
    current_hash = new_hash
```

## How Transitive and Canonical Work Together

```
┌─────────────────────────────────────────────────────────────┐
│                      GraphState                              │
│  (nodes, explicit_edges, topic_edges, topic_spaces)         │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                   TransitiveProcessor                        │
│                                                              │
│  TransitiveCache:                                            │
│    explicit_only[space] → tree + flat set (explicit edges)  │
│    full[space] → tree + flat set (explicit + topic edges)   │
│    reverse_deps[space] → spaces that include this space     │
│                                                              │
│  On invalidation: remove from cache, lazy recompute         │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                   CanonicalProcessor                         │
│                                                              │
│  Phase 1: canonical_set = cache.get_explicit_only(root).flat│
│                                                              │
│  Phase 2: for each topic edge from canonical nodes:         │
│             attach cache.get_full(member) filtered to       │
│             canonical_set                                    │
│                                                              │
│  Output: CanonicalGraphUpdated (if changed)                 │
└─────────────────────────────────────────────────────────────┘
```

## Performance Characteristics

The `TransitiveProcessor` does the heavy lifting (BFS traversal). The `CanonicalProcessor` becomes mostly lookups and filtering:

| Operation | Without TransitiveCache | With TransitiveCache |
|-----------|------------------------|----------------------|
| Phase 1 | O(explicit_edges) BFS | O(1) lookup |
| Phase 2 subtree | O(subtree) traversal | O(subtree) clone + filter |
| Invalidation | N/A | O(reverse_deps) |

The trade-off is memory (storing transitive graphs) and invalidation complexity, but computation per event is significantly reduced.

## Related Documents

- [0001: Canonical Graph Implementation Plan](./agents/plans/0001-canonical-graph-implementation-plan.md)
- [0002: Transitive Graph Implementation Plan](./agents/plans/0002-transitive-graph-implementation-plan.md)
- [Graph Concepts](./graph-concepts.md)
- [Storage Design](./storage.md)
