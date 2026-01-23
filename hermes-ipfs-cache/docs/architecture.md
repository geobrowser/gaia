# hermes-ipfs-cache Architecture

This document describes the implementation details of the IPFS cache service.

## Overview

The IPFS cache pre-fetches content for edit-related actions from Amp. It runs ahead of `hermes-pipeline`, storing resolved IPFS content in PostgreSQL so the edits pipeline doesn't block on network I/O.

```
┌─────────────────┐     ┌─────────────────────┐     ┌─────────────────┐
│       Amp       │────▶│  hermes-ipfs-cache  │────▶│   PostgreSQL    │
│ (actions log)   │     │                     │     │  (ipfs_cache)   │
└─────────────────┘     │  ┌───────────────┐  │     └─────────────────┘
                        │  │ IpfsCacheSink │  │              │
                        │  └───────┬───────┘  │              │
                        │          │          │              ▼
                        │  ┌───────▼───────┐  │     ┌─────────────────┐
                        │  │  Semaphore    │  │     │  hermes-pipeline  │
                        │  │ (20 permits)  │  │     │  (reads cache)  │
                        │  └───────┬───────┘  │     └─────────────────┘
                        │          │          │
                        │  ┌───────▼───────┐  │
                        │  │  IPFS Gateway │  │
                        │  └───────────────┘  │
                        └─────────────────────┘
```

## Components

### IpfsCacheSink

Processes action batches from the Amp stream.

```rust
pub struct IpfsCacheSink {
    cache: Arc<Mutex<Cache>>,
    ipfs: Arc<dyn IpfsFetcher>,
    semaphore: Arc<Semaphore>,
    pending: Arc<Mutex<PendingFetches>>,
}
```

- **cache**: PostgreSQL storage for resolved content
- **ipfs**: Client for fetching content from IPFS gateway
- **semaphore**: Limits concurrent IPFS fetches (default: 20)
- **pending**: Tracks in-flight fetches for cursor management

### PendingFetches

Tracks pending fetches per block to ensure correct cursor persistence.

```rust
struct PendingFetches {
    blocks: BTreeMap<u64, (String, usize)>,  // block -> (cursor, count)
}
```

Key operations:
- `add_block(block, cursor, count)`: Register a new block with its edit count
- `complete_one(block)`: Decrement count, return cursor if block complete and is minimum

### Cache

High-level interface to PostgreSQL storage.

```rust
pub struct Cache {
    storage: Storage,
}
```

Operations:
- `put(item)`: Insert with `ON CONFLICT DO NOTHING` (upsert)
- `load_cursor(id)`: Load persisted cursor for restart
- `persist_cursor(id, cursor, block)`: Save cursor position

## IPFS URI Validation

The protocol accepts arbitrary bytes for IPFS URIs, so validation is performed during action decoding.

### Validation Logic

The `extract_ipfs_uri` helper in `hermes-codec` extracts and validates IPFS URIs:

1. Searches for `ipfs://` pattern in the raw ABI-encoded event data
2. Extracts the CID (alphanumeric characters after the prefix)
3. Validates the CID format:

| CID Version | Prefix | Encoding | Length | Example |
|-------------|--------|----------|--------|---------|
| CIDv0 | `Qm` | Base58 | Exactly 46 chars | `QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG` |
| CIDv1 | `b` (e.g., `bafy`) | Base32 | ≥50 chars | `bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3okuez3djvxfzq` |

### Where Validation Happens

- **hermes-codec**: Extracts the `content_uri` field (empty if invalid)
- **hermes-ipfs-cache**: Skips events with empty `content_uri`
- **hermes-pipeline**: Uses `extract_ipfs_uri` helper when reading from raw `Actions` data

### Handling Invalid URIs

Events with invalid IPFS URIs (inline text content, malformed CIDs, etc.) are:
- **In extraction**: `content_uri` set to empty string, raw `data` preserved for debugging
- **In cache**: Skipped entirely (not cached)
- **In pipeline**: Filtered out via `filter_map`

## Data Flow

### Block Processing

1. `process_actions_block` receives a block of actions from Amp
2. Extract `content_uri` values from edit-related actions
3. Skip edits with empty `content_uri` (invalid IPFS URI)
4. Register block in `PendingFetches` with valid edit count
5. For each valid edit, spawn an async task:
   - Acquire semaphore permit (limits concurrency)
   - Fetch content from IPFS gateway using validated `content_uri`
   - Decode into `Edit` protobuf
   - Store in cache (success or error entry)
   - Mark fetch complete in `PendingFetches`
   - Persist cursor if appropriate

### Cursor Persistence

The cursor is persisted only when:
1. A block's fetch count reaches zero (all fetches complete)
2. That block is the minimum (oldest) in the pending map

This ensures correct restart behavior:

```
Block 100: 3 edits pending
Block 101: 2 edits pending
Block 102: 1 edit pending

# Block 102 completes first
complete_one(102) -> None  # Not minimum, don't persist

# Block 101 completes
complete_one(101) -> None  # Not minimum, don't persist

# Block 100 completes (2 remaining -> 1 remaining)
complete_one(100) -> None  # Not complete yet

# Block 100 final edit completes
complete_one(100) -> Some((100, cursor_100))  # Persist!
```

On restart, processing resumes from cursor 100. Blocks 101 and 102 will be reprocessed, but their content is already cached (upsert is a no-op).

## Error Handling

### IPFS Fetch Failures

When IPFS fetch or decode fails, we still cache an entry:

```rust
CacheItem {
    uri: "ipfs://...",
    json: None,           // No content
    is_errored: true,     // Mark as errored
    ...
}
```

This allows consumers to know the event exists but content is invalid, rather than retrying indefinitely.

### Duplicate URIs

The database uses `ON CONFLICT (uri) DO NOTHING`, so:
- Same URI in same block: First insert wins, subsequent are no-ops
- Same URI across blocks: Already cached, no duplicate work
- No explicit `has()` check needed, reducing DB round-trips

## Concurrency Model

### Cross-Block Parallelism

Blocks are processed without waiting for previous blocks to complete:

```
Time ─────────────────────────────────────────────▶

Block 100  ├──fetch A──┤├──fetch B──┤├──fetch C──┤
Block 101       ├──fetch D──┤├──fetch E──┤
Block 102            ├──fetch F──┤
```

This maximizes throughput while the cursor persistence logic ensures correctness.

### Semaphore-Based Rate Limiting

The semaphore limits concurrent IPFS requests to prevent overwhelming the gateway:

```rust
const MAX_CONCURRENT_FETCHES: usize = 20;

let permit = semaphore.acquire_owned().await;
// ... do IPFS fetch ...
drop(permit);  // Release for next fetch
```

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes | - | PostgreSQL connection string |
| `IPFS_GATEWAY_URL` | Yes | - | IPFS gateway base URL |
| `AMP_FLIGHT_URL` | Yes | - | Amp Flight SQL URL |
| `AMP_DATASET` | No | `geo/actions` | Amp dataset |
| `AMP_START_BLOCK` | No | 82655 | Starting block number |
| `AMP_END_BLOCK` | No | - | Ending block (unset = stream forever) |
| `AMP_ACTIONS_ADDRESS` | No | - | Actions contract address |

## Database Schema

```sql
-- IPFS content cache
CREATE TABLE ipfs_cache (
    uri TEXT PRIMARY KEY,
    data BYTEA,              -- Raw protobuf bytes (Edit message)
    block TEXT NOT NULL,
    space TEXT NOT NULL,     -- Space ID as UUID string
    is_errored BOOLEAN NOT NULL DEFAULT FALSE
);

-- Cursor persistence
CREATE TABLE meta (
    id TEXT PRIMARY KEY,
    cursor TEXT NOT NULL,
    block_number TEXT NOT NULL
);
```

The `data` column stores raw protobuf bytes rather than JSON. This allows:
- Direct protobuf decode without JSON serialization overhead
- Preservation of exact binary content from IPFS

## Testing

Unit tests cover the `PendingFetches` logic:

- Single block with single/multiple edits
- Multiple blocks completing in order
- Multiple blocks completing out of order
- Interleaved completions across blocks
- Edge cases (empty blocks, unknown blocks)

Run tests:
```bash
cargo test -p hermes-ipfs-cache
```
