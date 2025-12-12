# 0001: Cursor Persistence Strategy

## Status

Accepted

## Context

The IPFS cache needs to persist a cursor so it can resume from the correct position after a restart. The challenge is that IPFS fetches are spawned asynchronously and may complete out of order across blocks.

Consider this scenario:
1. Block 100 arrives with edits A, B, C - we spawn 3 fetch tasks
2. Block 101 arrives with edits D, E - we spawn 2 fetch tasks
3. Block 101's fetches complete first
4. If we persist cursor 101, then crash before block 100 completes...
5. On restart, we resume from 101 and never cache A, B, C

We need a persistence strategy that maintains correctness while preserving parallelism.

## Options Considered

### Option 1: Wait for all fetches per block

```rust
for edit in edits {
    handles.push(spawn(fetch(edit)));
}
join_all(handles).await;
persist_cursor(block);
```

**Pros:**
- Simple implementation
- Cursor always represents completed work

**Cons:**
- Blocks on slowest fetch per block
- Loses cross-block parallelism
- A slow IPFS fetch blocks all subsequent blocks

### Option 2: Persist cursor immediately, rely on consumer retry

The hermes-relay `Sink` trait calls `persist_cursor` after each `process_block_scoped_data` returns. We could persist immediately and rely on downstream consumers (edits transformer) to retry on cache miss.

**Pros:**
- Maximum throughput
- Simple implementation

**Cons:**
- On restart, may skip blocks whose fetches never completed
- Consumers must handle retries (adds complexity elsewhere)
- Violates principle of least surprise

### Option 3: Track minimum pending block

Track pending fetches per block. Only persist the cursor when:
1. A block fully completes (all fetches done)
2. That block is the minimum (oldest) pending block

```rust
struct PendingFetches {
    blocks: BTreeMap<u64, (String, usize)>,  // block -> (cursor, count)
}
```

**Pros:**
- Maintains full cross-block parallelism
- Cursor always represents safe restart point
- No reliance on consumer retry logic

**Cons:**
- More complex bookkeeping
- Cursor may lag behind actual progress

### Option 4: Persist every N completions

Persist periodically rather than on every completion:

```rust
if completion_count % 100 == 0 {
    persist_min_pending_cursor();
}
```

**Pros:**
- Reduces DB writes

**Cons:**
- Still needs minimum tracking logic
- Arbitrary batching parameter
- More restart reprocessing on crash

## Decision

We chose **Option 3: Track minimum pending block**.

The implementation:

1. When a block arrives with N edits, register it: `pending.add_block(block, cursor, N)`
2. Spawn all fetch tasks (full parallelism)
3. When a fetch completes, call `pending.complete_one(block)`
4. `complete_one` returns `Some((block, cursor))` only if:
   - Block's count reached zero (fully complete)
   - Block is the minimum in the BTreeMap (oldest pending)
5. If cursor returned, persist it to database

The `Sink` trait's `persist_cursor` method is a no-op - we handle persistence ourselves in the spawned tasks.

### Why not use the trait's persist_cursor?

The trait calls `persist_cursor` synchronously after `process_block_scoped_data` returns, but our fetches complete asynchronously later. By the time the trait calls persist, we don't know which fetches are done.

```rust
// In Sink trait (hermes-relay)
async fn run(...) {
    loop {
        self.process_block_scoped_data(&data).await?;  // Spawns tasks, returns immediately
        self.persist_cursor(cursor, block).await?;      // Called before tasks complete!
    }
}
```

By handling persistence in the spawned tasks themselves, we persist at the right moment.

## Consequences

### Positive

- **Correctness**: On restart, we never skip blocks with incomplete fetches
- **Parallelism**: Full cross-block parallelism maintained
- **Efficiency**: Persist once per block (when complete), not once per fetch
- **Self-contained**: No reliance on consumer retry logic

### Negative

- **Cursor lag**: Persisted cursor may lag behind actual progress if early blocks have slow fetches
- **Reprocessing**: On restart, may reprocess already-cached content (but upsert makes this cheap)
- **Complexity**: More bookkeeping than naive approaches

### Neutral

- **DB writes**: One write per completed block (acceptable for typical block rates)

## Example Walkthrough

```
Time ──────────────────────────────────────────────────▶

Block 100 (3 edits)  ├──A──┤    ├──B──┤        ├──C──┤
Block 101 (2 edits)       ├──D──┤  ├──E──┤
Block 102 (1 edit)             ├──F──┤

Events:
1. Block 100 arrives -> pending: {100: (c100, 3)}
2. Block 101 arrives -> pending: {100: (c100, 3), 101: (c101, 2)}
3. Block 102 arrives -> pending: {100: (c100, 3), 101: (c101, 2), 102: (c102, 1)}
4. A completes      -> pending: {100: (c100, 2), 101: (c101, 2), 102: (c102, 1)}
5. D completes      -> pending: {100: (c100, 2), 101: (c101, 1), 102: (c102, 1)}
6. F completes      -> pending: {100: (c100, 2), 101: (c101, 1)} // 102 done but not min
7. E completes      -> pending: {100: (c100, 2)}                  // 101 done but not min
8. B completes      -> pending: {100: (c100, 1)}
9. C completes      -> pending: {} -> PERSIST cursor 100

If crash after step 7:
- Restart from cursor 100
- Reprocess 100, 101, 102 (all already cached, upserts are no-ops)
- Minimal wasted work
```

## References

- `hermes-ipfs-cache/src/lib.rs` - `PendingFetches` implementation
- `hermes-ipfs-cache/docs/architecture.md` - Overall architecture
- `docs/hermes-architecture.md` - Hermes system design
