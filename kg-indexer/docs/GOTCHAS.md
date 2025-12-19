# Gotchas

## Block buffering

### Stale blocks

If hermes-pipeline crashes mid-block, the `is_last` event never arrives. kg-indexer will buffer events indefinitely until the stale timeout (default 60s, configurable via `BLOCK_STALE_TIMEOUT_SECS`). After timeout, warnings are logged but events remain buffered.

**Resolution**: Restart hermes-pipeline. It will re-emit the block from its cursor position.

### Consumer restart

On restart, any buffered events are lost. This is fine because Kafka offsets are only committed after a full block is processed. The consumer will re-receive all events for any incomplete blocks.

### Missing block metadata

Events without `block_number` in their metadata are processed immediately (no buffering). This provides backwards compatibility with older hermes-pipeline versions. A warning is logged when this happens.

## Sequence numbers

Sequence numbers are per-block, not global. They represent the action's index in the substream `Actions.actions` array. Two events in different blocks can have the same sequence number.
