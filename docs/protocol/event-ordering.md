# Event Ordering & Sequences

This document describes the exact order of events emitted by contract functions. This is critical for pipeline squashing logic and ensuring correct state reconstruction.

## Space Registry Events

### `registerSpaceId(bytes32 _type, bytes memory _version)`

**Always emits:**
1. `SPACE_ID_REGISTERED`

**Conditionally emits (if `_type != 0`):**
2. `SPACE_TYPE_DECLARED`

```
registerSpaceId(type, version)
    │
    ├─► SPACE_ID_REGISTERED
    │       from: bytes16(0)
    │       to:   newSpaceId
    │       topic: bytes32(bytes20(msg.sender))
    │       data: ''
    │
    └─► SPACE_TYPE_DECLARED (if type != 0)
            from: spaceId
            to:   spaceId
            topic: _type
            data: _version
```

### `clearSpaceId()`

**Emits:** 1 event

```
clearSpaceId()
    │
    └─► SPACE_ID_CLEARED
            from: spaceId
            to:   bytes16(0)
            topic: bytes32(bytes20(msg.sender))
            data: ''
```

### `acceptSpaceMigration(bytes16 _spaceId, bytes32 _type, bytes memory _version)`

**Always emits:**
1. `SPACE_ID_MIGRATED`

**Conditionally emits (if `_type != 0`):**
2. `SPACE_TYPE_DECLARED`

```
acceptSpaceMigration(spaceId, type, version)
    │
    ├─► SPACE_ID_MIGRATED
    │       from: spaceId
    │       to:   spaceId
    │       topic: bytes32(bytes20(msg.sender))  // NEW address
    │       data: ''
    │
    └─► SPACE_TYPE_DECLARED (if type != 0)
            from: spaceId
            to:   spaceId
            topic: _type
            data: _version
```

### `setPermissionlessAction(bytes32 _action, bool _isPermissionless)`

**Emits:** 1 event

```
setPermissionlessAction(action, true)
    │
    └─► PERMISSIONLESS_ACTION_ADDED
            from: bytes16(0)
            to:   bytes16(0)
            topic: _action
            data: ''

setPermissionlessAction(action, false)
    │
    └─► PERMISSIONLESS_ACTION_REMOVED
            from: bytes16(0)
            to:   bytes16(0)
            topic: _action
            data: ''
```

---

## DAOSpace Events

### `initialize(bytes calldata _initializerData)`

**Event sequence:**

```
initialize(data)
    │
    ├─1─► SPACE_ID_REGISTERED
    │         from: bytes16(0)
    │         to:   spaceId
    │         topic: bytes32(bytes20(DAOSpace))
    │
    ├─2─► SPACE_TYPE_DECLARED
    │         from: spaceId
    │         to:   spaceId
    │         topic: keccak256("DAO_SPACE")
    │         data: abi.encode("1.0.0")
    │
    ├─3─► EDITS_PUBLISHED (if publishEditsData.length != 0)
    │         from: spaceId
    │         to:   spaceId
    │         topic: ''
    │         data: publishEditsData
    │
    ├─4─► EDITOR_ADDED × N (for each initial editor, in order)
    │         from: spaceId
    │         to:   spaceId
    │         topic: bytes32(bytes20(editor[i]))
    │
    └─5─► MEMBER_ADDED × M (for each initial member, in order)
              from: spaceId
              to:   spaceId
              topic: bytes32(bytes20(member[j]))
```

**Critical for indexing:** Editors are added before members. The order within each array is preserved.

### Proposal Creation via `enter()` with `PROPOSAL_CREATED`

**Event sequence:**

```
enter(from, to, PROPOSAL_CREATED, topic, data, sig)
    │
    ├─1─► PROPOSAL_CREATED
    │         from: proposer's spaceId
    │         to:   DAO's spaceId
    │         topic: bytes32(proposalId)  // set by fetch()
    │         data: abi.encode(proposalId, votingMode, actions)
    │
    └─2─► PROPOSAL_SETTINGS_USED (ping)
              from: DAO's spaceId
              to:   DAO's spaceId
              topic: bytes32(proposalId)
              data: abi.encode(startDate, lastDate, votingMode, quorum, threshold)
```

**Squashing note:** These two events are always paired. Process them together.

### Voting via `enter()` with `PROPOSAL_VOTED`

**Normal vote (no escalation):**

```
enter(from, to, PROPOSAL_VOTED, topic, data, sig)
    │
    └─► PROPOSAL_VOTED
            from: voter's spaceId
            to:   DAO's spaceId
            topic: bytes32(proposalId)
            data: abi.encode(proposalId, voteOption)
```

**Fast path NO vote (escalation to slow path):**

```
enter(from, to, PROPOSAL_VOTED, topic, data, sig)  [voteOption = No on fast path]
    │
    ├─1─► PROPOSAL_VOTED
    │         from: voter's spaceId
    │         to:   DAO's spaceId
    │         topic: bytes32(proposalId)
    │         data: abi.encode(proposalId, No)
    │
    └─2─► PROPOSAL_SETTINGS_USED (ping)
              from: DAO's spaceId
              to:   DAO's spaceId
              topic: bytes32(proposalId)
              data: abi.encode(NEW startDate, NEW lastDate, Slow, quorum, NEW threshold)
```

**Fast path YES vote with immediate execution:**

```
enter(from, to, PROPOSAL_VOTED, topic, data, sig)  [voteOption = Yes, threshold met]
    │
    ├─► PROPOSAL_VOTED
    │       from: voter's spaceId
    │       to:   DAO's spaceId
    │       topic: bytes32(proposalId)
    │       data: abi.encode(proposalId, Yes)
    │
    └─► [Proposal actions execute - see below for emitted events]
```

### Proposal Update via `enter()` with `PROPOSAL_UPDATED`

**Event sequence:**

```
enter(from, to, PROPOSAL_UPDATED, topic, data, sig)
    │
    ├─1─► PROPOSAL_UPDATED
    │         from: creator's spaceId
    │         to:   DAO's spaceId
    │         topic: bytes32(proposalId)
    │         data: abi.encode(proposalId, votingMode, actions)
    │
    └─2─► PROPOSAL_SETTINGS_USED (ping)
              from: DAO's spaceId
              to:   DAO's spaceId
              topic: bytes32(proposalId)
              data: abi.encode(startDate, lastDate, votingMode, quorum, threshold)
```

### Proposal Execution via `enter()` with `PROPOSAL_EXECUTED`

**Event sequence:**

```
enter(from, to, PROPOSAL_EXECUTED, topic, data, sig)
    │
    ├─1─► PROPOSAL_EXECUTED
    │         from: executor's spaceId
    │         to:   DAO's spaceId
    │         topic: bytes32(proposalId)
    │         data: abi.encode(proposalId)
    │
    └─2+─► [Events from executed actions - variable]
```

The executed actions may emit additional events depending on what the proposal does.

### Editor/Member Management

**addEditor (via proposal execution):**
```
addEditor(address)
    │
    └─► EDITOR_ADDED (ping)
            from: spaceId
            to:   spaceId
            topic: bytes32(bytes20(newEditor))
            data: ''
```

**removeEditor (via proposal execution):**
```
removeEditor(address)
    │
    └─► EDITOR_REMOVED (ping)
            from: spaceId
            to:   spaceId
            topic: bytes32(bytes20(oldEditor))
            data: ''
```

**addMember (via proposal execution):**
```
addMember(address)
    │
    └─► MEMBER_ADDED (ping)
            from: spaceId
            to:   spaceId
            topic: bytes32(bytes20(newMember))
            data: ''
```

**removeMember (via proposal execution):**
```
removeMember(address)
    │
    └─► MEMBER_REMOVED (ping)
            from: spaceId
            to:   spaceId
            topic: bytes32(bytes20(oldMember))
            data: ''
```

### Editor Flagging

**EDITOR_FLAGGED (via enter):**
```
enter(from, to, EDITOR_FLAGGED, topic, data, sig)
    │
    └─► EDITOR_FLAGGED
            from: flagger's spaceId
            to:   DAO's spaceId
            topic: bytes32(bytes20(flaggedEditor))  // set by fetch()
            data: abi.encode(flaggedEditor)
```

**unflagEditor (via proposal execution):**
```
unflagEditor(address)
    │
    └─► EDITOR_UNFLAGGED (ping)
            from: spaceId
            to:   spaceId
            topic: bytes32(bytes20(unflaggedEditor))
            data: ''
```

### Leaving a Space

**SPACE_LEFT (via enter):**
```
enter(from, to, SPACE_LEFT, topic, data, sig)
    │
    ├─► SPACE_LEFT
    │       from: leaver's spaceId
    │       to:   DAO's spaceId
    │       topic: role (MEMBER or EDITOR hash)  // set by fetch()
    │       data: abi.encode(role)
    │
    └─► EDITOR_REMOVED or MEMBER_REMOVED (ping)
            from: spaceId
            to:   spaceId
            topic: bytes32(bytes20(leaver))
            data: ''
```

---

## Paired Events Summary

For pipeline squashing, these events should be processed together:

| Trigger | Events |
|---------|--------|
| `registerSpaceId()` | SPACE_ID_REGISTERED + SPACE_TYPE_DECLARED |
| `acceptSpaceMigration()` | SPACE_ID_MIGRATED + SPACE_TYPE_DECLARED |
| Proposal created | PROPOSAL_CREATED + PROPOSAL_SETTINGS_USED |
| Proposal updated | PROPOSAL_UPDATED + PROPOSAL_SETTINGS_USED |
| Fast→Slow escalation | PROPOSAL_VOTED + PROPOSAL_SETTINGS_USED |
| SPACE_LEFT | SPACE_LEFT + (EDITOR_REMOVED or MEMBER_REMOVED) |

---

## Event Ordering Within a Block

Within a single transaction, events are emitted in the order shown above. Across transactions in a block, events follow transaction ordering.

For cross-topic ordering in Kafka, use the `sequence` field from `BlockchainMetadata` to reconstruct the original order.
