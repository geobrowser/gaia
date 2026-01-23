# hermes-ipfs-cache

Pre-fetches IPFS content for edit-related actions so `hermes-pipeline` doesn't block on network I/O when processing edits.

## Overview

This service runs ahead of `hermes-pipeline`, subscribing to Amp action logs and fetching IPFS content by CID. The resolved content is stored in PostgreSQL so downstream consumers can read directly from the cache.

```
                                    ┌─────────────────────┐
Amp actions log ───────────────────▶│  hermes-ipfs-cache  │
                                    │                     │
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
| `AMP_FLIGHT_URL` | Live mode | Amp Flight SQL URL (default: `http://localhost:1602`) |
| `AMP_DATASET` | No | Amp dataset (default: `geo/actions`) |
| `AMP_START_BLOCK` | No | Block to start from (default: `82655`) |
| `AMP_END_BLOCK` | No | Block to end at (default: stream forever) |
| `AMP_ACTIONS_ADDRESS` | No | Actions contract address (default: `SPACE_REGISTRY_ADDRESS_HEX`) |
| `AMP_RECONNECT_DELAY_SECS` | No | Reconnect delay (default: `2`) |

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
export AMP_FLIGHT_URL=http://localhost:1602

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
# Start postgres (via docker-compose or locally)
cd hermes && docker-compose up ipfs-cache-postgres

# Run the service
DATABASE_URL=postgres://postgres:postgres@localhost:5433/ipfs_cache \
IPFS_GATEWAY_URL=https://ipfs.io/ipfs/ \
AMP_FLIGHT_URL=http://localhost:1602 \
cargo run -p hermes-ipfs-cache
```

### Using a .env file

The service loads environment variables from a `.env` file in the working directory. Create a `.env` file:

```bash
DATABASE_URL=postgres://postgres:postgres@localhost:5433/ipfs_cache
IPFS_GATEWAY_URL=https://ipfs.io/ipfs/
AMP_FLIGHT_URL=http://localhost:1602
AMP_START_BLOCK=88109
RUST_LOG=info,hermes_ipfs_cache=debug
```

Then simply run:

```bash
cargo run -p hermes-ipfs-cache
```

## Usage as a Library

```rust
use hermes_amp::{AmpStreamConfig, stream_actions};
use hermes_ipfs_cache::{IpfsCacheSink, cache::{Cache, Storage}};
use ipfs::IpfsClient;

let storage = Storage::new().await?;
let cache = Cache::new(storage);
let ipfs = IpfsClient::new(&ipfs_gateway);
let sink = IpfsCacheSink::new(cache, ipfs);
let config = AmpStreamConfig::from_env();

stream_actions(config, |block| async {
    sink.process_actions_block(&block.actions, block.block_num, block.timestamp_secs, block.cursor)
        .await
        .map_err(anyhow::Error::from)
}).await?;
```

## Cursor Persistence

The cache tracks pending fetches per block and only persists the cursor when:

1. A block's fetches are all complete (count reaches zero)
2. That block is the minimum (oldest) pending block

This ensures that on restart, processing resumes from the oldest incomplete block, even if later blocks completed first. Duplicate fetches are handled efficiently by the upsert - already-cached content is simply skipped.

## Cache Miss Behavior

Downstream consumers (like `hermes-pipeline`) should retry on cache miss. Since this service runs ahead, misses indicate the cache is catching up. The retry should eventually succeed once the content is fetched and stored.

**Note**: For local development, `hermes-pipeline` includes a mock IPFS cache with pre-populated test edits, so this service is only needed for production.
