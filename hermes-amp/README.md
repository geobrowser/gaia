# hermes-amp

This crate captures a minimal mapping of Hermes action logs into Amp's derived dataset model.

## What it provides

- `manifests/hermes-actions.json`: a derived dataset manifest with a single `actions` table.
- `SPACE_REGISTRY_ADDRESS_HEX`: the ZC16 Space Registry proxy address used to filter logs.

The `actions` table mirrors the anonymous `Action` event used in Hermes by mapping:

- `topic0` -> `from_id`
- `topic1` -> `to_id`
- `topic2` -> `action`
- `topic3` -> `topic`
- `data`   -> `data`

The raw topics are left-aligned for ZC16, so `from_id` and `to_id` are the first 16 bytes of the
32-byte topic values, and addresses are stored in the first 20 bytes. Consumers should slice as
needed.

## Configuration notes

- Update the manifest dependency `change_me/eth_mainnet@1.0.0` to the dataset you register in Amp.
- Update the table `network` if you use a different chain name than `zc16`.

## Action hashes

Action hash constants live in `hermes-codec/src/actions.rs` and can be used to filter `actions` by
`action` (topic2). Add more derived tables if you want pre-filtered views per action.

## Test consumers

### JSONL (batch only)

JSONL does **not** support streaming queries; it always returns the current batch and exits.

```bash
AMP_JSONL_URL=http://localhost:1603 \
AMP_SQL='SELECT * FROM "geo/actions".logs LIMIT 5' \
cargo run -p hermes-amp --bin amp_consumer
```

### Arrow Flight (streaming)

Use Arrow Flight for live streaming with `SETTINGS stream = true`.

```bash
AMP_FLIGHT_URL=http://localhost:1602 \
AMP_SQL='SELECT block_num, tx_hash, log_index, topic0, topic1, topic2, topic3 FROM "geo/actions".logs WHERE _block_num >= 82655 SETTINGS stream = true' \
cargo run -p hermes-amp --bin amp_flight_consumer
```

If you want to filter for Hermes Action logs only:

```bash
AMP_FLIGHT_URL=http://localhost:1602 \
AMP_SQL='SELECT block_num, tx_hash, log_index, topic0, topic1, topic2, topic3 FROM "geo/actions".logs WHERE address = evm_encode_hex('"'"'0xb01683b2f0d38d43fcd4d9aab980166988924132'"'"') AND _block_num >= 82655 SETTINGS stream = true' \
cargo run -p hermes-amp --bin amp_flight_consumer
```
