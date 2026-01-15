# Aggregation Mode (Spaces DAG)

## Goal
Support querying entity data from a space **and** its transitive graph, with deterministic conflict resolution.

## Graph Sources
- **Transitive graph**: per-space reachability rooted at the requested `spaceId`.
- **Canonical graph**: global reachability rooted at the blessed root space (Atlas output).

## API Surface (REST)
```
GET /entities/:id?spaceId=...&spaceScope=local|transitive|canonical
```

- `spaceScope=local` (default): only the requested `spaceId`.
- `spaceScope=transitive`: use the per-space transitive closure.
- `spaceScope=canonical`: use the global canonical graph.

## Reachability Storage
Persist reachability tables from Atlas:

```
space_reachability_transitive (
  root_space_id uuid,
  reachable_space_id uuid,
  distance int
)

space_reachability_canonical (
  root_space_id uuid,   -- blessed root
  reachable_space_id uuid,
  distance int
)
```

Indexes:
- `(root_space_id, reachable_space_id)`
- `(root_space_id, distance)`

## Conflict Resolution
Default policy: **nearest** (no voting/ranking in v1).

Rules:
- **Values**: for the same `(entity_id, property_id)`, choose the value from the closest space.
- **Relations**: for the same `relation_id`, choose the relation from the closest space.

Deterministic tie-breaker:
- `ORDER BY distance ASC, space_id ASC`

SQL pattern:
```
ROW_NUMBER() OVER (
  PARTITION BY <conflict_key>
  ORDER BY distance ASC, space_id ASC
)
```

## Notes
- If only local data is desired, callers should use `spaceScope=local`.
- Voting/ranking signals exist per space; v1 will not use them for conflict resolution.
- Cycles are handled by Atlas; reachability consumers can assume DAG semantics.
- If multiple candidates have the same distance, use a stable tie-breaker (e.g., `space_id`).
- Aggregation requires either `spaceId` or `spaceScope=canonical`.
- Consider adding `distance` to Atlas emissions for reachability to avoid reifying tree shape downstream.
