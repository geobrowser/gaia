# ipfs

IPFS client for fetching GRC-20 edit content.

## Purpose

Provides an IPFS gateway client abstraction with trait-based dependency injection for production and test use. The crate handles CID normalization (strips `ipfs://` prefix) and returns raw bytes for downstream GRC-20 decoding and validation.

## Consumers

- [hermes-ipfs-cache](../hermes-ipfs-cache/) — IPFS content caching service
- [cache](../cache/) — Legacy cache system (sunset)

## Key Types

| Type | Description |
|------|-------------|
| `IpfsFetcher` | Trait for async IPFS fetch (`get_bytes` by CID) |
| `IpfsClient` | Production implementation — HTTP gateway client |
| `MockIpfsClient` | Test mock with in-memory CID → bytes store |
| `IpfsSource` | Config enum (`Mock` / `MockBytes` / `Live`) with `into_fetcher()` |
| `IpfsError` | Error types for IPFS operations (network, decode, not found, timeout) |

## Usage

```rust
use ipfs::IpfsSource;

// Production: connect to IPFS gateway
let fetcher = IpfsSource::live("https://ipfs.io/ipfs/").into_fetcher();
let bytes = fetcher.get_bytes("ipfs://QmTestCid1").await?;

// Testing: mock with GRC-20 v2 bytes (recommended)
let mut data = HashMap::new();
data.insert("QmTestCid1".to_string(), grc20_bytes);
let fetcher = IpfsSource::mock_bytes(data).into_fetcher();
```

## Design Notes

- **CID normalization** — Both `IpfsClient` and `MockIpfsClient` strip the `ipfs://` prefix, so callers can pass either `ipfs://QmFoo` or `QmFoo`.
- **GRC-20 validation** — Downstream consumers can validate or decode fetched bytes with `grc_20::decode_edit()`, which handles both GRC2 and GRC2Z formats.
- **Follows `StreamSource` pattern** — `IpfsSource` mirrors the `hermes-relay::StreamSource` pattern for consistent source configuration across the codebase.
