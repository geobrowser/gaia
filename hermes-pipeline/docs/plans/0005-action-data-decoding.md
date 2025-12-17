# 0005: Action Data Decoding

## Status

Implemented

## Context

Actions emitted from the Space Registry contract include a `data` field that contains ABI-encoded payloads specific to each action type. The pipeline must decode this data and emit events with semantic field names.

## Action Data Layouts

### Actions with Empty Data (No Decoding Needed)

| Action | Topic Field | Data Field |
|--------|-------------|------------|
| Space Registered | `bytes32(bytes20(spaceAddress))` | Empty |
| Space Migrated | `bytes32(bytes20(newSpaceAddress))` | Empty |
| Editor Added | `bytes32(bytes20(_newEditor))` | Empty |
| Editor Removed | `bytes32(bytes20(_oldEditor))` | Empty |
| Member Added | `bytes32(bytes20(_newMember))` | Empty |
| Member Removed | `bytes32(bytes20(_oldMember))` | Empty |
| Editor Flagged | `bytes32(bytes20(_flaggedEditor))` | Empty |
| Editor Unflagged | `bytes32(bytes20(_unflaggedEditor))` | Empty |
| Space Left | `bytes32(keccak256('ROLE'))` | Empty |
| Subspace Added | `bytes32(spaceId)` | Empty |
| Subspace Removed | `bytes32(spaceId)` | Empty |
| Subspace Verified | `bytes32(spaceId)` | Empty |
| Subspace Related | `bytes32(spaceId)` | Empty |
| Subspace Topic Declared | `bytes32(bytes16(spaceId) \| bytes16(topicId) >> 128)` | Empty |

### Actions with Data (Decoding Required)

#### Governance Actions

| Action | Topic Field | Data Encoding | Decoded Fields |
|--------|-------------|---------------|----------------|
| Proposal Created | `bytes32(proposalId)` | `abi.encode(VotingMode, Action[])` | `voting_mode`, `action_index`, `action_count`, `action_type`, `action` |
| Proposal Voted | `bytes32(proposalId)` | `abi.encode(uint256(proposalId), VoteOption)` | `vote` |
| Proposal Executed | `bytes32(proposalId)` | `abi.encode(uint256(proposalId))` | (none - data redundant) |

**VotingMode:** `Fast (0)`, `Slow (1)`
**VoteOption:** `None (0)`, `Yes (1)`, `No (2)`, `Abstain (3)`
**Action struct:** `{address to, uint256 value, bytes data}`

**ProposalActionType** (decoded from calldata function selector):
- `ADD_MEMBER`, `REMOVE_MEMBER`, `ADD_EDITOR`, `REMOVE_EDITOR`
- `PUBLISH`, `FLAG`, `UNFLAG`, `UNFLAG_EDITOR`
- `UPDATE_VOTING_SETTINGS`, `PING`

Multi-action proposals emit one `HermesProposalCreated` event per action.

#### Content Actions

| Action | Topic Field | Data Encoding | Decoded Fields |
|--------|-------------|---------------|----------------|
| Topic Declared | `bytes32(topicId)` | `abi.encode(bytes(contentMetadata))` | `content_metadata` |
| Edits Published | `bytes32(topicId)` | `abi.encode(bytes(editsContentUri), bytes(editsMetadata))` | `content_uri`, `metadata` |
| Flagged | `bytes32(topicId)` | `abi.encode(bytes(flaggedUri))` | `uri` |
| Unflagged | `bytes32(topicId)` | `abi.encode(bytes(unflaggedUri))` | `uri` |

#### Permissionless Voting Actions

| Action | Topic Field | Data Encoding | Decoded Fields |
|--------|-------------|---------------|----------------|
| Upvoted | `bytes32(bytes4(objectType) << 224 \| bytes16(objectId) << 96)` | `abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))` | `version`, `group_id`, `space_pov` |
| Downvoted | (same) | (same) | (same) |
| Unvoted | (same) | (same) | (same) |

## Implementation

### ABI Decoding Module

The `decode` module in `hermes-pipeline` provides functions for each data type using the `alloy` crate.

### Pipeline Updates

Each pipeline's `convert_*` function decodes the raw data directly into the proto message fields.

## Dependencies

Added to `hermes-pipeline/Cargo.toml`:
```toml
alloy = { version = "0.9", features = ["sol-types"] }
```

## References

- [0004-governance-data-decoding.md](./0004-governance-data-decoding.md) - Original proposal
