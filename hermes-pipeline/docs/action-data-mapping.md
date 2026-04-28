# Action Data Mapping

This document maps blockchain actions to their Kafka protobuf outputs, showing how onchain data is decoded and transformed.

## Overview

Actions emitted from the Space Registry contract have the following structure:
- `action`: keccak256 hash identifying the action type
- `from_id`: 16-byte space ID of the actor
- `to_id`: 16-byte space ID of the target (when applicable)
- `topic`: 32-byte field with action-specific data
- `data`: ABI-encoded payload specific to each action type

## Action Mappings

### Governance Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| PROPOSAL_CREATED | `space.governance` | `HermesProposalCreated` (one per action) |
| PROPOSAL_SETTINGS_SELECTED | `space.governance` | Squashed with PROPOSAL_CREATED |
| PROPOSAL_VOTED | `space.governance` | `HermesProposalVoted` |
| PROPOSAL_EXECUTED | `space.governance` | `HermesProposalExecuted` |
| MEMBERSHIP_REQUESTED | `space.governance` | Triggers PROPOSAL_CREATED + PROPOSAL_SETTINGS_SELECTED |

#### PROPOSAL_CREATED

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.PROPOSAL_CREATED')`
- Topic: `bytes32(proposalId)` - set by fetch()
- Data: `abi.encode(bytes16 proposalId, VotingMode votingMode, Action[] actions)`

**VotingMode:** `Slow (0)`, `Fast (1)`
- Slow path: majority voting with voting window, multiple actions allowed
- Fast path: threshold-based, immediate execution, single action only

**Action struct:**
| Field | Type |
|-------|------|
| `to` | address (20 bytes) - target contract |
| `value` | uint256 (32 bytes) - ETH value to send |
| `data` | bytes - calldata (function selector + encoded args) |

**Proto Output:** `HermesProposalCreated`

One event is emitted **per action** in the proposal. For proposals with multiple actions
(slow path only), multiple events are emitted with the same `proposal_id` but different
`action_index` values.

| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `proposal_id` | `topic` | bytes (32) |
| `voting_mode` | Decoded from `data` | `VotingMode` enum |
| `action_index` | Index in actions array | uint32 (0-based) |
| `action_count` | Length of actions array | uint32 |
| `action_type` | Decoded from calldata selector | `ProposalActionType` enum |
| `action` | Raw action data | `ProposalAction` message |

**ProposalAction:**
| Field | Type |
|-------|------|
| `to` | bytes (20) - target address |
| `value` | bytes (32) - uint256 ETH value |
| `data` | bytes - calldata |
| `target_address` | bytes (20) - decoded address argument (for address-taking actions) |

The `target_address` field is populated for actions that take an address argument:
`ADD_MEMBER`, `REMOVE_MEMBER`, `ADD_EDITOR`, `REMOVE_EDITOR`, `UNRESTRICT_SPACE`

**ProposalActionType** (decoded from first 4 bytes of calldata):

| Type | Selector | Function Signature |
|------|----------|-------------------|
| `UNKNOWN` | - | Unknown/unrecognized selector |
| `ADD_MEMBER` | `0xca6d56dc` | `addMember(address)` |
| `REMOVE_MEMBER` | `0x0b1ca49a` | `removeMember(address)` |
| `ADD_EDITOR` | `0xe5975bdc` | `addEditor(address)` |
| `REMOVE_EDITOR` | `0x2d55feaf` | `removeEditor(address)` |
| `PUBLISH` | `0x6b47f61a` | `publish(bytes32,bytes,bytes)` |
| `FLAG` | `0xfe1e3042` | `flag(bytes32,bytes)` |
| `UNFLAG` | `0xc696840f` | `unflag(bytes32,bytes)` |
| `UNRESTRICT_SPACE` | `0xb2c436ba` | `unrestrictSpace(address)` |
| `UPDATE_VOTING_SETTINGS` | `0xd21e8541` | `updateVotingSettings((uint256,uint256,uint256,uint256))` |
| `PING` | `0xc70d8282` | `ping(bytes32,bytes32,bytes)` |

#### PROPOSAL_VOTED

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.PROPOSAL_VOTED')`
- Topic: `bytes32(proposalId)` - set by fetch()
- Data: `abi.encode(bytes16 proposalId, VoteOption voteOption)`

**VoteOption:** `None (0)`, `Yes (1)`, `No (2)`, `Abstain (3)`

**Proto Output:** `HermesProposalVoted`
| Field | Source | Type |
|-------|--------|------|
| `voter_id` | `from_id` | bytes (16) |
| `space_id` | `to_id` | bytes (16) |
| `proposal_id` | `topic` | bytes (32) |
| `vote` | Decoded from `data` | `ProposalVoteOption` enum |

#### PROPOSAL_EXECUTED

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.PROPOSAL_EXECUTED')`
- Topic: `bytes32(proposalId)` - set by fetch()
- Data: `abi.encode(bytes16 proposalId)`

**Proto Output:** `HermesProposalExecuted`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `proposal_id` | `topic` | bytes (32) |

> **Note: Fast-path auto-execution gap.** The DAOSpace contract auto-executes
> fast-path proposals inline when a YES vote meets the threshold (inside `_vote`
> → `_executeProposal`), but does **not** emit a `PROPOSAL_EXECUTED` event in
> that path. Only the explicit `enter(PROPOSAL_EXECUTED)` path (used for slow-path
> proposals and manual execution) emits this event. The kg-indexer compensates
> by detecting fast-path execution in its tally worker: after updating vote counts,
> it checks for fast-path proposals where `yes_count > threshold` and
> `executed_at IS NULL`, then sets `executed_at` from the latest vote timestamp.

#### PROPOSAL_SETTINGS_SELECTED

Emitted by the DAO (ping pattern) after a proposal is created or escalated.

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.PROPOSAL_SETTINGS_SELECTED')`
- Topic: `bytes32(proposalId)`
- Data: `abi.encode(uint256 startDate, uint256 lastDate, VotingMode votingMode, uint256 quorum, uint256 supportThreshold)`

**Note:** This event is squashed with PROPOSAL_CREATED during pipeline processing.
The voting settings are merged into the `HermesProposalCreated` message.

| Field | Type | Description |
|-------|------|-------------|
| `startDate` | uint256 | Voting period start timestamp |
| `lastDate` | uint256 | Voting period end timestamp |
| `votingMode` | uint8 | Slow (0) or Fast (1) |
| `quorum` | uint256 | Minimum total votes required |
| `supportThreshold` | uint256 | Flat threshold (fast) or percentage of RATIO_BASE (slow) |

#### MEMBERSHIP_REQUESTED

Emitted via `enter()` when a space requests to join a DAO as a member.
This action triggers an internal `createProposal()` which emits PROPOSAL_CREATED + PROPOSAL_SETTINGS_SELECTED.

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.MEMBERSHIP_REQUESTED')`
- Topic: `bytes32(proposalId)` - set by fetch()
- Data: `abi.encode(bytes16 proposalId, address newMember)`

**Proto Output:** Processed as part of the triggered PROPOSAL_CREATED flow

| Field | Source | Type |
|-------|--------|------|
| `requester_id` | `from_id` | bytes (16) |
| `space_id` | `to_id` | bytes (16) |
| `proposal_id` | `topic` | bytes (32) |
| `new_member` | Decoded from `data` | address (20 bytes) |

---

### Permissionless Voting Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| UPVOTED | `curation.votes` | `HermesVoteCast` |
| DOWNVOTED | `curation.votes` | `HermesVoteCast` |
| UNVOTED | `curation.votes` | `HermesVoteCast` |

#### UPVOTED / DOWNVOTED / UNVOTED

**Onchain:**
- Action: `keccak256('PERMISSIONLESS.UPVOTED')` / `DOWNVOTED` / `UNVOTED`
- Topic: `bytes32(bytes4(objectType) << 224 | bytes16(objectId) << 96)`
- Data: `abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))`

**Proto Output:** `HermesVoteCast`
| Field | Source | Type |
|-------|--------|------|
| `voter_id` | `from_id` | bytes (16) |
| `object_type` | `topic[0..4]` | bytes (4) |
| `object_id` | `topic[4..20]` | bytes (16) |
| `direction` | Action type | `VoteDirection` enum |
| `version` | Decoded from `data` | uint32 |
| `group_id` | Decoded from `data` | bytes (16) |
| `space_pov` | Decoded from `data` | bytes (16) |

**VoteDirection:** `UP (0)`, `DOWN (1)`, `NONE (2)`

---

### Topic Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| TOPIC_SET | `space.topics` | `HermesTopicDeclared` |

#### TOPIC_SET

**Onchain:**
- Action: `keccak256('GOVERNANCE.TOPIC_SET')`
- Topic: `bytes32(bytes16(topicId) | padding)`
- Data: optional topic metadata payload

> Note: the wire-format proto stays `HermesTopicDeclared` for Kafka consumer
> compatibility. Only the onchain action selector renamed in Governance V2.

**Proto Output:** `HermesTopicDeclared`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `topic_id` | First 16 bytes of `topic` | bytes (16) |

---

### Moderation Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| SPACE_FAST_PATH_RESTRICTED | `space.moderation` | `HermesEditorFlagged` |
| SPACE_FAST_PATH_UNRESTRICTED | `space.moderation` | `HermesEditorUnflagged` |
| FLAGGED | `space.moderation` | `HermesContentFlagged` |
| UNFLAGGED | `space.moderation` | `HermesContentUnflagged` |

#### SPACE_FAST_PATH_RESTRICTED

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.SPACE_FAST_PATH_RESTRICTED')`
- Topic: `bytes32(spaceId)` - restricted space's ID (set by fetch())
- Data: `abi.encode(address)` - restricted space's address

**Proto Output:** `HermesEditorFlagged`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `editor_account` | `data[12..32]` | bytes (20) |

#### SPACE_FAST_PATH_UNRESTRICTED

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.SPACE_FAST_PATH_UNRESTRICTED')`
- Topic: `bytes32(spaceId)` - unrestricted space's ID
- Data: `abi.encode(address)` - unrestricted space's address

**Proto Output:** `HermesEditorUnflagged`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `editor_account` | `data[12..32]` | bytes (20) |

#### FLAGGED / UNFLAGGED

**Onchain:**
- Action: `keccak256('GOVERNANCE.FLAGGED')` / `UNFLAGGED`
- Topic: `bytes32(TOPIC_UUID)` (optional)
- Data: `abi.encode(bytes(uri))`

**Proto Output:** `HermesContentFlagged` / `HermesContentUnflagged`
| Field | Source | Type |
|-------|--------|------|
| `flagger_id` / `unflagger_id` | `from_id` | bytes (16) |
| `target_space_id` | `to_id` | bytes (16) |
| `topic_id` | `topic` | bytes (32) |
| `uri` | Decoded from `data` | string |

---

### Membership Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| EDITOR_ADDED | `space.membership` | `HermesRoleGranted` |
| EDITOR_REMOVED | `space.membership` | `HermesRoleRevoked` |
| MEMBER_ADDED | `space.membership` | `HermesRoleGranted` |
| MEMBER_REMOVED | `space.membership` | `HermesRoleRevoked` |
| SPACE_LEFT | `space.membership` | `HermesSpaceLeft` |

#### EDITOR_ADDED / MEMBER_ADDED

**Onchain:**
- Action: `keccak256('GOVERNANCE.EDITOR_ADDED')` / `MEMBER_ADDED`
- Topic: `bytes32(memberSpaceId)` - member's space ID (first 16 bytes)
- Data: empty

**Proto Output:** `HermesRoleGranted`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `member_space_id` | `topic[0..16]` | bytes (16) |
| `role` | Action type | `MembershipRole` enum |

#### EDITOR_REMOVED / MEMBER_REMOVED

**Onchain:**
- Action: `keccak256('GOVERNANCE.EDITOR_REMOVED')` / `MEMBER_REMOVED`
- Topic: `bytes32(memberSpaceId)` - member's space ID (first 16 bytes)
- Data: empty

**Proto Output:** `HermesRoleRevoked`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `member_space_id` | `topic[0..16]` | bytes (16) |
| `role` | Action type | `MembershipRole` enum |

#### SPACE_LEFT

**Onchain (ZC16):**
- Action: `keccak256('GOVERNANCE.SPACE_LEFT')`
- Topic: `bytes32(keccak256('ROLE'))` - set by fetch() from data, e.g., `keccak256('EDITOR')` or `keccak256('MEMBER')`
- Data: `abi.encode(bytes32 role)` - the role being left

**Proto Output:** `HermesSpaceLeft`
| Field | Source | Type |
|-------|--------|------|
| `member_id` | `from_id` | bytes (16) |
| `space_id` | `to_id` | bytes (16) |
| `role` | `topic` | bytes (32) |

---

### Space Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| SPACE_REGISTERED | `space.creations` | `HermesCreateSpace` |
| SPACE_MIGRATED | `space.creations` | `HermesCreateSpace` |

#### SPACE_REGISTERED / SPACE_MIGRATED

**Onchain:**
- Action: `keccak256('GOVERNANCE.SPACE_ID_REGISTERED')` / `SPACE_ID_MIGRATED`
- Topic: `bytes32(bytes20(spaceAddress))`
- Data: empty

**Proto Output:** `HermesCreateSpace`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `to_id` | bytes (16) |
| `payload` | Determined by space type | oneof `EoaSpacePayload` / `DefaultDaoSpacePayload` |

**Payload Types:**
- `EoaSpacePayload`: `owner` (20 bytes) - owner's EOA address from `topic[0..20]`
- `DefaultDaoSpacePayload`: `address` (20 bytes) - DAO contract address from `topic[0..20]`

---

### Trust/Topology Actions

| Action | Kafka Topic | Proto Extension Variant | Header `extension-type` |
|--------|-------------|------------------------|------------------------|
| SUBSPACE_VERIFIED | `space.trust.extensions` | `VerifiedExtension` | `VERIFIED` |
| SUBSPACE_RELATED | `space.trust.extensions` | `RelatedExtension` | `RELATED` |
| SUBSPACE_TOPIC_SET | `space.trust.extensions` | `SubtopicExtension` | `SUBTOPIC` |
| SUBSPACE_UNVERIFIED | `space.trust.extensions` | `VerifiedRemoval` | `VERIFIED_REMOVAL` |
| SUBSPACE_UNRELATED | `space.trust.extensions` | `RelatedRemoval` | `RELATED_REMOVAL` |
| SUBSPACE_TOPIC_UNSET | `space.trust.extensions` | `SubtopicRemoval` | `SUBTOPIC_REMOVAL` |


#### SUBSPACE_VERIFIED / SUBSPACE_RELATED / SUBSPACE_UNVERIFIED / SUBSPACE_UNRELATED

**Onchain:**
- Action: `keccak256('GOVERNANCE.SUBSPACE_*')`
- Topic: `bytes32(bytes16(targetSpaceId))` → `[target_space_id: 16 bytes | padding: 16 bytes]`
- Data: empty

ZC16: Solidity `bytes32(bytes16)` right-pads, so the bytes16 value occupies `[0..16]`.

**Proto Output:** `HermesSpaceTrustExtension`
| Field | Source | Type |
|-------|--------|------|
| `source_space_id` | `from_id` | bytes (16) |
| `extension` | Action type → oneof variant | `VerifiedExtension` / `RelatedExtension` / `VerifiedRemoval` / `RelatedRemoval` |
| `extension.*.target_space_id` | `topic[0..16]` | bytes (16) |

#### SUBSPACE_TOPIC_SET / SUBSPACE_TOPIC_UNSET

**Onchain:**
- Action: `keccak256('GOVERNANCE.SUBSPACE_TOPIC_SET')` / `SUBSPACE_TOPIC_UNSET`
- Topic: `[subspace_id: 16 bytes | topic_id: 16 bytes]`
- Data: empty

**Proto Output:** `HermesSpaceTrustExtension`
| Field | Source | Type |
|-------|--------|------|
| `source_space_id` | `from_id` | bytes (16) |
| `extension` | Action type → oneof variant | `SubtopicExtension` / `SubtopicRemoval` |
| `extension.*.target_topic_id` | `topic[16..32]` | bytes (16) |
