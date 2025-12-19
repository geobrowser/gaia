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

**Update**: Superseded by ADR-002 for cross-topic ordering within blocks.

## ADR-002: Block-level buffering for cross-topic ordering

**Status**: Accepted

**Context**: ADR-001 processed each Kafka message independently. However, Kafka doesn't guarantee ordering across topics. An edit referencing a space could arrive before the space creation event, causing foreign key violations or missing data.

**Decision**: Buffer events by block number until `is_last` is received, then process all events for that block in sequence order within a single transaction. This relies on hermes-pipeline adding `sequence` and `is_last` fields to BlockchainMetadata (see hermes-pipeline ADR-001).

**Consequences**:
- Events within a block are processed in blockchain order
- Single transaction per block maintains atomicity
- Memory usage proportional to events per block (typically small)
- Kafka offsets only committed after full block is processed
- If `is_last` never arrives, stale block detection warns after timeout
- Messages without block metadata fall back to immediate processing
