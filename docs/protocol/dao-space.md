# DAOSpace Protocol

DAOSpace is a governance contract that manages proposals, voting, and membership for a decentralized autonomous organization.

## Core Concepts

### Roles

| Role | Hash | Capabilities |
|------|------|--------------|
| `EDITOR` | `keccak256('EDITOR')` | Vote, create fast/slow proposals, flag editors |
| `MEMBER` | `keccak256('MEMBER')` | Create slow path proposals only |
| `DAO` | `keccak256('DAO')` | Execute proposal actions (self-reference) |
| `SPACE_REGISTRY` | `keccak256('SPACE_REGISTRY')` | Call `write()` |

### Voting Modes

```solidity
enum VotingMode {
    Slow,  // 0 - Majority voting with duration
    Fast   // 1 - Threshold-based, immediate execution
}
```

### Vote Options

```solidity
enum VoteOption {
    None,    // 0 - Invalid/not voted
    Yes,     // 1
    No,      // 2
    Abstain  // 3
}
```

## Initialization Sequence

When `initialize()` is called, events fire in this order:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. SPACE_ID_REGISTERED                                     │
│    from: bytes16(0)                                        │
│    to:   [new space ID]                                    │
│    topic: bytes32(bytes20(DAOSpace address))               │
├─────────────────────────────────────────────────────────────┤
│ 2. SPACE_TYPE_DECLARED                                     │
│    from: spaceId                                           │
│    to:   spaceId                                           │
│    topic: keccak256(bytes("DAO_SPACE"))                    │
│    data:  abi.encode("1.0.0")                              │
├─────────────────────────────────────────────────────────────┤
│ 3. EDITS_PUBLISHED (if _publishEditsData provided)         │
│    from: spaceId                                           │
│    to:   spaceId                                           │
│    topic: ''                                               │
│    data:  _publishEditsData                                │
├─────────────────────────────────────────────────────────────┤
│ 4. EDITOR_ADDED × N (for each initial editor)              │
│    from: spaceId                                           │
│    to:   spaceId                                           │
│    topic: bytes32(bytes20(editor))                         │
├─────────────────────────────────────────────────────────────┤
│ 5. MEMBER_ADDED × M (for each initial member)              │
│    from: spaceId                                           │
│    to:   spaceId                                           │
│    topic: bytes32(bytes20(member))                         │
└─────────────────────────────────────────────────────────────┘
```

### Initialization Data Format

```solidity
abi.decode(_initializerData, (
    ISpaceRegistry _spaceRegistry,
    VotingSettings _votingSettings,
    address[] _initialEditors,
    address[] _initialMembers,
    bytes _publishEditsData
));
```

## The Ping Pattern

DAOSpace uses `_ping()` to emit self-referential events:

```solidity
function _ping(bytes32 _action, bytes32 _topic, bytes memory _data) internal {
    $.spaceRegistry.enter(
        address(this),  // _fromSpace
        address(this),  // _toSpace (same!)
        _action,
        _topic,
        _data,
        ''              // no signature needed
    );
}
```

**Result:** Both `fromSpaceId` and `toSpaceId` are the DAO's space ID.

Actions that use ping:
- `EDITOR_ADDED`, `EDITOR_REMOVED`
- `MEMBER_ADDED`, `MEMBER_REMOVED`
- `EDITOR_UNFLAGGED`
- `PROPOSAL_SETTINGS_USED`
- `EDITS_PUBLISHED`
- `FLAGGED`, `UNFLAGGED`

## Proposal Lifecycle

### Creating a Proposal

External space calls `enter()` with `PROPOSAL_CREATED`:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Registry calls DAOSpace.fetch()                         │
│    - Returns bytes32(proposalId) to set as topic           │
├─────────────────────────────────────────────────────────────┤
│ 2. Registry emits PROPOSAL_CREATED                         │
│    from: proposer's spaceId                                │
│    to:   DAO's spaceId                                     │
│    topic: bytes32(proposalId)                              │
├─────────────────────────────────────────────────────────────┤
│ 3. Registry calls DAOSpace.write()                         │
│    - Validates proposer role                               │
│    - Stores proposal                                       │
│    - Pings PROPOSAL_SETTINGS_USED                          │
└─────────────────────────────────────────────────────────────┘
```

#### Fast Path Requirements
- Proposer must be an EDITOR
- Proposer must not be flagged
- Exactly 1 action
- Action selector must be in allowed list:
  - `addMember.selector`
  - `removeMember.selector`
  - `publish.selector`
  - `flag.selector`
  - `unflag.selector`

#### Slow Path Requirements
- Proposer must be MEMBER or EDITOR
- Multiple actions allowed

### Voting

External space calls `enter()` with `PROPOSAL_VOTED`:

```
┌─────────────────────────────────────────────────────────────┐
│ Validation:                                                 │
│ - Voter must be EDITOR                                     │
│ - Proposal must exist and not be executed                  │
│ - Voting period must not have ended                        │
│ - Vote option must not be None                             │
├─────────────────────────────────────────────────────────────┤
│ Vote Replacement:                                           │
│ - Previous vote (if any) is subtracted from tally          │
│ - New vote is added to tally                               │
├─────────────────────────────────────────────────────────────┤
│ Fast Path Special Logic:                                    │
│                                                             │
│ IF vote is NO:                                              │
│ ├─ Escalate to Slow Path                                   │
│ ├─ Reset timing (new startDate, lastDate)                  │
│ ├─ Update threshold to percentage-based                    │
│ └─ Ping PROPOSAL_SETTINGS_USED                             │
│                                                             │
│ IF vote is YES and threshold met:                          │
│ └─ Execute immediately                                     │
└─────────────────────────────────────────────────────────────┘
```

### Execution

Anyone can call `enter()` with `PROPOSAL_EXECUTED` once criteria are met:

**Fast Path Execution Criteria:**
- `tally.yes > supportThreshold` (flat number)
- Can execute immediately upon reaching threshold

**Slow Path Execution Criteria:**
- Voting period ended (`block.timestamp > lastDate`)
- Quorum met (`abstain + yes + no >= quorum`)
- Support threshold met (percentage calculation):
  ```
  (RATIO_BASE - threshold) * yes > threshold * no
  ```

### Updating a Proposal

Only the original creator can update via `PROPOSAL_UPDATED`:
- Proposal must not be executed
- Resets voting (increments version, clears tally)
- Re-emits `PROPOSAL_SETTINGS_USED`

## Voting Settings

```solidity
struct VotingSettings {
    uint256 slowPathPercentageThreshold;  // Out of RATIO_BASE (10e6)
    uint256 fastPathFlatThreshold;        // Absolute number of YES votes
    uint256 quorum;                       // Minimum total votes for slow path
    uint256 duration;                     // Voting period length (min 1 minute)
}
```

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `MINIMUM_VOTING_DURATION` | 1 minute | Minimum voting period |
| `RATIO_BASE` | 10,000,000 | Denominator for percentage calculations |

## Editor/Member Management

### Adding an Editor

Via proposal execution → `addEditor()` → `_addEditor()`:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Verify not already an editor                            │
│ 2. Grant EDITOR role                                       │
│ 3. Increment totalEditors                                  │
│ 4. Ping EDITOR_ADDED                                       │
│    topic: bytes32(bytes20(newEditor))                      │
└─────────────────────────────────────────────────────────────┘
```

### Removing an Editor

Via proposal execution → `removeEditor()` → `_removeEditor()`:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Verify is an editor                                     │
│ 2. Validate removal won't break governance:                │
│    - quorum < totalEditors                                 │
│    - fastPathFlatThreshold < totalEditors                  │
│ 3. Revoke EDITOR role                                      │
│ 4. Decrement totalEditors                                  │
│ 5. Clear flagged status                                    │
│ 6. Ping EDITOR_REMOVED                                     │
│    topic: bytes32(bytes20(oldEditor))                      │
└─────────────────────────────────────────────────────────────┘
```

### Flagging an Editor

Via `enter()` with `EDITOR_FLAGGED` (editor-to-DAO):
- Flagged editors cannot create fast path proposals
- No ping event; the incoming `EDITOR_FLAGGED` action IS the event

### Unflagging an Editor

Via proposal execution → `unflagEditor()` → `_unflagEditor()`:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Verify is an editor                                     │
│ 2. Clear flagged status                                    │
│ 3. Ping EDITOR_UNFLAGGED                                   │
│    topic: bytes32(bytes20(unflaggedEditor))                │
└─────────────────────────────────────────────────────────────┘
```

### Leaving a Space

Via `enter()` with `SPACE_LEFT`:
- Member/editor can leave voluntarily
- Triggers `_removeMember()` or `_removeEditor()` based on role in data

## Content Publishing

Via proposal execution → `publish()`:

```
┌─────────────────────────────────────────────────────────────┐
│ Ping EDITS_PUBLISHED                                       │
│ topic: _topic (user-defined)                               │
│ data:  abi.encode(editsContentUri, editsMetadata)          │
└─────────────────────────────────────────────────────────────┘
```

## Contract Details

| Property | Value |
|----------|-------|
| Name | `DAO_SPACE` |
| Version | `1.0.0` |
| Upgradeable | No (but uses AccessControl) |

## Valid Fast Path Actions

By default, these selectors can use fast path:
- `addMember(address)` → `0xca6d56dc`
- `removeMember(address)` → `0x0b1ca49a`
- `publish(bytes32,bytes,bytes)` → `0x...`
- `flag(bytes32,bytes)` → `0x...`
- `unflag(bytes32,bytes)` → `0x...`
