# hermes-ipfs-cache

Pre-fetches IPFS content for `EditsPublished` events so `hermes-pipeline` doesn't block on network I/O when processing edits.

## Overview

This service runs ahead of `hermes-pipeline`, subscribing to `EditsPublished` events from hermes-substream and fetching IPFS content by CID. The resolved content is stored in PostgreSQL so downstream consumers can read directly from the cache.

```
                                    ┌─────────────────────┐
hermes-substream ──────────────────▶│  hermes-ipfs-cache  │
(EditsPublished)                    │                     │
                                    │  1. Fetch IPFS CID  │
                                    │  2. Decode content  │
                                    │  3. Store in cache  │
                                    └──────────┬──────────┘
                                               │
                                               ▼
                                    ┌─────────────────────┐
                                    │     PostgreSQL      │
                                    │    (ipfs_cache)     │
                                    └─────────────────────┘
```

## Features

- **Parallel fetching**: Configurable concurrency with semaphore-based limiting (default: 20 concurrent fetches)
- **Cross-block parallelism**: Processes multiple blocks simultaneously without blocking
- **Upsert storage**: Uses `ON CONFLICT DO NOTHING` to efficiently handle duplicate URIs
- **Correct cursor persistence**: Only persists cursor when a block fully completes and it's the minimum pending block
- **Error handling**: Caches errored entries so consumers know the event exists but content is invalid

## Configuration

Environment variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `USE_MOCK` | No | Set to "true" or "1" to use mock data (default: false) |
| `DATABASE_URL` | Live mode | PostgreSQL connection string |
| `IPFS_GATEWAY_URL` | Live mode | IPFS gateway URL (e.g., `https://ipfs.io/ipfs/`) |
| `SUBSTREAMS_ENDPOINT` | Live mode | Substreams endpoint URL (e.g., `geotest.substreams.pinax.network:443`) |
| `SUBSTREAMS_API_TOKEN` | Live mode | API token for substreams authentication |
| `SUBSTREAMS_START_BLOCK` | No | Block to start from (default: `82655`; set to `0` for a fresh/local chain) |
| `SUBSTREAMS_END_BLOCK` | No | Block to end at (default: u64::MAX = stream forever) |

## Database Schema

Requires the following tables:

```sql
CREATE TABLE ipfs_cache (
    uri TEXT PRIMARY KEY,
    json JSONB,
    block TEXT NOT NULL,
    space_id TEXT NOT NULL,
    is_errored BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE meta (
    id TEXT PRIMARY KEY,
    cursor TEXT NOT NULL,
    block_number TEXT NOT NULL
);
```

## Local Development

### Using docker-compose

The easiest way to run locally is with docker-compose from the `hermes/` directory:

```bash
cd hermes

# Set required env vars (or use .env file)
export SUBSTREAMS_ENDPOINT=https://...
export SUBSTREAMS_API_TOKEN=...

# Start postgres and the cache service
docker-compose up ipfs-cache-postgres hermes-ipfs-cache
```

This starts:
- **ipfs-cache-postgres**: PostgreSQL 16 on port 5433
- **hermes-ipfs-cache**: The cache service

**Note**: Database tables must be created separately before running.

### Running directly with cargo

If you prefer to run outside Docker:

```bash
# Start infrastructure (from repo root)
docker compose --profile infra up -d

# Run the service
DATABASE_URL=postgres://postgres:postgres@localhost:5433/ipfs_cache \
IPFS_GATEWAY_URL=https://ipfs.io/ipfs/ \
SUBSTREAMS_ENDPOINT=geotest.substreams.pinax.network:443 \
SUBSTREAMS_API_TOKEN=... \
cargo run -p hermes-ipfs-cache
```

### Using a .env file

The service loads environment variables from a `.env` file in the working directory. Create a `.env` file:

```bash
DATABASE_URL=postgres://postgres:postgres@localhost:5433/ipfs_cache
IPFS_GATEWAY_URL=https://ipfs.io/ipfs/
SUBSTREAMS_ENDPOINT=geotest.substreams.pinax.network:443
SUBSTREAMS_API_TOKEN=...
SUBSTREAMS_START_BLOCK=82655
RUST_LOG=info,hermes_ipfs_cache=debug
```

Then simply run:

```bash
cargo run -p hermes-ipfs-cache
```

## Usage as a Library

```rust
use hermes_ipfs_cache::{IpfsCacheSink, cache::{Cache, Storage}};
use hermes_relay::Sink;
use ipfs::IpfsClient;

let storage = Storage::new().await?;
let cache = Cache::new(storage);
let ipfs = IpfsClient::new(&ipfs_gateway);
let sink = IpfsCacheSink::new(cache, ipfs);

sink.run(
    &endpoint_url,
    IpfsCacheSink::module(),
    start_block,
    end_block,
).await?;
```

## Cursor Persistence

The cache tracks pending fetches per block and only persists the cursor when:

1. A block's fetches are all complete (count reaches zero)
2. That block is the minimum (oldest) pending block

This ensures that on restart, processing resumes from the oldest incomplete block, even if later blocks completed first. Duplicate fetches are handled efficiently by the upsert - already-cached content is simply skipped.

### Recovery

If the `meta.cursor` row for `id = 'hermes_ipfs_cache'` is corrupted or lost, the most recent safe cursor can be reconstructed from logs without reprocessing history from a much earlier checkpoint. Every persist emits an `info!` line with `event = "ipfs_cache.batch_end"` carrying `indexer_id`, `block_number`, and `cursor` — and that line fires *before* the database write, so it reaches stdout / Sentry / Axiom even when the persist itself fails (the failure surfaces separately as `event = "ipfs_cache.persist_cursor_failed"` with the same fields).

To recover:

1. In Axiom (or Sentry / your log store), filter for `event = "ipfs_cache.batch_end"` and `indexer_id = "hermes_ipfs_cache"`, sorted by time descending. Take the most recent entry.
2. Note its `cursor` and `block_number`.
3. Restore the `meta` row, e.g.:

   ```sql
   INSERT INTO meta (id, cursor, block_number)
   VALUES ('hermes_ipfs_cache', '<cursor from log>', '<block_number from log>')
   ON CONFLICT (id) DO UPDATE
     SET cursor = EXCLUDED.cursor,
         block_number = EXCLUDED.block_number;
   ```

4. Restart the service. It will log `Resuming from persisted cursor` and continue from there.

The recovered cursor lags real progress by however many in-flight blocks were behind the minimum at crash time — restart will reprocess those blocks, but the `ON CONFLICT DO NOTHING` upsert on `ipfs_cache` makes the replay cheap.

## Cache Miss Behavior

Downstream consumers (like `hermes-pipeline`) should retry on cache miss. Since this service runs ahead, misses indicate the cache is catching up. The retry should eventually succeed once the content is fetched and stored.

**Note**: For local development, `hermes-pipeline` includes a mock IPFS cache with pre-populated test edits, so this service is only needed for production.
