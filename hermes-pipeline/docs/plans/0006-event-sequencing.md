# Plan: Event Sequencing for Cross-Topic Ordering

**Status**: Implemented

## Problem

Kafka only guarantees ordering within a topic partition, not across topics. kg-indexer consumes from multiple topics (space.creations, space.membership, knowledge.edits, etc.) and may receive events out of order. For example, an edit referencing a space might arrive before the space creation event.

## Solution

Add explicit sequence numbers based on blockchain order (the order events appear in the block log from Amp). The consumer buffers events and processes them in sequence order once all events for a block are received.

## Changes

### 1. Update BlockchainMetadata proto

```protobuf
// hermes-schema/proto/blockchain_metadata.proto
message BlockchainMetadata {
    uint64 created_at = 1;
    bytes created_by = 2;
    uint64 block_number = 3;
    string cursor = 4;
    uint32 sequence = 5;    // Blockchain order within block (from action index)
    bool is_last = 6;       // True for final event in block
}
```

### 2. Use action array index as sequence

The `Actions.actions` repeated field from Amp is already in blockchain order. Use the array index as the sequence number:

```rust
// hermes-pipeline/src/pipelines/spaces.rs
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<SpacesOutput> {
    let mut events = Vec::new();

    for (index, action) in actions.iter().enumerate() {
        if let Some(mut space) = process_space_action(action, meta)? {
            space.meta.sequence = index as u32;  // Array index = blockchain order
            events.push(space);
        }
    }

    Ok(SpacesOutput { events })
}
```

This is simpler than parsing the cursor and doesn't require changes to the action schema.

### 3. Propagate index through all pipelines

Each pipeline receives the actions array and uses enumeration to get the sequence:

- `spaces::transform` - uses index
- `membership::transform` - uses index
- `trust::transform` - uses index
- `edits::transform` - uses index
- etc.

The sequence is set on `BlockchainMetadata.sequence` for each event.

### 4. Mark last event at emission time

After all events are collected, mark the highest sequence as `is_last`:

```rust
// In process_block_impl
let mut all_events: Vec<&mut dyn HasMeta> = vec![];
// Collect all events...

// Find max sequence and mark as last
if let Some(last) = all_events.iter_mut().max_by_key(|e| e.meta().sequence) {
    last.meta_mut().is_last = true;
}

// Emit all events (order doesn't matter, consumer will reorder)
for event in all_events {
    emitter.emit(event)?;
}
```

### 4. Update kg-indexer consumer

Buffer events until `is_last` is received, then sort by sequence and process:

```rust
// kg-indexer/src/main.rs
let mut buffer: HashMap<u64, Vec<Event>> = HashMap::new();

for msg in consumer.poll() {
    let block = msg.meta.block_number;
    buffer.entry(block).or_default().push(msg);

    if msg.meta.is_last {
        let mut events = buffer.remove(&block).unwrap();
        events.sort_by_key(|e| e.meta.sequence);

        let mut tx = storage.pool.begin().await?;
        for event in events {
            process_event(event, &storage, &mut tx).await?;
        }
        tx.commit().await?;

        // Commit Kafka offsets for all buffered messages
    }
}
```

### 5. Handle edge cases

- **Empty blocks**: No events emitted; consumers don't need to track empty blocks
- **Consumer restart**: Kafka offsets only committed after full block processing, so partial blocks are re-delivered
- **Timeout**: Stale block detection warns if `is_last` never arrives (configurable via `BLOCK_STALE_TIMEOUT_SECS`)

## Files to Modify

1. `hermes-schema/proto/blockchain_metadata.proto` - Add fields
2. `hermes-pipeline/src/emit.rs` - Add SequencedEmitter
3. `hermes-pipeline/src/main.rs` - Use SequencedEmitter, track last event
4. `kg-indexer/src/main.rs` - Buffer and reorder logic
5. `kg-indexer/src/consumer.rs` - May need to expose metadata fields

## Migration

- New fields have default values (sequence=0, is_last=false)
- kg-indexer can fall back to current behavior if fields are missing
- Deploy hermes-pipeline first, then kg-indexer
