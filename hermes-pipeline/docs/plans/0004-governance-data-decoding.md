# 0004: Governance Data Decoding

## Status

Proposed

## Context

[0003-governance-events.md](./0003-governance-events.md) implements governance event support by converting raw actions to Hermes protobuf messages. However, the current design passes through the `data` field as raw bytes without decoding.

This means consumers receive opaque byte arrays that they must decode themselves, requiring knowledge of the on-chain encoding format. This defeats the purpose of the pipeline, which should transform raw blockchain data into consumer-friendly formats.

### Current State

The governance events currently emit raw `data` bytes:

```protobuf
message HermesProposalCreated {
  bytes space_id = 1;
  bytes proposal_id = 2;
  bytes data = 3;           // Raw bytes - consumer must decode
  BlockchainMetadata meta = 4;
}

message HermesProposalVoted {
  bytes voter_id = 1;
  bytes space_id = 2;
  bytes proposal_id = 3;
  bytes data = 4;           // Raw bytes - contains encoded vote choice
  BlockchainMetadata meta = 5;
}

message HermesProposalExecuted {
  bytes space_id = 1;
  bytes proposal_id = 2;
  bytes data = 3;           // Raw bytes - consumer must decode
  BlockchainMetadata meta = 4;
}
```

### Desired State

Governance events should have decoded, typed fields:

| Event | Raw `data` contains | Should decode to |
|-------|---------------------|------------------|
| `PROPOSAL_CREATED` | Proposal metadata | Title, description, voting period, quorum, etc. |
| `PROPOSAL_VOTED` | Vote choice | Enum: `YES`, `NO`, `ABSTAIN`, etc. |
| `PROPOSAL_EXECUTED` | Execution result | Success/failure status, result data |

## Decision

After implementing the initial governance events in [0003](./0003-governance-events.md), we will update the protobuf schemas and pipeline to decode the raw `data` bytes into typed fields.

This is deferred to a follow-up task because:
1. It requires understanding the exact on-chain encoding format
2. The initial implementation provides value even with raw bytes
3. Decoding logic can be added incrementally without breaking changes

## Work Required

1. **Research**: Document the exact encoding format for each governance action's `data` field
2. **Schema updates**: Replace `bytes data` with typed fields in the protobuf definitions
3. **Pipeline updates**: Add decoding logic in the conversion functions
4. **Testing**: Add test cases for various encoded values

## References

- [0003-governance-events.md](./0003-governance-events.md) - Initial governance implementation
- Space Registry contract ABI - Source of encoding format
