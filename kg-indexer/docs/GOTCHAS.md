# Gotchas

## Block buffering

### Stale blocks

If hermes-pipeline crashes mid-block, the `is_last` event never arrives. kg-indexer will buffer events indefinitely until the stale timeout (default 60s, configurable via `BLOCK_STALE_TIMEOUT_SECS`). After timeout, warnings are logged but events remain buffered.

**Resolution**: Restart hermes-pipeline. It will re-emit the block from its cursor position.

### Consumer restart

On restart, any buffered events are lost. This is fine because Kafka offsets are only committed after a full block is processed. The consumer will re-receive all events for any incomplete blocks.

### Missing block metadata

Events without `block_number` in their metadata are processed immediately (no buffering). This provides backwards compatibility with older hermes-pipeline versions. A warning is logged when this happens.

## Logging and tracing

kg-indexer emits structured, low-volume "canonical logs" for batch processing:

- `kg_indexer.batch_start` — intent for a block batch (counts, offsets, partitions, reason)
- `kg_indexer.batch_end` — outcome (durations, counts, missing types, commit failures)
- `kg_indexer.event_error` — per-event failures only (includes `event_id`)

We also consume `hermes.blocks` (canonical block summaries) so batches can close even when `is_last` is missing on the topics we read.

### Debug flag: event IDs

Set `LOG_EVENT_IDS=true` to emit a per-event log line with the Kafka `event-id` header. This is off by default to keep log volume low.

### OTEL tracing

If `OTEL_URL` is set, traces are exported via OTLP (HTTP). Otherwise, logs go to stdout (console backend). `OTEL_DEBUG=true` mirrors spans to stdout when OTLP is enabled.

## Sequence numbers

Sequence numbers are per-block, not global. They represent the action's index in the substream `Actions.actions` array. Two events in different blocks can have the same sequence number.
