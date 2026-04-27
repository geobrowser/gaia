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

## ADR-003: Root-space gating for system property relations

**Status**: Accepted

**Context**: The four onchain-derived system properties (`SpaceAddress`, `VotingMode`, `SpaceId`, `CreatedAtBlock`, enumerated in `sdk::core::ids::PROTECTED_PROPERTY_IDS`) need schema relations in the graph (e.g. `Types: Property`, `Data Type: Text/Integer`) so they look like first-class properties. Authoring those relations from arbitrary spaces would let any space redefine the schema of system properties. At the same time, their *values* are written directly by `handlers::system_entities::map_space_registered` from onchain events; permitting edit-pipeline writes for those values would create two writers racing for the same value rows.

**Decision**: Edits from any space are blocked from authoring relations whose `from`/`to` is in `PROTECTED_PROPERTY_IDS`, with one exception: the root space (`sdk::core::ids::ROOT_SPACE_ID`) may author such relations to define the system-property schema. Values for those properties remain non-writable via edits — even from the root space — because the onchain handler is the sole writer.

**Consequences**:
- Schema for system properties is authored exactly once, by the root space
- Non-root spaces still have system-property relations dropped, preserving prior behavior
- Onchain handler retains exclusive ownership of system-property values, so no row-level write races
- If the root space is ever migrated, `sdk::core::ids::ROOT_SPACE_ID` must be updated alongside the indexer's gating check
- Implementation lives in GEO-563/564; the SDK constant is GEO-562
