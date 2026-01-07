# Action Event Reference

All governance actions emit the `Action` event from the Space Registry contract.

## Event Signature

```solidity
event Action(
    bytes16 indexed fromSpaceId,
    bytes16 indexed toSpaceId,
    bytes32 indexed action,
    bytes32 indexed topic,
    bytes data
);
```

**Note:** This is an anonymous event with 4 indexed topics. The EVM allows max 4 indexed topics for anonymous events (vs 3 for named events).

## Action Constants

All action identifiers are `keccak256` hashes of their string names.

### Space Registry Actions

| Action | Hash | Emitted By |
|--------|------|------------|
| `SPACE_ID_REGISTERED` | `keccak256('GOVERNANCE.SPACE_ID_REGISTERED')` | SpaceRegistry |
| `SPACE_ID_CLEARED` | `keccak256('GOVERNANCE.SPACE_ID_CLEARED')` | SpaceRegistry |
| `SPACE_ID_MIGRATED` | `keccak256('GOVERNANCE.SPACE_ID_MIGRATED')` | SpaceRegistry |
| `SPACE_TYPE_DECLARED` | `keccak256('GOVERNANCE.SPACE_TYPE_DECLARED')` | SpaceRegistry |
| `PERMISSIONLESS_ACTION_ADDED` | `keccak256('GOVERNANCE.PERMISSIONLESS_ACTION_ADDED')` | SpaceRegistry |
| `PERMISSIONLESS_ACTION_REMOVED` | `keccak256('GOVERNANCE.PERMISSIONLESS_ACTION_REMOVED')` | SpaceRegistry |

### DAOSpace Governance Actions

| Action | Hash | Emitted By |
|--------|------|------------|
| `PROPOSAL_CREATED` | `keccak256('GOVERNANCE.PROPOSAL_CREATED')` | via enter() |
| `PROPOSAL_SETTINGS_SELECTED` | `keccak256('GOVERNANCE.PROPOSAL_SETTINGS_SELECTED')` | DAOSpace (ping) |
| `PROPOSAL_UPDATED` | `keccak256('GOVERNANCE.PROPOSAL_UPDATED')` | via enter() |
| `PROPOSAL_VOTED` | `keccak256('GOVERNANCE.PROPOSAL_VOTED')` | via enter() |
| `PROPOSAL_EXECUTED` | `keccak256('GOVERNANCE.PROPOSAL_EXECUTED')` | via enter() |

### DAOSpace Membership Actions

| Action | Hash | Emitted By |
|--------|------|------------|
| `EDITOR_ADDED` | `keccak256('GOVERNANCE.EDITOR_ADDED')` | DAOSpace (ping) |
| `EDITOR_REMOVED` | `keccak256('GOVERNANCE.EDITOR_REMOVED')` | DAOSpace (ping) |
| `MEMBER_ADDED` | `keccak256('GOVERNANCE.MEMBER_ADDED')` | DAOSpace (ping) |
| `MEMBER_REMOVED` | `keccak256('GOVERNANCE.MEMBER_REMOVED')` | DAOSpace (ping) |
| `MEMBERSHIP_REQUESTED` | `keccak256('GOVERNANCE.MEMBERSHIP_REQUESTED')` | via enter() |
| `SPACE_FAST_PATH_RESTRICTED` | `keccak256('GOVERNANCE.SPACE_FAST_PATH_RESTRICTED')` | via enter() |
| `SPACE_FAST_PATH_UNRESTRICTED` | `keccak256('GOVERNANCE.SPACE_FAST_PATH_UNRESTRICTED')` | DAOSpace (ping) |
| `SPACE_LEFT` | `keccak256('GOVERNANCE.SPACE_LEFT')` | via enter() |

### Content Actions

| Action | Hash | Emitted By |
|--------|------|------------|
| `EDITS_PUBLISHED` | `keccak256('GOVERNANCE.EDITS_PUBLISHED')` | DAOSpace (ping) |
| `FLAGGED` | `keccak256('GOVERNANCE.FLAGGED')` | DAOSpace (ping) |
| `UNFLAGGED` | `keccak256('GOVERNANCE.UNFLAGGED')` | DAOSpace (ping) |

### Permissionless Actions

| Action | Hash | Notes |
|--------|------|-------|
| `UPVOTED` | `keccak256('PERMISSIONLESS.UPVOTED')` | No fetch/write, direct emit |
| `DOWNVOTED` | `keccak256('PERMISSIONLESS.DOWNVOTED')` | No fetch/write, direct emit |
| `UNVOTED` | `keccak256('PERMISSIONLESS.UNVOTED')` | No fetch/write, direct emit |
| `COMMENTED` | `keccak256('PERMISSIONLESS.COMMENTED')` | No fetch/write, direct emit |

### Subspace Actions (Legacy)

| Action | Hash | Notes |
|--------|------|-------|
| `SUBSPACE_ADDED` | `keccak256('GOVERNANCE.SUBSPACE_ADDED')` | Legacy |
| `SUBSPACE_REMOVED` | `keccak256('GOVERNANCE.SUBSPACE_REMOVED')` | Legacy |

---

## Action Field Mappings

### SPACE_ID_REGISTERED

Emitted when a new space registers with the registry.

| Field | Value |
|-------|-------|
| fromSpaceId | `bytes16(0)` (null) |
| toSpaceId | The newly generated space ID |
| action | `SPACE_ID_REGISTERED` |
| topic | `bytes32(bytes20(msg.sender))` - the registering account address |
| data | `''` (empty) |

### SPACE_TYPE_DECLARED

Emitted alongside registration or migration when a type is specified.

| Field | Value |
|-------|-------|
| fromSpaceId | The space ID |
| toSpaceId | The space ID (same - ping pattern) |
| action | `SPACE_TYPE_DECLARED` |
| topic | `_type` (e.g., `keccak256(bytes("DAO_SPACE"))`) |
| data | `_version` (e.g., `abi.encode("1.0.0")`) |

### SPACE_ID_CLEARED

Emitted when a space is removed from the registry.

| Field | Value |
|-------|-------|
| fromSpaceId | The space ID being cleared |
| toSpaceId | `bytes16(0)` (null) |
| action | `SPACE_ID_CLEARED` |
| topic | `bytes32(bytes20(msg.sender))` - the account address |
| data | `''` (empty) |

### SPACE_ID_MIGRATED

Emitted when a space migrates to a new address.

| Field | Value |
|-------|-------|
| fromSpaceId | The space ID |
| toSpaceId | The space ID (same - ping pattern) |
| action | `SPACE_ID_MIGRATED` |
| topic | `bytes32(bytes20(msg.sender))` - the NEW account address |
| data | `''` (empty) |

### PERMISSIONLESS_ACTION_ADDED / REMOVED

Emitted when permissionless actions are configured.

| Field | Value |
|-------|-------|
| fromSpaceId | `bytes16(0)` (null) |
| toSpaceId | `bytes16(0)` (null) |
| action | `PERMISSIONLESS_ACTION_ADDED` or `PERMISSIONLESS_ACTION_REMOVED` |
| topic | The action being configured |
| data | `''` (empty) |

### PROPOSAL_CREATED

Emitted when a proposal is created via `enter()`.

| Field | Value |
|-------|-------|
| fromSpaceId | The proposer's space ID |
| toSpaceId | The DAO space ID |
| action | `PROPOSAL_CREATED` |
| topic | `bytes32(_proposalId)` - set by `fetch()` |
| data | `abi.encode(bytes16 proposalId, VotingMode votingMode, Action[] actions)` |

### PROPOSAL_SETTINGS_SELECTED

Emitted by the DAO (ping) after proposal creation or escalation.

| Field | Value |
|-------|-------|
| fromSpaceId | The DAO space ID |
| toSpaceId | The DAO space ID (same - ping pattern) |
| action | `PROPOSAL_SETTINGS_SELECTED` |
| topic | `bytes32(_proposalId)` |
| data | `abi.encode(startDate, lastDate, votingMode, quorum, supportThreshold)` |

### PROPOSAL_VOTED

Emitted when an editor votes via `enter()`.

| Field | Value |
|-------|-------|
| fromSpaceId | The voter's space ID |
| toSpaceId | The DAO space ID |
| action | `PROPOSAL_VOTED` |
| topic | `bytes32(_proposalId)` - set by `fetch()` |
| data | `abi.encode(bytes16 proposalId, VoteOption voteOption)` |

### PROPOSAL_UPDATED

Emitted when a proposal creator updates their proposal via `enter()`.

| Field | Value |
|-------|-------|
| fromSpaceId | The creator's space ID |
| toSpaceId | The DAO space ID |
| action | `PROPOSAL_UPDATED` |
| topic | `bytes32(_proposalId)` - set by `fetch()` |
| data | `abi.encode(bytes16 proposalId, VotingMode votingMode, Action[] actions)` |

### PROPOSAL_EXECUTED

Emitted when a proposal is executed via `enter()`.

| Field | Value |
|-------|-------|
| fromSpaceId | The executor's space ID (anyone) |
| toSpaceId | The DAO space ID |
| action | `PROPOSAL_EXECUTED` |
| topic | `bytes32(_proposalId)` - set by `fetch()` |
| data | `abi.encode(bytes16 proposalId)` |

### EDITOR_ADDED / REMOVED

Emitted by the DAO (ping) when editor membership changes.

| Field | Value |
|-------|-------|
| fromSpaceId | The DAO space ID |
| toSpaceId | The DAO space ID (same - ping pattern) |
| action | `EDITOR_ADDED` or `EDITOR_REMOVED` |
| topic | `bytes32(spaceId)` - target space ID (zeros for self) |
| data | `abi.encode(address)` - the editor's address |

### MEMBER_ADDED / REMOVED

Emitted by the DAO (ping) when member membership changes.

| Field | Value |
|-------|-------|
| fromSpaceId | The DAO space ID |
| toSpaceId | The DAO space ID (same - ping pattern) |
| action | `MEMBER_ADDED` or `MEMBER_REMOVED` |
| topic | `bytes32(spaceId)` - target space ID (zeros for self) |
| data | `abi.encode(address)` - the member's address |

### SPACE_FAST_PATH_RESTRICTED

Emitted via `enter()` when a space is restricted from the fast path.

| Field | Value |
|-------|-------|
| fromSpaceId | The restricting space's ID |
| toSpaceId | The DAO space ID |
| action | `SPACE_FAST_PATH_RESTRICTED` |
| topic | `bytes32(bytes20(_restrictedSpace))` - the restricted space's address |
| data | `''` (empty) |

### SPACE_FAST_PATH_UNRESTRICTED

Emitted by the DAO (ping) when a space is unrestricted from the fast path.

| Field | Value |
|-------|-------|
| fromSpaceId | The DAO space ID |
| toSpaceId | The DAO space ID (same - ping pattern) |
| action | `SPACE_FAST_PATH_UNRESTRICTED` |
| topic | `bytes32(bytes20(_unrestrictedSpace))` - the unrestricted space's address |
| data | `''` (empty) |

### MEMBERSHIP_REQUESTED

Emitted via `enter()` when a space requests to join a DAO as a member.

| Field | Value |
|-------|-------|
| fromSpaceId | The requesting space's ID |
| toSpaceId | The DAO space ID |
| action | `MEMBERSHIP_REQUESTED` |
| topic | `bytes32(0)` |
| data | `''` (empty) |

### SPACE_LEFT

Emitted via `enter()` when a member/editor leaves.

| Field | Value |
|-------|-------|
| fromSpaceId | The leaving account's space ID |
| toSpaceId | The DAO space ID |
| action | `SPACE_LEFT` |
| topic | Role being left (`MEMBER` or `EDITOR` hash) - set by `fetch()` |
| data | `abi.encode(bytes32 role)` |

### EDITS_PUBLISHED

Emitted by the DAO (ping) when content is published.

| Field | Value |
|-------|-------|
| fromSpaceId | The DAO space ID |
| toSpaceId | The DAO space ID (same - ping pattern) |
| action | `EDITS_PUBLISHED` |
| topic | User-defined topic |
| data | `abi.encode(bytes editsContentUri, bytes editsMetadata)` |

### FLAGGED / UNFLAGGED

Emitted by the DAO (ping) for content flagging.

| Field | Value |
|-------|-------|
| fromSpaceId | The DAO space ID |
| toSpaceId | The DAO space ID (same - ping pattern) |
| action | `FLAGGED` or `UNFLAGGED` |
| topic | User-defined topic |
| data | Flagged/unflagged ID (bytes) |
