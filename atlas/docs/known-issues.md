# Known Issues

## Duplicate Event Emission on Restart

**Status**: Accepted (not yet addressed)

### Problem

When Atlas restarts, it resumes from the last persisted cursor and recomputes the canonical graph. If the graph state is identical to what was previously emitted, Atlas will emit a duplicate `CanonicalGraphUpdated` message to Kafka.

### Impact

Downstream consumers may receive the same canonical graph update multiple times. If consumers are applying full state replacements (not accumulating deltas), this is harmless—they just overwrite with identical data.

### Potential Solutions

1. **Producer-side deduplication**: Persist the last emitted `tree_hash` per root alongside the cursor. Skip emission if the recomputed hash matches.

2. **Kafka log compaction with content-based keys**: Use `(root_id, tree_hash)` as the message key. Compaction deduplicates in storage, though consumers may still see duplicates in-flight.

3. **Transactional outbox pattern**: Atomically persist state + outbox record, then emit from outbox. Guarantees exactly-once emission but adds complexity.

4. **Consumer-side deduplication**: Include `tree_hash` in the message. Consumers store the last seen hash per root and skip duplicates.

### Current Approach

We accept occasional duplicate emissions. Downstream consumers should treat `CanonicalGraphUpdated` as a full state snapshot and apply it idempotently.

### Future Considerations

If duplicate emissions cause issues (e.g., triggering expensive downstream reprocessing), revisit with producer-side deduplication (option 1) as the simplest fix.
