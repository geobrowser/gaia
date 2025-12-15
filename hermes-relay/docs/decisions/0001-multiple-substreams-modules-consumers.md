# 0001: Multiple Substreams Modules for Consumers

## Status

Accepted

## Context

The substreams protocol only supports consuming a **single output module** per stream in production mode. This is a limitation of the substreams RPC API itself - the `Request` message has a single `output_module` field.

Some transformers need events from multiple modules. For example, the spaces transformer needs `SpacesRegistered`, `SubspacesAdded`, and `SubspacesRemoved` events.

## Options Considered

### 1. Parallel Streams with Synchronization

Run multiple streams in parallel (one per module) and synchronize them by block number.

**Challenges:**
- Streams may progress at different rates, requiring buffering
- Undo signals need coordinated handling across all streams
- Each stream has its own cursor, complicating persistence
- Significant implementation complexity

### 2. Combined Modules in hermes-substream

Create new modules in hermes-substream that combine related events (e.g., `map_space_events` that includes registrations and subspace changes).

**Trade-offs:**
- Requires changes to hermes-substream for each new combination
- More efficient on the wire (only sends relevant events)
- Good when usage patterns are well understood

### 3. Use `map_actions` with Client-Side Filtering

Subscribe to the raw `Actions` module and filter client-side for the events needed.

**Trade-offs:**
- Simple implementation (single stream)
- Pulls more data than strictly necessary
- Filtering is cheap (just checking action type bytes)
- No synchronization complexity

## Decision

Use option 3: `map_actions` with client-side filtering.

For transformers that need a single event type, use the specific module (e.g., `HermesModule::EditsPublished`).

For transformers that need multiple event types, use `HermesModule::Actions` and filter client-side using the constants in the `actions` module:

```rust
use hermes_relay::{actions, HermesModule};

// Filter for space-related events
fn is_space_event(action_bytes: &[u8]) -> bool {
    actions::matches(action_bytes, &actions::SPACE_REGISTERED)
        || actions::matches(action_bytes, &actions::SUBSPACE_ADDED)
        || actions::matches(action_bytes, &actions::SUBSPACE_REMOVED)
}
```

## Consequences

- Simple implementation with no synchronization complexity
- Slightly more data transferred over the wire than necessary
- As usage patterns emerge, we can add combined modules to hermes-substream to optimize data transfer
