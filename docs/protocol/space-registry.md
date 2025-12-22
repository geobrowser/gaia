# Space Registry Protocol

The Space Registry is the central hub for managing spaces in the Geo protocol. It handles space registration, migration, and routes all inter-space communication through the `enter()` function.

## Core Concepts

### Space IDs

Each space has a unique 16-byte identifier (bytes16) that remains constant even if the space migrates to a new address.

**Generation Algorithm:**
```solidity
bytes16 spaceId = bytes16(keccak256(abi.encodePacked(
    'grc20.space',
    _account,
    _nonce,
    block.chainid
)));
```

- `_account`: The registering address
- `_nonce`: Auto-incrementing counter per registry
- `block.chainid`: Prevents cross-chain ID collisions

### Bi-directional Mapping

The registry maintains two mappings:
- `addressToSpaceId[address]` → bytes16
- `spaceIdToAddress[bytes16]` → address

## Registration Flow

When a contract calls `registerSpaceId(bytes32 _type, bytes memory _version)`:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Verify caller not already registered                    │
├─────────────────────────────────────────────────────────────┤
│ 2. Generate space ID from (address, nonce, chainid)        │
├─────────────────────────────────────────────────────────────┤
│ 3. Store bi-directional mappings                           │
├─────────────────────────────────────────────────────────────┤
│ 4. Emit SPACE_ID_REGISTERED                                │
│    from: bytes16(0)                                        │
│    to:   spaceId                                           │
│    topic: bytes32(bytes20(msg.sender))                     │
├─────────────────────────────────────────────────────────────┤
│ 5. If _type != 0, emit SPACE_TYPE_DECLARED                 │
│    from: spaceId                                           │
│    to:   spaceId                                           │
│    topic: _type                                            │
│    data:  _version                                         │
└─────────────────────────────────────────────────────────────┘
```

### Events Emitted

1. **SPACE_ID_REGISTERED** (always)
2. **SPACE_TYPE_DECLARED** (if `_type` is provided)

For a DAOSpace, `_type` = `keccak256(bytes("DAO_SPACE"))` and `_version` = `abi.encode("1.0.0")`.

## Migration Flow

Spaces can migrate to new addresses while keeping their ID. This is a two-step process:

### Step 1: Propose Migration

The current space address calls `proposeSpaceMigration(address _newAccount)`:

```solidity
function proposeSpaceMigration(address _newAccount) external {
    bytes16 spaceId = $.addressToSpaceId[msg.sender];
    if (spaceId == bytes16(0)) revert InvalidCaller();
    $.spaceIdToProposedAddress[spaceId] = _newAccount;
}
```

No events are emitted during proposal.

### Step 2: Accept Migration

The new address calls `acceptSpaceMigration(bytes16 _spaceId, bytes32 _type, bytes memory _version)`:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Verify caller is the proposed address                   │
├─────────────────────────────────────────────────────────────┤
│ 2. Verify caller not already registered                    │
├─────────────────────────────────────────────────────────────┤
│ 3. Update mappings:                                        │
│    - Clear old address → spaceId                           │
│    - Set new address ↔ spaceId                             │
│    - Clear proposal                                        │
├─────────────────────────────────────────────────────────────┤
│ 4. Emit SPACE_ID_MIGRATED                                  │
│    from: spaceId                                           │
│    to:   spaceId                                           │
│    topic: bytes32(bytes20(msg.sender)) [new address]       │
├─────────────────────────────────────────────────────────────┤
│ 5. If _type != 0, emit SPACE_TYPE_DECLARED                 │
│    from: spaceId                                           │
│    to:   spaceId                                           │
│    topic: _type                                            │
│    data:  _version                                         │
└─────────────────────────────────────────────────────────────┘
```

## Clearing a Space

A space can remove itself from the registry via `clearSpaceId()`:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Clear all mappings for this space                       │
├─────────────────────────────────────────────────────────────┤
│ 2. Emit SPACE_ID_CLEARED                                   │
│    from: spaceId                                           │
│    to:   bytes16(0)                                        │
│    topic: bytes32(bytes20(msg.sender))                     │
└─────────────────────────────────────────────────────────────┘
```

## The `enter()` Function

All inter-space communication goes through `enter()`:

```solidity
function enter(
    address _fromSpace,
    address _toSpace,
    bytes32 _action,
    bytes32 _topic,
    bytes calldata _data,
    bytes calldata _signature
) external;
```

### Execution Flow

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Resolve space IDs from addresses                        │
│    - Both must be registered or revert                     │
├─────────────────────────────────────────────────────────────┤
│ 2. If msg.sender != _fromSpace:                            │
│    - Call _fromSpace.verify() to validate signature        │
├─────────────────────────────────────────────────────────────┤
│ 3. Check if action is permissionless                       │
│                                                             │
│    IF PERMISSIONLESS:                                       │
│    ├─ Emit Action event directly                           │
│    └─ Done (no fetch/write)                                │
│                                                             │
│    IF NOT PERMISSIONLESS:                                   │
│    ├─ If msg.sender != _toSpace:                           │
│    │   └─ _topic = _toSpace.fetch(_action, _topic, _data)  │
│    ├─ Emit Action event                                    │
│    └─ If msg.sender != _toSpace:                           │
│        └─ _toSpace.write(_fromSpace, _action, _topic, _data)│
└─────────────────────────────────────────────────────────────┘
```

### The fetch() / write() Pattern

- **fetch()**: Called before emitting the event. Allows the target space to compute/modify the topic (e.g., set topic to proposalId for proposals).
- **write()**: Called after emitting the event. Allows the target space to process the action and update state.

This pattern enables the target space to:
1. Enrich the event with computed data (via fetch)
2. React to the action (via write)

### Permissionless Actions

Some actions skip fetch/write entirely:
- `UPVOTED`
- `DOWNVOTED`
- `UNVOTED`
- `COMMENTED`

These are configured during initialization and can be modified by the owner via `setPermissionlessAction()`.

When a permissionless action is configured, events are emitted:
- `PERMISSIONLESS_ACTION_ADDED` (from/to both `bytes16(0)`)
- `PERMISSIONLESS_ACTION_REMOVED` (from/to both `bytes16(0)`)

## Contract Details

| Property | Value |
|----------|-------|
| Name | `SPACE_REGISTRY` |
| Version | `1.0.0` |
| Upgradeable | Yes (UUPS) |
| Owner | Controls permissionless actions and upgrades |

## Storage Layout

```solidity
struct SpaceRegistryStorage {
    uint256 _spaceIdNonce;
    mapping(bytes16 => address) spaceIdToAddress;
    mapping(bytes16 => address) spaceIdToProposedAddress;
    mapping(address => bytes16) addressToSpaceId;
    mapping(bytes32 => bool) permissionlessActions;
}
```
