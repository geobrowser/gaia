---
title: Decode Subspace Proposal Actions from Ping Calldata
type: feat
date: 2026-03-04
---

# Decode Subspace Proposal Actions from Ping Calldata

## Overview

The DAOSpace contract uses `ping(bytes32 _action, bytes32 _topic, bytes _data)` as a generic passthrough for subspace operations. When a proposal contains a subspace action, the action calldata is `ping.selector (4 bytes) + abi.encode(action_hash, topic, data)`. The pipeline currently maps the `ping` selector (`0xc70d8282`) to `ProposalActionType::Ping` and drops it — returning `None` from `decode_proposal_action`. This means subspace proposals are stored as `Unknown` in the database and are invisible to API consumers.

## Problem Statement

Users cannot filter or view subspace proposals (verified, related, topic) in the proposals API because:

1. The pipeline recognizes `ping` at the selector level but doesn't decode the inner `_action` bytes32
2. No proto messages exist for subspace proposal actions
3. The database `proposalActionTypeEnum` has no subspace values
4. The API has no subspace action response types

## Proposed Solution

Decode the inner `_action` bytes32 from ping calldata to sub-classify subspace proposals, then propagate them end-to-end: pipeline → proto → kg-indexer → database → REST API.

### Architecture Decision: Where does sub-classification happen?

Keep `ProposalActionType::from_calldata` as a pure selector match (returns `Ping`). The sub-classification into specific subspace types happens inside `decode_proposal_action` — a new `decode_ping_subspace_action` function decodes the ABI-encoded ping args, matches the inner `_action` bytes32 against the known subspace constants, and returns the appropriate proto variant. Non-subspace pings fall through to `None` as today.

This preserves the existing architectural separation: `from_calldata` → classify by selector, `decode_proposal_action` → decode typed payload.

### Data Layout

The `ping(bytes32,bytes32,bytes)` calldata after selector strip is ABI-encoded:

```
offset 0:    _action  (bytes32) — keccak256 hash of the action name
offset 32:   _topic   (bytes32) — packed field, layout depends on action type
offset 64:   _data    (bytes)   — dynamic, always empty for subspace actions
```

The `_topic` field packing (same layout used by the trust pipeline in `trust.rs`):

| Action | `_topic[0..16]` | `_topic[16..32]` |
|--------|-----------------|-------------------|
| `SUBSPACE_VERIFIED` | zero-padded | target child space ID |
| `SUBSPACE_UNVERIFIED` | zero-padded | target child space ID |
| `SUBSPACE_RELATED` | zero-padded | target child space ID |
| `SUBSPACE_UNRELATED` | zero-padded | target child space ID |
| `SUBSPACE_TOPIC_DECLARED` | child space ID | topic entity ID |
| `SUBSPACE_TOPIC_REMOVED` | child space ID | topic entity ID |

For edge actions (verified/related), the parent space is the proposal's `space_id` (the DAO that owns the proposal). The `_topic` field carries only the target child space.

For topic actions, the `_topic` field packs both the child space ID and the topic entity ID.

### Storage Model

The `proposal_actions` table already has `target_id` (UUID, nullable). For edge actions, `target_id` stores the target child space ID. For topic actions, `target_id` stores the topic entity ID and a new `subspace_id` column stores the child space ID. This avoids overloading `target_id` with different semantics.

**Alternative considered:** Use `target_id` for one and `content_id` (existing bytes column) for the other. Rejected because `content_id` is typed as `bytea` not `uuid`, and repurposing it would be confusing.

## Technical Approach

### Phase 1: Proto Schema + Pipeline Decode

#### `hermes-schema/proto/governance.proto`

Add 4 new proto messages and 6 new enum values:

```protobuf
enum ProposalActionType {
  // ... existing values 0-10 ...
  PROPOSAL_ACTION_SUBSPACE_VERIFIED = 11;
  PROPOSAL_ACTION_SUBSPACE_UNVERIFIED = 12;
  PROPOSAL_ACTION_SUBSPACE_RELATED = 13;
  PROPOSAL_ACTION_SUBSPACE_UNRELATED = 14;
  PROPOSAL_ACTION_SUBSPACE_TOPIC_DECLARED = 15;
  PROPOSAL_ACTION_SUBSPACE_TOPIC_REMOVED = 16;
}

// Decoded action: add a verified subspace edge
message SubspaceVerifiedAction {
  bytes target_space_id = 1;  // 16 bytes - child space to verify
}

// Decoded action: remove a verified subspace edge
message SubspaceUnverifiedAction {
  bytes target_space_id = 1;  // 16 bytes - child space to unverify
}

// Decoded action: add a related subspace edge
message SubspaceRelatedAction {
  bytes target_space_id = 1;  // 16 bytes - child space to relate
}

// Decoded action: remove a related subspace edge
message SubspaceUnrelatedAction {
  bytes target_space_id = 1;  // 16 bytes - child space to unrelate
}

// Decoded action: declare a topic for a subspace
message SubspaceTopicDeclaredAction {
  bytes subspace_id = 1;  // 16 bytes - child space
  bytes topic_id = 2;     // 16 bytes - topic entity ID
}

// Decoded action: remove a topic from a subspace
message SubspaceTopicRemovedAction {
  bytes subspace_id = 1;  // 16 bytes - child space
  bytes topic_id = 2;     // 16 bytes - topic entity ID
}
```

Add to `ProposalAction.oneof action`:

```protobuf
SubspaceVerifiedAction subspace_verified = 19;
SubspaceUnverifiedAction subspace_unverified = 20;
SubspaceRelatedAction subspace_related = 21;
SubspaceUnrelatedAction subspace_unrelated = 22;
SubspaceTopicDeclaredAction subspace_topic_declared = 23;
SubspaceTopicRemovedAction subspace_topic_removed = 24;
```

#### `hermes-pipeline/src/decode.rs`

Add a `decode_ping_args` function using `alloy::sol!`:

```rust
sol! {
    function ping(bytes32 action, bytes32 topic, bytes data);
}

pub struct PingArgs {
    pub action: [u8; 32],
    pub topic: [u8; 32],
    pub data: Vec<u8>,
}

pub fn decode_ping_args(calldata: &[u8]) -> Result<PingArgs, DecodeError> {
    // Strip 4-byte selector, ABI-decode (bytes32, bytes32, bytes)
}
```

No changes to `ProposalActionType` enum or `from_calldata` — `Ping` stays as-is.

#### `hermes-pipeline/src/pipelines/governance.rs`

Replace the `Ping => None` arm:

```rust
ProposalActionType::Ping => decode_ping_subspace_action(calldata),
```

New function `decode_ping_subspace_action`:

```rust
fn decode_ping_subspace_action(calldata: &[u8]) -> Option<proposal_action::Action> {
    let args = decode_ping_args(calldata).ok()?;
    
    if args.action == *actions::SUBSPACE_VERIFIED {
        let target = args.topic[16..32].to_vec();
        Some(proposal_action::Action::SubspaceVerified(SubspaceVerifiedAction {
            target_space_id: target,
        }))
    } else if args.action == *actions::SUBSPACE_UNVERIFIED {
        // ... same pattern
    } else if args.action == *actions::SUBSPACE_TOPIC_DECLARED {
        let subspace_id = args.topic[0..16].to_vec();
        let topic_id = args.topic[16..32].to_vec();
        Some(proposal_action::Action::SubspaceTopicDeclared(SubspaceTopicDeclaredAction {
            subspace_id,
            topic_id,
        }))
    } else {
        // Non-subspace ping — fall through to None (stored as Unknown)
        None
    }
}
```

### Phase 2: KG-Indexer

#### `kg-indexer/src/models/governance.rs`

Add 6 new `ProposalActionPayload` variants:

```rust
SubspaceVerified { target_space_id: Uuid },
SubspaceUnverified { target_space_id: Uuid },
SubspaceRelated { target_space_id: Uuid },
SubspaceUnrelated { target_space_id: Uuid },
SubspaceTopicDeclared { subspace_id: Uuid, topic_id: Uuid },
SubspaceTopicRemoved { subspace_id: Uuid, topic_id: Uuid },
```

#### `kg-indexer/src/handlers/governance.rs`

Add new arms in `map_proposal_action` for the 6 new proto `Action` variants. Same pattern as existing member/editor variants: extract bytes → `bytes_to_uuid`.

Update `derive_proposal_name` with human-readable labels:
- `SubspaceVerified` → `"Add Verified Subspace"`
- `SubspaceUnverified` → `"Remove Verified Subspace"`
- `SubspaceRelated` → `"Add Related Subspace"`
- `SubspaceUnrelated` → `"Remove Related Subspace"`
- `SubspaceTopicDeclared` → `"Declare Subspace Topic"`
- `SubspaceTopicRemoved` → `"Remove Subspace Topic"`

#### `kg-indexer/src/storage.rs`

Add new arms in `insert_proposal_actions` for the 6 variants. Edge actions use `target_id` for the target space. Topic actions use `target_id` for the topic entity ID and `subspace_id` for the child space.

The action_type strings for the SQL enum cast:
- `"SubspaceVerified"`, `"SubspaceUnverified"`, `"SubspaceRelated"`, `"SubspaceUnrelated"`, `"SubspaceTopicDeclared"`, `"SubspaceTopicRemoved"`

### Phase 3: Database Migration

#### `api/src/services/storage/schema.ts`

Add 6 new values to `proposalActionTypeEnum`:

```typescript
export const proposalActionTypeEnum = pgEnum("proposalActionType", [
    "AddMember", "RemoveMember", "AddEditor", "RemoveEditor",
    "UnflagEditor", "Publish", "Flag", "Unflag",
    "UpdateVotingSettings", "Unknown",
    // Subspace proposal actions
    "SubspaceVerified", "SubspaceUnverified",
    "SubspaceRelated", "SubspaceUnrelated",
    "SubspaceTopicDeclared", "SubspaceTopicRemoved",
])
```

Add `subspace_id` column to `proposal_actions` table (UUID, nullable) for topic actions.

Run `drizzle-kit generate` to create the migration. The generated SQL will need manual adjustment because `ALTER TYPE ... ADD VALUE` cannot run inside a transaction. Each `ADD VALUE` statement must be a separate statement outside `BEGIN/COMMIT`.

### Phase 4: API Layer

#### `api/src/proposals/types.ts`

Add 6 new values to `PROPOSAL_ACTION_TYPES`:

```typescript
export const PROPOSAL_ACTION_TYPES = [
    // ... existing values ...
    "SubspaceVerified", "SubspaceUnverified",
    "SubspaceRelated", "SubspaceUnrelated",
    "SubspaceTopicDeclared", "SubspaceTopicRemoved",
] as const
```

Add new `ActionResponse` interfaces:

```typescript
interface SubspaceVerifiedAction { actionType: "SUBSPACE_VERIFIED"; targetSpaceId: string }
interface SubspaceUnverifiedAction { actionType: "SUBSPACE_UNVERIFIED"; targetSpaceId: string }
interface SubspaceRelatedAction { actionType: "SUBSPACE_RELATED"; targetSpaceId: string }
interface SubspaceUnrelatedAction { actionType: "SUBSPACE_UNRELATED"; targetSpaceId: string }
interface SubspaceTopicDeclaredAction { actionType: "SUBSPACE_TOPIC_DECLARED"; subspaceId: string; topicId: string }
interface SubspaceTopicRemovedAction { actionType: "SUBSPACE_TOPIC_REMOVED"; subspaceId: string; topicId: string }
```

Add to the `ActionResponse` discriminated union.

#### `api/src/proposals/router.ts`

Add 6 new cases in `mapToActionResponse`:

```typescript
case "SubspaceVerified":
    return { actionType: "SUBSPACE_VERIFIED", targetSpaceId: action.targetId ?? "" }
// ... similar for others
case "SubspaceTopicDeclared":
    return { actionType: "SUBSPACE_TOPIC_DECLARED", subspaceId: action.subspaceId ?? "", topicId: action.targetId ?? "" }
```

## Acceptance Criteria

- [ ] Proposals with subspace ping actions are decoded and stored with the correct `action_type` (not `Unknown`)
- [ ] `GET /proposals?actionTypes=SubspaceVerified` returns only proposals with verified subspace actions
- [ ] Non-subspace ping actions still fall through to `Unknown`
- [ ] Malformed ping calldata is handled gracefully (logged, stored as `Unknown`)
- [ ] Pipeline unit tests cover all 6 subspace action types + non-subspace ping fallthrough + malformed data
- [ ] KG-indexer handler unit tests cover all 6 proto variants
- [ ] API returns correct typed responses for each subspace action type

## Deployment Order

1. **Database migration** — `ALTER TYPE` adds new enum values. Must run first.
2. **API** — New types + `mapToActionResponse` cases. Safe to deploy because no rows use the new types yet.
3. **KG-indexer** — New handler arms + storage. Safe because old pipeline still emits `None` for ping.
4. **Pipeline** — Starts emitting new proto variants. KG-indexer already handles them.

The indexer should be rerun after all components are deployed to backfill historical subspace proposals.

## Dependencies & Risks

- **`ALTER TYPE ADD VALUE` is not transactional** — each statement must run outside a transaction. Drizzle-generated migration may need manual adjustment (same as migration `0045`).
- **Backward compatibility** — Old pipeline emitting `Ping` → new indexer: proto `action` is `None` → stored as `Unknown`. Fine.
- **Forward compatibility** — New pipeline → old indexer: unknown proto oneof variant → `action` is `None` → stored as `Unknown`. Fine, but the new enum values won't exist in the old database. Hence: deploy migration first.
- **Re-indexing** — Historical subspace proposals currently stored as `Unknown` will need re-indexing to be reclassified. The indexer rerun handles this.

## References

- Trust pipeline topic field decoding: `hermes-pipeline/src/pipelines/trust.rs` (lines 130-200)
- Existing proposal action decode pattern: `hermes-pipeline/src/pipelines/governance.rs` (lines 258-327)
- Action constants: `hermes-substream/src/lib.rs` (lines 113-152)
- Existing proposal action storage: `kg-indexer/src/storage.rs` (lines 844-950)
- DAOSpace contract `ping` function: `ping(bytes32 _action, bytes32 _topic, bytes calldata _data)` — generic passthrough that re-enters Space Registry
- Previous PR: #438 (wired typed subspace removal events end-to-end)
