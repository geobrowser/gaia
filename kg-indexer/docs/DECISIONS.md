# Architecture Decision Records

## ADR-001: Per-message processing instead of cross-message batching

**Status**: Accepted

**Context**: Initial implementation batched operations across multiple Kafka messages (flush on 1000 ops or 5 second timeout). This introduced complexity around:
- Ordering guarantees across topics (Kafka only guarantees order within a partition)
- Squashing conflicting operations (e.g., ADD then REMOVE for same entity)
- Block boundary alignment (batch boundaries were arbitrary, not aligned with blockchain blocks)

**Decision**: Process each Kafka message individually with its own database transaction. Bulk inserts are still used within a single edit message (which can contain thousands of ops).

**Consequences**:
- Simpler code, easier to reason about
- No cross-topic ordering issues
- Each message is atomic (all-or-nothing)
- Slightly more DB transactions, but edit ops (the high-volume case) are still batched
- Can revisit batching later if performance requires it
