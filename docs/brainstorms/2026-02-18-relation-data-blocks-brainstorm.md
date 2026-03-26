---
date: 2026-02-18
topic: relation-data-blocks
---

# Relation Data Blocks

## What We're Building
We are defining a dedicated relation data block spec for querying graph edges that originate from a single, required source entity. The relation block should return matching relation entities (edges) plus their TO entities by default, since the FROM entity is already known by the anchor.

The v1 behavior is intentionally constrained: one-hop traversal only, no recursive traversal semantics, and default inclusion of all relation types unless the caller filters by type. Filtering should be expressive where it matters for this use case: relation entity fields and TO entity fields.

## Why This Approach
We evaluated three directions: a dedicated anchored one-hop relation block, a strict profile layered on the generic filter spec, and a new relation DSL with recursion. We chose the dedicated anchored one-hop model because it best matches the product goal of predictable semantics while still allowing meaningful filtering expressiveness.

This is the most YAGNI-aligned option for v1. It avoids over-designing traversal logic before the query model is validated, while still preserving future extension points (for example, recursive traversal or output mode variants) if real usage demands them later.

## Key Decisions
- Relation blocks are source-anchored: every block requires exactly one FROM entity anchor.
- Default output is edge-centric: return relation entities plus TO entities.
- Traversal is one hop in v1: no recursive or multi-hop behavior.
- Filter scope in v1 includes relation entity fields and TO entity fields.
- Relation type is optional in v1: when omitted, all relation types are included.
- Multiplicity is preserved: if multiple matching edges point to the same TO entity, keep all edges.
- Compatibility with existing query/collection block ergonomics is not a v1 optimization goal.

## Open Questions
- Should response payloads be strictly edge-first (edges as primary records) with TO hydration attached, or expose two top-level collections (`relations`, `entities`) with references?
- Should relation-type filtering live in a dedicated top-level field, in normal filter predicates, or both?
- Should ordering and pagination semantics be explicitly out of scope for this RFC (as in 0004), or constrained for relation blocks in v1?
- What versioning trigger should be defined for future recursive traversal support so v1 contracts stay stable?

## Next Steps
-> `/workflows:plan` to define implementation scope, schema contracts, and verification.
