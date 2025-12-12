# Mock Substream Integration for hermes-relay

## Status

Implemented

## Problem

We need to develop and test hermes transformers without relying on a real substream connection. This enables:
1. Running transformers with mock blockchain events
2. Deterministic integration testing
3. Parallel development without smart contract dependencies

## Solution

A config-based approach using `StreamSource` enum to switch between mock and live data sources.

### Architecture

```
Production Path:
┌──────────────┐     ┌─────────────────┐     ┌────────────────┐
│  Blockchain  │────▶│ hermes-substream│────▶│  hermes-relay  │
└──────────────┘     └─────────────────┘     └────────────────┘

Mock Path:
┌──────────────┐     ┌────────────────┐
│  MockSource  │────▶│  hermes-relay  │
└──────────────┘     └────────────────┘
```

### StreamSource Config

The `StreamSource` enum provides explicit configuration for choosing between mock and live data:

```rust
use hermes_relay::{Sink, StreamSource, HermesModule};

// Development/testing: use mock data
sink.run(StreamSource::mock()).await?;

// Production: use live substream
let source = StreamSource::live(
    "https://substreams.example.com",
    HermesModule::Actions,
    0,      // start_block
    1000,   // end_block
);
sink.run(source).await?;
```

### MockSource

`MockSource` generates `BlockScopedData` containing mock events. When using `StreamSource::mock()`, all test topology events are delivered in a single block.

For more control over mock data, use `MockSource` directly:

```rust
use hermes_relay::source::{MockSource, mock_events};
use hermes_substream::pb::hermes::Actions;
use prost::Message;

// Create custom mock actions
let actions = Actions {
    actions: vec![
        mock_events::space_created([0x01; 16], [0xaa; 32]),
        mock_events::trust_extended_verified([0x01; 16], [0x02; 16]),
        mock_events::edit_published([0x01; 16], "QmYwAPJzv5CZsnA..."),
    ],
};

// Iterate and process directly
for block in MockSource::builder(actions.encode_to_vec()).single_block(100) {
    transformer.process_block_scoped_data(&block).await?;
}
```

### mock_events Module

Event builders create `Action` events in the chain format:

| Event Type | Builder | Action Type |
|------------|---------|-------------|
| Personal Space | `space_created(space_id, owner)` | `SPACE_REGISTERED` |
| DAO Space | `space_created_dao(space_id, editors, members)` | `SPACE_REGISTERED` |
| Verified Trust | `trust_extended_verified(source, target)` | `SUBSPACE_ADDED` |
| Related Trust | `trust_extended_related(source, target)` | `SUBSPACE_ADDED` |
| Subtopic Trust | `trust_extended_subtopic(source, topic)` | `SUBSPACE_ADDED` |
| Edit | `edit_published(space_id, ipfs_hash)` | `EDITS_PUBLISHED` |

Trust extension types are encoded in the first 2 bytes of the `data` field:
- Verified: `[0x00, 0x00]`
- Related: `[0x00, 0x01]`
- Subtopic: `[0x00, 0x02]`

### Test Topology

`MockSource::test_topology()` and `mock_events::test_topology::generate()` produce:
- 18 space creations (11 canonical + 7 non-canonical)
- 19 trust extensions (14 explicit + 5 topic-based)
- 6 edit events

### Helper Functions

```rust
use hermes_relay::source::mock_events::{make_id, make_address};

// Create well-known IDs from a single byte
let space_id = make_id(0x01);      // [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0x01]
let owner = make_address(0xAA);    // [0,...,0,0xAA] (32 bytes)
```

## Usage Examples

### Using StreamSource (Recommended)

```rust
use hermes_relay::{Sink, StreamSource};

struct MyTransformer { /* ... */ }

impl Sink for MyTransformer {
    type Error = anyhow::Error;
    
    async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
        // Process events...
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let transformer = MyTransformer::new();
    
    // All test topology events delivered in a single block
    transformer.run(StreamSource::mock()).await?;
    
    Ok(())
}
```

### Direct MockSource Usage (For Custom Scenarios)

```rust
use hermes_relay::source::mock_events::test_topology::*;

let actions = Actions {
    actions: vec![
        // Create root and child spaces
        space_created(ROOT_SPACE_ID, ROOT_OWNER),
        space_created(SPACE_A, USER_1),
        // Establish trust
        trust_extended_verified(ROOT_SPACE_ID, SPACE_A),
        // Publish edit
        edit_published(SPACE_A, "QmTestEdit"),
    ],
};

for block in MockSource::builder(actions.encode_to_vec()).single_block(100) {
    sink.process_block_scoped_data(&block).await?;
}
```

### Unit Testing

```rust
#[tokio::test]
async fn test_handles_space_creation() {
    let sink = MySink::new();
    
    let actions = Actions {
        actions: vec![
            mock_events::space_created(make_id(0x01), make_address(0xAA)),
        ],
    };
    
    for block in MockSource::builder(actions.encode_to_vec()).single_block(100) {
        sink.process_block_scoped_data(&block).await.unwrap();
    }
    
    // Assert sink state
}
```

## References

- [hermes-substream](../../../hermes-substream/) - Chain event format (`Action` struct)
- [stream](../../../stream/) - Substreams connection utilities
