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
| PROPOSAL_CREATED | `space.governance` | `HermesProposalCreated` |
| PROPOSAL_VOTED | `space.governance` | `HermesProposalVoted` |
| PROPOSAL_EXECUTED | `space.governance` | `HermesProposalExecuted` |

#### PROPOSAL_CREATED

**Onchain:**
- Action: `keccak256('GOVERNANCE.PROPOSAL_CREATED')`
- Topic: `bytes32(proposalId)`
- Data: `abi.encode(Operation[], VoteOption)`

**Proto Output:** `HermesProposalCreated`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `proposal_id` | `topic` | bytes (32) |
| `operations` | Decoded from `data` | `ProposalOperation[]` |
| `default_vote` | Decoded from `data` | `ProposalVoteOption` enum |

**ProposalOperation:**
| Field | Type |
|-------|------|
| `to` | bytes (20) - target address |
| `value` | bytes (32) - uint256 ETH value |
| `calldata` | bytes - operation calldata |

**ProposalVoteOption:** `YES (0)`, `NO (1)`, `ABSTAIN (2)`

#### PROPOSAL_VOTED

**Onchain:**
- Action: `keccak256('GOVERNANCE.PROPOSAL_VOTED')`
- Topic: `bytes32(proposalId)`
- Data: `abi.encode(bytes32(proposalId), VoteOption)`

**Proto Output:** `HermesProposalVoted`
| Field | Source | Type |
|-------|--------|------|
| `voter_id` | `from_id` | bytes (16) |
| `space_id` | `to_id` | bytes (16) |
| `proposal_id` | `topic` | bytes (32) |
| `vote` | Decoded from `data` | `ProposalVoteOption` enum |

#### PROPOSAL_EXECUTED

**Onchain:**
- Action: `keccak256('GOVERNANCE.PROPOSAL_EXECUTED')`
- Topic: `bytes32(proposalId)`
- Data: `abi.encode(bytes32(proposalId))` (redundant)

**Proto Output:** `HermesProposalExecuted`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `proposal_id` | `topic` | bytes (32) |

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
| TOPIC_DECLARED | `space.topics` | `HermesTopicDeclared` |

#### TOPIC_DECLARED

**Onchain:**
- Action: `keccak256('GOVERNANCE.TOPIC_DECLARED')`
- Topic: unused
- Data: `abi.encode(bytes16(topicId))`

**Proto Output:** `HermesTopicDeclared`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `topic_id` | Decoded from `data` | bytes (16) |

---

### Moderation Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| EDITOR_FLAGGED | `space.moderation` | `HermesEditorFlagged` |
| EDITOR_UNFLAGGED | `space.moderation` | `HermesEditorUnflagged` |
| FLAGGED | `space.moderation` | `HermesContentFlagged` |
| UNFLAGGED | `space.moderation` | `HermesContentUnflagged` |

#### EDITOR_FLAGGED / EDITOR_UNFLAGGED

**Onchain:**
- Action: `keccak256('GOVERNANCE.EDITOR_FLAGGED')` / `EDITOR_UNFLAGGED`
- Topic: `bytes32(bytes20(editorAddress))`
- Data: empty

**Proto Output:** `HermesEditorFlagged` / `HermesEditorUnflagged`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `editor_account` | `topic[12..32]` | bytes (20) |

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
- Topic: `bytes32(bytes20(accountAddress))`
- Data: empty

**Proto Output:** `HermesRoleGranted`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `account` | `topic[12..32]` | bytes (20) |
| `role` | Action type | `MembershipRole` enum |

#### EDITOR_REMOVED / MEMBER_REMOVED

**Onchain:**
- Action: `keccak256('GOVERNANCE.EDITOR_REMOVED')` / `MEMBER_REMOVED`
- Topic: `bytes32(bytes20(accountAddress))`
- Data: empty

**Proto Output:** `HermesRoleRevoked`
| Field | Source | Type |
|-------|--------|------|
| `space_id` | `from_id` | bytes (16) |
| `account` | `topic[12..32]` | bytes (20) |
| `role` | Action type | `MembershipRole` enum |

#### SPACE_LEFT

**Onchain:**
- Action: `keccak256('GOVERNANCE.SPACE_LEFT')`
- Topic: `bytes32(keccak256('ROLE'))` e.g., `keccak256('EDITOR')`
- Data: empty

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
| `space_id` | `from_id` | bytes (16) |
| `topic_id` | `topic` | bytes (32) |
| `payload` | Determined by space type | oneof `PersonalSpace` / `DefaultDaoSpace` |

---

### Trust/Topology Actions

| Action | Kafka Topic | Proto Message |
|--------|-------------|---------------|
| SUBSPACE_ADDED | `space.trust.extensions` | `HermesSpaceTrustExtension` |
| SUBSPACE_REMOVED | `space.trust.extensions` | `HermesSpaceTrustExtension` |
| SUBSPACE_VERIFIED | `space.trust.extensions` | `HermesSpaceTrustExtension` |
| SUBSPACE_RELATED | `space.trust.extensions` | `HermesSpaceTrustExtension` |
| SUBSPACE_TOPIC_DECLARED | `space.trust.extensions` | `HermesSpaceTrustExtension` |

#### SUBSPACE_ADDED / SUBSPACE_REMOVED / SUBSPACE_VERIFIED / SUBSPACE_RELATED

**Onchain:**
- Action: `keccak256('GOVERNANCE.SUBSPACE_*')`
- Topic: `bytes32(spaceId)` - Space ID of the subspace
- Data: empty

**Proto Output:** `HermesSpaceTrustExtension`
| Field | Source | Type |
|-------|--------|------|
| `source_space_id` | `from_id` | bytes (16) |
| `extension` | Action type + `topic` | oneof `Verified` / `Related` / `Subtopic` |

#### SUBSPACE_TOPIC_DECLARED

**Onchain:**
- Action: `keccak256('GOVERNANCE.SUBSPACE_TOPIC_DECLARED')`
- Topic: `bytes32(bytes16(spaceId) | bytes16(topicId) >> 128)`
- Data: empty

**Proto Output:** `HermesSpaceTrustExtension`
| Field | Source | Type |
|-------|--------|------|
| `source_space_id` | `from_id` | bytes (16) |
| `extension.subtopic.target_space_id` | `topic[0..16]` | bytes (16) |
| `extension.subtopic.topic_id` | `topic[16..32]` | bytes (16) |
