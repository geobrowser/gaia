# Data Encoding Reference

This document describes the ABI encoding formats for all action data payloads.

## Type Definitions

### VotingMode

```solidity
enum VotingMode {
    Slow,  // 0
    Fast   // 1
}
```

Encoded as `uint8`.

### VoteOption

```solidity
enum VoteOption {
    None,    // 0 - Invalid
    Abstain, // 1
    Yes,     // 2
    No       // 3
}
```

Encoded as `uint8`.

### Action (Proposal Action)

```solidity
struct Action {
    address to;      // Target contract
    uint256 value;   // ETH value to send
    bytes data;      // Calldata
}
```

### VotingSettings

```solidity
struct VotingSettings {
    uint256 slowPathPercentageThreshold;  // Out of 10,000,000 (RATIO_BASE)
    uint256 fastPathFlatThreshold;        // Absolute number
    uint256 quorum;                       // Minimum votes
    uint256 duration;                     // Seconds
}
```

### ProposalParameters

```solidity
struct ProposalParameters {
    uint256 startDate;         // Block timestamp
    uint256 lastDate;          // Block timestamp
    VotingMode votingMode;     // Slow (0) or Fast (1)
    uint256 quorum;            // Minimum total votes
    uint256 supportThreshold;  // Flat (fast) or percentage (slow)
}
```

---

## Action Data Payloads

### PROPOSAL_CREATED / PROPOSAL_UPDATED

```solidity
abi.encode(
    bytes16 proposalId,
    VotingMode votingMode,
    Action[] actions
)
```

**Decoding in Rust:**
```rust
// proposalId: first 16 bytes (padded to 32)
// votingMode: uint8 at offset 32
// actions: dynamic array starting at offset 64
```

**Example hex breakdown:**
```
0x
0000000000000000000000000000000011111111111111111111111111111111  // proposalId (bytes16, left-padded)
0000000000000000000000000000000000000000000000000000000000000001  // votingMode = Fast
0000000000000000000000000000000000000000000000000000000000000060  // offset to actions array
0000000000000000000000000000000000000000000000000000000000000001  // actions.length = 1
... // Action struct data
```

### PROPOSAL_VOTED

```solidity
abi.encode(
    bytes16 proposalId,
    VoteOption voteOption
)
```

**Example:**
```
0x
0000000000000000000000000000000011111111111111111111111111111111  // proposalId
0000000000000000000000000000000000000000000000000000000000000002  // voteOption = Yes
```

### PROPOSAL_EXECUTED

```solidity
abi.encode(bytes16 proposalId)
```

**Example:**
```
0x
0000000000000000000000000000000011111111111111111111111111111111  // proposalId
```

### PROPOSAL_SETTINGS_SELECTED

```solidity
abi.encode(
    uint256 startDate,
    uint256 lastDate,
    VotingMode votingMode,
    uint256 quorum,
    uint256 supportThreshold
)
```

**Example:**
```
0x
0000000000000000000000000000000000000000000000000000000067890123  // startDate (timestamp)
0000000000000000000000000000000000000000000000000000000067890183  // lastDate (startDate + duration)
0000000000000000000000000000000000000000000000000000000000000000  // votingMode = Slow
0000000000000000000000000000000000000000000000000000000000000003  // quorum = 3
0000000000000000000000000000000000000000000000000000000000989680  // supportThreshold = 10000000 (100%)
```

### SPACE_LEFT

```solidity
abi.encode(bytes32 role)
```

**Role values:**
- MEMBER: `keccak256('MEMBER')` = `0x829b824e2329e205435d941c9f13baf578548505283d29261236d8e6596d4636`
- EDITOR: `keccak256('EDITOR')` = `0x21d1167972f621f75904fb065136bc8b53c7ba1c60ccd3a7f8f71e47c6b4e977`

### EDITOR_FLAGGED

```solidity
abi.encode(address flaggedEditor)
```

**Example:**
```
0x
000000000000000000000000aabbccdd11223344556677889900aabbccdd1122  // flaggedEditor address
```

### EDITS_PUBLISHED

```solidity
abi.encode(
    bytes editsContentUri,
    bytes editsMetadata
)
```

**Dynamic encoding:** Both are dynamic byte arrays with offset pointers.

### FLAGGED / UNFLAGGED

```solidity
// data is the flagged/unflagged ID (raw bytes, not abi.encode wrapped)
bytes flaggedId
```

---

## Space Type Encoding

### SPACE_TYPE_DECLARED

**topic:** `keccak256(bytes(contractName))`

For DAOSpace: `keccak256(bytes("DAO_SPACE"))` = `0x...`

**data:** `abi.encode(string version)`

For DAOSpace v1.0.0: `abi.encode("1.0.0")`

```
0x
0000000000000000000000000000000000000000000000000000000000000020  // offset
0000000000000000000000000000000000000000000000000000000000000005  // length = 5
312e302e30000000000000000000000000000000000000000000000000000000  // "1.0.0" padded
```

---

## Address Encoding in Topics

Several actions encode addresses in the `topic` field:

```solidity
bytes32(bytes20(address))
```

This left-pads the 20-byte address into a 32-byte value:

```
0x000000000000000000000000aabbccdd11223344556677889900aabbccdd1122
                          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                          20-byte address, right-aligned
```

**Actions using this pattern:**
- SPACE_ID_REGISTERED (topic = account)
- SPACE_ID_CLEARED (topic = account)
- SPACE_ID_MIGRATED (topic = new account)
- EDITOR_ADDED/REMOVED (topic = editor)
- MEMBER_ADDED/REMOVED (topic = member)
- EDITOR_FLAGGED/UNFLAGGED (topic = editor)

---

## Space ID Format

Space IDs are 16 bytes (bytes16), generated as:

```solidity
bytes16(keccak256(abi.encodePacked(
    'grc20.space',
    address account,
    uint256 nonce,
    uint256 chainId
)))
```

The result is the first 16 bytes of the keccak256 hash.

---

## Initialization Data

### DAOSpace Initialize

```solidity
abi.encode(
    ISpaceRegistry spaceRegistry,
    VotingSettings votingSettings,
    address[] initialEditors,
    address[] initialMembers,
    bytes publishEditsData
)
```

**VotingSettings tuple:**
```solidity
(
    uint256 slowPathPercentageThreshold,
    uint256 fastPathFlatThreshold,
    uint256 quorum,
    uint256 duration
)
```

### SpaceRegistry Initialize

```solidity
abi.encode(address owner)
```

---

## Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `RATIO_BASE` | 10,000,000 | Denominator for percentage thresholds |
| `MINIMUM_VOTING_DURATION` | 60 seconds | Minimum proposal voting period |

### Percentage Calculation

For slow path support threshold:

```
passed = (RATIO_BASE - threshold) * yes > threshold * no
```

Example with 50% threshold (5,000,000):
```
passed = (10000000 - 5000000) * yes > 5000000 * no
       = 5000000 * yes > 5000000 * no
       = yes > no
```

---

## Rust Decoding Examples

### Decode proposalId from bytes16

```rust
fn decode_proposal_id(data: &[u8]) -> [u8; 16] {
    // bytes16 is right-aligned in 32 bytes when ABI encoded
    let mut id = [0u8; 16];
    id.copy_from_slice(&data[16..32]);
    id
}
```

### Decode address from topic

```rust
fn decode_address_from_topic(topic: [u8; 32]) -> [u8; 20] {
    // Address is right-aligned (last 20 bytes)
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&topic[12..32]);
    addr
}
```

### Decode VotingMode

```rust
fn decode_voting_mode(value: u8) -> VotingMode {
    match value {
        0 => VotingMode::Slow,
        1 => VotingMode::Fast,
        _ => panic!("Invalid voting mode"),
    }
}
```
