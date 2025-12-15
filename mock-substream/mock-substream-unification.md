# Mock Substream Unification

## Problem

We have two parallel mock event generation systems:

1. **hermes-producer** (`hermes-producer/src/main.rs`): Generates protobuf events inline and publishes to Kafka
2. **atlas** (`atlas/src/mock_substream.rs`): Generates native Rust events in-memory for testing graph algorithms

Both are simulating the same thing: blockchain events from a substream. This duplication leads to:
- Inconsistent test data between systems
- Duplicated code for event generation
- Harder maintenance when event schemas change

## Architecture

The correct architecture has both `hermes-producer` and `Atlas` as **parallel consumers** of the same substream:

```
Blockchain → Substreams RPC → ┬→ hermes-producer → Kafka (edits, space events)
                              │
                              └→ Atlas → Kafka (canonical graph updates)
```

For testing/development, we need a mock substream that both can consume:

```
MockSubstream → ┬→ hermes-producer → Kafka (edits, space events)
                │
                └→ Atlas → Kafka (canonical graph updates)
```

## Solution

Create a `mock-substream` crate that provides:

1. **Unified event types** representing raw blockchain events
2. **Event generator** with both random and deterministic modes
3. **Conversion traits** so consumers can convert to their internal types

### Event Types

The mock substream should emit events that mirror what the real substream produces:

```rust
/// A block of events from the mock substream
pub struct MockBlock {
    pub number: u64,
    pub timestamp: u64,
    pub cursor: String,
    pub events: Vec<MockEvent>,
}

/// Events that can occur on-chain
pub enum MockEvent {
    SpaceCreated(SpaceCreated),
    TrustExtended(TrustExtended),
    EditPublished(EditPublished),
}

pub struct SpaceCreated {
    pub space_id: [u8; 16],
    pub topic_id: [u8; 16],
    pub space_type: SpaceType,
}

pub enum SpaceType {
    Personal { owner: [u8; 32] },
    Dao { initial_editors: Vec<[u8; 16]>, initial_members: Vec<[u8; 16]> },
}

pub struct TrustExtended {
    pub source_space_id: [u8; 16],
    pub extension: TrustExtension,
}

pub enum TrustExtension {
    Verified { target_space_id: [u8; 16] },
    Related { target_space_id: [u8; 16] },
    Subtopic { target_topic_id: [u8; 16] },
}

pub struct EditPublished {
    pub edit_id: [u8; 16],
    pub space_id: [u8; 16],
    pub author: [u8; 32],
    // ... GRC-20 operations
}
```

### Generator

```rust
pub struct MockSubstream {
    config: MockConfig,
    // internal state
}

pub struct MockConfig {
    pub deterministic: bool,
    pub include_edits: bool,
    pub num_spaces: usize,
    pub edits_per_space: usize,
}

impl MockSubstream {
    pub fn new(config: MockConfig) -> Self;
    
    /// Generate the next block of events
    pub fn next_block(&mut self) -> MockBlock;
    
    /// Generate a deterministic topology for testing
    pub fn deterministic_topology() -> Vec<MockBlock>;
}
```

### Deterministic Topology

Move the well-known test constants from atlas into the shared crate:

```rust
pub mod test_topology {
    pub const ROOT_SPACE_ID: [u8; 16] = make_id(0x01);
    pub const SPACE_A: [u8; 16] = make_id(0x0A);
    pub const SPACE_B: [u8; 16] = make_id(0x0B);
    // ... etc
    
    /// Generate a deterministic topology with:
    /// - 11 canonical spaces reachable from Root
    /// - 7 non-canonical spaces in isolated islands
    /// - Topic edges demonstrating resolution
    pub fn generate() -> Vec<MockBlock>;
}
```

### Consumer Integration

Each consumer converts mock events to their internal types:

**Atlas:**
```rust
impl From<&mock_substream::SpaceCreated> for atlas::events::SpaceCreated {
    fn from(event: &mock_substream::SpaceCreated) -> Self { ... }
}
```

**hermes-producer:**
```rust
impl From<&mock_substream::SpaceCreated> for hermes_schema::pb::space::HermesCreateSpace {
    fn from(event: &mock_substream::SpaceCreated) -> Self { ... }
}
```

## File Structure

```
mock-substream/
├── Cargo.toml
├── src/
│   ├── lib.rs           # Re-exports
│   ├── events.rs        # Event type definitions
│   ├── generator.rs     # MockSubstream implementation
│   └── test_topology.rs # Deterministic test topology
```

## Implementation Steps

1. Create `mock-substream` crate with event types
2. Implement `MockSubstream` generator
3. Move deterministic topology from atlas to mock-substream
4. Update atlas to use mock-substream (remove `mock_substream.rs`)
5. Update hermes-producer to use mock-substream (remove inline generation)

## Benefits

- **Single source of truth** for mock event generation
- **Consistent test data** across the entire system
- **Reusable deterministic topology** for integration tests
- **Type-safe conversions** between mock events and consumer types
- **Easier maintenance** when event schemas change

## Related Documents

- [Hermes Architecture](../docs/hermes-architecture.md) - System that consumes substream events
- [Atlas README](../atlas/README.md) - Canonical graph consumer
