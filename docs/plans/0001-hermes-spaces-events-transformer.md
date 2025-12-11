# 0001: Hermes Spaces Events Transformer

## Status

Proposed

## Context

The Hermes architecture (see `docs/hermes-architecture.md`) defines independent transformer binaries that:
- Connect to the blockchain data source via `hermes-relay`
- Filter for specific event types
- Transform and emit to Kafka topics
- Maintain independent cursors for restart/replay

Currently, `hermes-processor` is a proof-of-concept that consumes mock events from `mock-substream` and writes all event types (spaces, trust extensions, edits) to Kafka. This was useful for validating the Kafka integration, but doesn't follow the architecture's separation of concerns.

We need to implement a **spaces transformer** that handles space-related events (excluding edits) and writes them to Kafka using the real substream infrastructure via `hermes-relay`.

## Decision

Create a new binary crate `hermes-spaces` that:

1. **Implements `hermes_relay::Sink`** to consume events from `hermes-substream`
2. **Subscribes to `HermesModule::Actions`** and filters client-side for:
   - `SPACE_REGISTERED` - new space registrations
   - `SUBSPACE_ADDED` - trust extensions (verified/related/subtopic)
   - `SUBSPACE_REMOVED` - trust revocations
3. **Excludes edit events** (`EDITS_PUBLISHED`) - these will be handled by a separate `hermes-edits` transformer
4. **Transforms events** into Hermes protobuf messages:
   - `HermesCreateSpace` for space registrations
   - `HermesSpaceTrustExtension` for trust changes
5. **Writes to Kafka topics**:
   - `space.creations` for new spaces
   - `space.trust.extensions` for trust changes

### Why Filter Client-Side?

The substreams protocol only supports consuming a single output module per stream in production mode. Since the spaces transformer needs multiple event types (space registrations AND subspace changes), we must:
- Subscribe to `map_actions` (all raw actions)
- Filter client-side using action type constants from `hermes_relay::actions`

See `hermes-relay/docs/decisions/0001-multiple-substreams-modules-consumers.md` for details.

## Implementation Plan

### Phase 1: Crate Setup

1. Create `hermes-spaces/` directory structure:
   ```
   hermes-spaces/
   ├── src/
   │   ├── main.rs
   │   ├── transformer.rs    # Sink implementation
   │   ├── conversion.rs     # Event -> Proto conversion
   │   └── kafka.rs          # Kafka producer utilities
   ├── Cargo.toml
   ├── Dockerfile
   └── README.md
   ```

2. Add to workspace `Cargo.toml`:
   ```toml
   members = [
       # ...existing members...
       "hermes-spaces",
   ]
   ```

3. Define dependencies in `hermes-spaces/Cargo.toml`:
   ```toml
   [dependencies]
   hermes-relay = { path = "../hermes-relay" }
   hermes-schema = { path = "../hermes-schema" }
   rdkafka = { version = "0.36", features = ["cmake-build", "zstd", "ssl"] }
   prost = "0.13.5"
   tokio = { version = "1", features = ["full"] }
   anyhow = "1"
   hex = "0.4"
   ```

### Phase 2: Transformer Implementation

1. **Define `SpacesTransformer` struct** (`transformer.rs`):
   ```rust
   pub struct SpacesTransformer {
       producer: BaseProducer,
       // Future: cursor storage for persistence
   }
   ```

2. **Implement `hermes_relay::Sink` trait**:
   - `process_block_scoped_data`: Decode actions, filter, transform, send to Kafka
   - `process_block_undo_signal`: Handle chain reorgs (delete affected data)
   - `persist_cursor` / `load_persisted_cursor`: Initially no-ops, later add DB persistence

3. **Action filtering logic**:
   ```rust
   use hermes_relay::actions;
   
   fn is_space_event(action_bytes: &[u8]) -> bool {
       actions::matches(action_bytes, &actions::SPACE_REGISTERED)
           || actions::matches(action_bytes, &actions::SUBSPACE_ADDED)
           || actions::matches(action_bytes, &actions::SUBSPACE_REMOVED)
   }
   ```

### Phase 3: Event Conversion

1. **Move/adapt conversion logic from `hermes-processor`** (`conversion.rs`):
   - `convert_block_metadata` - BlockchainMetadata from block clock
   - `convert_space_registered` - Action -> HermesCreateSpace
   - `convert_subspace_added` - Action -> HermesSpaceTrustExtension (Verified)
   - `convert_subspace_removed` - Action -> HermesSpaceTrustExtension (removal)

2. **Decode raw Action bytes**:
   - Parse `Action` protobuf from `hermes-substream`
   - Extract space_id, topic_id, payload from action fields
   - Map to Hermes proto types

### Phase 4: Kafka Integration

1. **Reuse Kafka setup from `hermes-processor`** (`kafka.rs`):
   - `create_producer()` with SASL/SSL support for managed Kafka
   - Plaintext fallback for local development

2. **Send functions**:
   - `send_space_creation(producer, space)` -> `space.creations` topic
   - `send_trust_extension(producer, extension)` -> `space.trust.extensions` topic

### Phase 5: Main Entrypoint

1. **Configuration via environment variables**:
   ```rust
   // Required
   SUBSTREAMS_ENDPOINT  // e.g., "https://mainnet.eth.streamingfast.io"
   KAFKA_BROKER         // e.g., "localhost:9092"
   
   // Optional
   SUBSTREAMS_API_TOKEN // Auth token for substreams
   KAFKA_USERNAME       // SASL username (for managed Kafka)
   KAFKA_PASSWORD       // SASL password
   KAFKA_SSL_CA_PEM     // Custom CA cert
   START_BLOCK          // Block to start from (default: 0)
   END_BLOCK            // Block to stop at (default: live streaming)
   ```

2. **Main function**:
   ```rust
   #[tokio::main]
   async fn main() -> Result<()> {
       let transformer = SpacesTransformer::new(/* config */)?;
       transformer.run(
           &endpoint_url,
           HermesModule::Actions,
           start_block,
           end_block,
       ).await
   }
   ```

### Phase 6: Testing & Deployment

1. **Unit tests**:
   - Conversion functions
   - Action filtering

2. **Integration tests**:
   - Use `mock-substream` for deterministic event generation
   - Verify Kafka messages match expected protos

3. **Docker/K8s deployment**:
   - Dockerfile similar to `hermes-processor`
   - K8s manifests in `hermes/k8s/`

## File Changes Summary

| File | Action | Description |
|------|--------|-------------|
| `Cargo.toml` | Modify | Add `hermes-spaces` to workspace members |
| `hermes-spaces/Cargo.toml` | Create | Crate manifest with dependencies |
| `hermes-spaces/src/main.rs` | Create | Entrypoint, config parsing, run loop |
| `hermes-spaces/src/transformer.rs` | Create | `Sink` implementation |
| `hermes-spaces/src/conversion.rs` | Create | Event -> Proto conversion |
| `hermes-spaces/src/kafka.rs` | Create | Kafka producer utilities |
| `hermes-spaces/Dockerfile` | Create | Container build |
| `hermes-spaces/README.md` | Create | Documentation |

## Consequences

### Positive

- **Follows architecture**: Independent transformer with its own cursor
- **Reuses infrastructure**: `hermes-relay` for substream connection, existing Kafka patterns
- **Isolated failures**: Spaces transformer can crash without affecting edits processing
- **Independent replay**: Can reprocess spaces from any block without replaying edits

### Negative

- **Client-side filtering overhead**: Processing all actions to filter for space events
- **No cursor persistence initially**: Will restart from beginning on crash (add later)

### Neutral

- **Code duplication**: Some Kafka/conversion code duplicated from `hermes-processor`
  - Could extract shared utilities to `hermes-schema` or new `hermes-common` crate later

## Future Work

1. **Cursor persistence**: Add PostgreSQL or Redis storage for cursor
2. **hermes-edits transformer**: Similar pattern for edit events with IPFS resolution
3. **Shared Kafka utilities**: Extract common code if pattern repeats
4. **Metrics/observability**: Add Prometheus metrics, structured logging
