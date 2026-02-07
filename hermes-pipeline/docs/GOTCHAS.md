# Gotchas

## Event sequencing

### Sequence numbers

The `sequence` field in BlockchainMetadata is the action's index in `Actions.actions` from substreams. It's per-block, not global. Two events in different blocks can have the same sequence number.

### is_last flag

Exactly one event per block has `is_last = true`. This is the event with the highest sequence number. If a block has events across multiple topics, only one of them gets the flag.

### Empty blocks

Blocks with no relevant actions emit no events. Consumers should not expect a signal for empty blocks.

## Governance

### Fast-path proposals don't emit PROPOSAL_EXECUTED

The DAOSpace contract auto-executes fast-path proposals inline when a YES vote meets the threshold (inside `_vote` → `_executeProposal`), but does **not** emit a `PROPOSAL_EXECUTED` event. The `PROPOSAL_EXECUTED` event is only emitted when someone explicitly calls `enter(PROPOSAL_EXECUTED)` — which is the path used for slow-path proposals and manual execution after criteria are met.

This means the pipeline never sees a `PROPOSAL_EXECUTED` event for fast-path auto-executions. The kg-indexer compensates by detecting fast-path execution in its tally worker: after updating vote counts, it checks for fast-path proposals where `yes_count > threshold` and `executed_at IS NULL`, then infers `executed_at` from the latest vote timestamp.

