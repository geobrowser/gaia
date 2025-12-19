# Gotchas

## Event sequencing

### Sequence numbers

The `sequence` field in BlockchainMetadata is the action's index in `Actions.actions` from substreams. It's per-block, not global. Two events in different blocks can have the same sequence number.

### is_last flag

Exactly one event per block has `is_last = true`. This is the event with the highest sequence number. If a block has events across multiple topics, only one of them gets the flag.

### Empty blocks

Blocks with no relevant actions emit no events. Consumers should not expect a signal for empty blocks.
