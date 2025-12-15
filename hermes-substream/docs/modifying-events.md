# Modifying Events

This guide covers how to add, modify, or remove events in hermes-substream.

## Adding a New Event

### 1. Compute the Action Hash

Each action type is identified by a keccak256 hash of its name string. Compute the hash:

```bash
# Using a temporary Rust project
cd /tmp
cat << 'EOF' > Cargo.toml
[package]
name = "compute_hash"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "compute_hash"
path = "main.rs"

[dependencies]
tiny-keccak = { version = "2.0", features = ["keccak"] }
EOF

cat << 'EOF' > main.rs
use tiny_keccak::{Hasher, Keccak};

fn main() {
    let action = "GOVERNANCE.MY_NEW_ACTION"; // Replace with your action name
    
    let mut hasher = Keccak::v256();
    let mut output = [0u8; 32];
    hasher.update(action.as_bytes());
    hasher.finalize(&mut output);
    
    let bytes: Vec<String> = output.iter().map(|b| format!("0x{:02x}", b)).collect();
    println!("const ACTION_MY_NEW_ACTION: [u8; 32] = [{}];", bytes.join(", "));
}
EOF

cargo run --quiet
```

### 2. Add Protobuf Message

Edit `proto/schema.proto` to add the new message type:

```protobuf
message MyNewEvent {
    bytes space_id = 1;        // 16 bytes
    bytes some_field = 2;      // Extract from topic field
    bytes data = 3;            // Raw data payload
}

message MyNewEventList {
    repeated MyNewEvent events = 1;
}
```

### 3. Add Action Hash Constant

Edit `src/lib.rs` to add the hash constant (from step 1):

```rust
const ACTION_MY_NEW_ACTION: [u8; 32] = [0x..., 0x..., ...];
```

### 4. Add Handler Function

Add a new handler function in `src/lib.rs`:

```rust
#[substreams::handlers::map]
fn map_my_new_events(block: eth::v2::Block) -> Result<MyNewEventList, substreams::errors::Error> {
    let events: Vec<MyNewEvent> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_MY_NEW_ACTION)
        .map(|action| MyNewEvent {
            space_id: action.from_id,
            some_field: action.topic[12..32].to_vec(), // Adjust based on field type
            data: action.data,
        })
        .collect();

    Ok(MyNewEventList { events })
}
```

**Field extraction from `topic`:**
- Address (20 bytes): `action.topic[12..32].to_vec()`
- Space ID (16 bytes): `action.topic[16..32].to_vec()`
- Full bytes32: `action.topic.clone()` or `action.topic.to_vec()`
- Object type + ID: `action.topic[0..4].to_vec()` and `action.topic[4..20].to_vec()`

### 5. Add Module to substreams.yaml

```yaml
modules:
  # ... existing modules ...
  
  - name: map_my_new_events
    kind: map
    initialBlock: 0
    inputs:
      - source: sf.ethereum.type.v2.Block
    output:
      type: proto:hermes.MyNewEventList
```

### 6. Regenerate and Build

```bash
# Regenerate protobuf bindings
substreams protogen

# Build WASM
cargo build --target wasm32-unknown-unknown --release

# Repack
substreams pack -o hermes-substream.spkg
```

### 7. Update Documentation

Update `README.md` to include the new module in the tables.

## Modifying an Existing Event

### Changing Field Extraction

If you need to change how fields are extracted from the Action event:

1. Update the handler in `src/lib.rs`
2. If field types change, update the protobuf message in `proto/schema.proto`
3. Regenerate and build (step 6 above)

### Changing the Action Hash

If the action name string changes:

1. Compute the new hash (step 1 above)
2. Update the constant in `src/lib.rs`
3. Rebuild (no protogen needed)

### Adding/Removing Fields

1. Update the protobuf message in `proto/schema.proto`
2. Update the handler mapping in `src/lib.rs`
3. Regenerate and build (step 6 above)

## Removing an Event

### 1. Remove Handler

Delete the handler function from `src/lib.rs`.

### 2. Remove Action Hash Constant

Delete the `const ACTION_...` line from `src/lib.rs` (if no longer used).

### 3. Remove Module from substreams.yaml

Delete the module entry from `substreams.yaml`.

### 4. Remove Protobuf Messages

Remove the message definitions from `proto/schema.proto` (if no longer used).

### 5. Regenerate and Build

```bash
substreams protogen
cargo build --target wasm32-unknown-unknown --release
substreams pack -o hermes-substream.spkg
```

### 6. Update Documentation

Remove the module from `README.md`.

## Updating the ABI

If the Space Registry contract ABI changes:

1. Replace `abis/space-registry.json` with the new ABI
2. If event structure changes, update handlers accordingly
3. Rebuild

Note: The current implementation doesn't use `use_contract!` macro - it manually parses the anonymous `Action` event. ABI changes only matter if:
- The `Action` event signature changes
- You want to decode additional events from the contract

## Testing Changes

### Local Testing

```bash
# Run against a specific block range
substreams run hermes-substream.spkg map_my_new_events -e <endpoint> --start-block 1000 --stop-block 1010
```

### Verifying Output

```bash
# Output as JSON for inspection
substreams run hermes-substream.spkg map_my_new_events -e <endpoint> --start-block 1000 --stop-block 1001 -o json
```

## Common Issues

### "module not found" Error

Ensure the module name in `substreams.yaml` matches exactly what you're requesting.

### Protobuf Mismatch

If you get type errors after changing protos, ensure you ran `substreams protogen` and the generated files in `src/pb/` are up to date.

### Build Failures

The substream must be built outside the Cargo workspace. Ensure you're running commands from the `hermes-substream/` directory, not the workspace root.
