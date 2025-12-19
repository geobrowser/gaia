# Architecture Decision Records

## ADR-001: Event sequencing for cross-topic ordering

**Status**: Accepted

**Context**: hermes-pipeline emits events to multiple Kafka topics (space.creations, space.membership, knowledge.edits, etc.). Kafka only guarantees ordering within a single topic partition, not across topics. Consumers like kg-indexer may receive events out of order - for example, an edit referencing a space might arrive before the space creation event.

**Decision**: Add explicit `sequence` (uint32) and `is_last` (bool) fields to BlockchainMetadata. The sequence is derived from the action's index in the `Actions.actions` array from substreams, which is already in blockchain order. hermes-pipeline marks the final event with `is_last = true`. Consumers buffer events until `is_last` is received, then sort by sequence and process in blockchain order.

**Consequences**:
- Ordering is guaranteed within a block, regardless of Kafka topic
- Consumers must buffer events (memory usage proportional to block size)
- Adds complexity to both producer and consumer
- Requires handling edge cases: empty blocks, partial blocks on restart, producer crashes
- Alternative considered: single topic (rejected - other consumers only need specific event types)
