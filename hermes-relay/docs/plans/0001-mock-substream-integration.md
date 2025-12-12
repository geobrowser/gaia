# Mock Substream Integration for hermes-relay

## Status

Implemented

## Problem

We need to develop and test hermes transformers without relying on a real substream connection. This enables:
1. Running transformers with mock blockchain events
2. Deterministic integration testing
3. Parallel development without smart contract dependencies

## Solution

A simple iterator-based approach using `MockSource` and `mock_events` builders.

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

### MockSource

`MockSource` is a simple iterator over `BlockScopedData`. Tests iterate directly and call `process_block_scoped_data`:

```rust
use hermes_relay::source::{MockSource, mock_events};
use hermes_substream::pb::hermes::Actions;
use prost::Message;

// Create mock actions
let actions = Actions {
    actions: vec![
        mock_events::space_created([0x01; 16], [0xaa; 32]),
        mock_events::trust_extended_verified([0x01; 16], [0x02; 16]),
        mock_events::edit_published([0x01; 16], "QmYwAPJzv5CZsnA..."),
    ],
};

// Iterate and process
for block in MockSource::new(actions.encode_to_vec()).with_blocks(100, 110) {
    transformer.process_block_scoped_data(&block).await?;
}
```

### mock_events Module

Event builders mirror `mock-substream`'s event types, converting them to the `Action` chain format:

| mock-substream Event | mock_events Builder | Action Type |
|---------------------|---------------------|-------------|
| `SpaceCreated` (Personal) | `space_created(space_id, owner)` | `SPACE_REGISTERED` |
| `SpaceCreated` (DAO) | `space_created_dao(space_id, editors, members)` | `SPACE_REGISTERED` |
| `TrustExtended::Verified` | `trust_extended_verified(source, target)` | `SUBSPACE_ADDED` |
| `TrustExtended::Related` | `trust_extended_related(source, target)` | `SUBSPACE_ADDED` |
| `TrustExtended::Subtopic` | `trust_extended_subtopic(source, topic)` | `SUBSPACE_ADDED` |
| `EditPublished` | `edit_published(space_id, ipfs_hash)` | `EDITS_PUBLISHED` |

Trust extension types are encoded in the first 2 bytes of the `data` field:
- Verified: `[0x00, 0x00]`
- Related: `[0x00, 0x01]`
- Subtopic: `[0x00, 0x02]`

### Test Topology

`mock_events::test_topology::generate()` returns the same topology as `mock-substream::test_topology::generate()`:
- 18 space creations (11 canonical + 7 non-canonical)
- 19 trust extensions (14 explicit + 5 topic-based)
- 6 edit events

```rust
// Use full test topology
for block in MockSource::test_topology().with_blocks(100, 150) {
    transformer.process_block_scoped_data(&block).await?;
}
```

### Helper Functions

```rust
use hermes_relay::source::mock_events::{make_id, make_address};

// Create well-known IDs from a single byte
let space_id = make_id(0x01);      // [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0x01]
let owner = make_address(0xAA);    // [0,...,0,0xAA] (32 bytes)
```

## Usage Examples

### Basic Testing

```rust
#[tokio::test]
async fn test_handles_space_creation() {
    let sink = MySink::new();
    
    let actions = Actions {
        actions: vec![
            mock_events::space_created(make_id(0x01), make_address(0xAA)),
        ],
    };
    
    for block in MockSource::new(actions.encode_to_vec()).single_block(100) {
        sink.process_block_scoped_data(&block).await.unwrap();
    }
    
    // Assert sink state
}
```

### Full Topology Testing

```rust
#[tokio::test]
async fn test_processes_full_topology() {
    let sink = MySink::new();
    
    for block in MockSource::test_topology().with_blocks(100, 150) {
        sink.process_block_scoped_data(&block).await.unwrap();
    }
    
    // Verify 18 spaces, 19 trust edges, 6 edits processed
}
```

### Custom Event Sequences

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

for block in MockSource::new(actions.encode_to_vec()).with_blocks(100, 104) {
    sink.process_block_scoped_data(&block).await?;
}
```

## References

- [mock-substream](../../../mock-substream/) - Original mock event types
- [hermes-substream](../../../hermes-substream/) - Chain event format (`Action` struct)
