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

The `ping(bytes32,bytes32,bytes)` calldata after selector strip is ABI-encoded as a tuple `(bytes32, bytes32, bytes)`:

```
head[0]:    _action  (bytes32) — keccak256 hash of the action name
head[32]:   _topic   (bytes32) — packed field, layout depends on action type
head[64]:   offset to _data
tail:       _data length + bytes — always empty for subspace actions
```

The `_topic` field packing (same layout used by the trust pipeline in `trust.rs`):

| Action | `_topic[0..16]` | `_topic[16..32]` |
|--------|-----------------|-------------------|
| `SUBSPACE_VERIFIED` | zero-padded | target child space ID |
| `SUBSPACE_UNVERIFIED` | zero-padded | target child space ID |
| `SUBSPACE_RELATED` | zero-padded | target child space ID |
| `SUBSPACE_UNRELATED` | zero-padded | target child space ID |
| `SUBSPACE_TOPIC_DECLARED` | (unused by pipeline) | topic entity ID |
| `SUBSPACE_TOPIC_REMOVED` | (unused by pipeline) | topic entity ID |

For edge actions (verified/related), the parent space is the proposal's `space_id` (the DAO that owns the proposal). The `_topic` field carries only the target child space in `[16..32]`.

For topic actions, the trust pipeline only extracts `topic[16..32]` as the `target_topic_id` — the `source_space_id` (from `from_id`) is the parent space. In the proposal context, the parent is the proposal's `space_id`. So we extract only `topic[16..32]` for the topic entity ID, matching the trust pipeline pattern exactly (`trust.rs:196`).

### Storage Model

The `proposal_actions` table has `target_id` (UUID, nullable, no FK constraint). This column is already a polymorphic UUID pointer — for member/editor actions it holds a space ID, for subspace actions it will hold:

- **Edge actions** (verified/related/unverified/unrelated): `target_id` = the target child space ID (references `spaces.id`)
- **Topic actions** (topic declared/removed): `target_id` = the topic entity ID (references `entities.id`)

The parent space is always the proposal's `space_id`. No new columns needed.

### Proto Message Design

Use 2 proto messages instead of 6 (simplicity reviewer finding). The 4 edge actions have identical structure (`bytes target_space_id`), and the 2 topic actions differ only in semantics. The `ProposalActionType` enum already discriminates the specific action.

However, since proto `oneof` requires distinct message types per field, and the existing codebase uses 1 message per action type (e.g., separate `AddMemberAction`/`RemoveMemberAction` with identical fields), we use 2 messages with 6 `oneof` arms — the edge message is shared across 4 arms, and the topic message across 2 arms.

## Technical Approach

### Phase 1: Proto Schema + Pipeline Decode

#### `hermes-schema/proto/governance.proto`

Add 2 new proto messages and 6 new enum values:

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

// Decoded action: subspace edge operation (add/remove verified or related edge)
// The specific operation type is determined by the ProposalActionType enum value.
message SubspaceEdgeAction {
  bytes target_space_id = 1;  // 16 bytes - target child space
}

// Decoded action: subspace topic operation (declare/remove topic)
// The specific operation type is determined by the ProposalActionType enum value.
message SubspaceTopicAction {
  bytes target_topic_id = 1;  // 16 bytes - topic entity ID
}
```

Add to `ProposalAction.oneof action` (6 arms, 2 message types):

```protobuf
SubspaceEdgeAction subspace_verified = 19;
SubspaceEdgeAction subspace_unverified = 20;
SubspaceEdgeAction subspace_related = 21;
SubspaceEdgeAction subspace_unrelated = 22;
SubspaceTopicAction subspace_topic_declared = 23;
SubspaceTopicAction subspace_topic_removed = 24;
```

#### `hermes-pipeline/src/decode.rs`

Add a `decode_ping_args` function following the existing tuple-decode pattern (strip selector, then `SolType::abi_decode`):

```rust
type PingArgsType = sol! { (bytes32, bytes32, bytes) };

pub struct PingArgs {
    pub action: [u8; 32],
    pub topic: [u8; 32],
    pub data: Vec<u8>,
}

pub fn decode_ping_args(calldata: &[u8]) -> Result<PingArgs, DecodeError> {
    // Strip 4-byte selector, then ABI-decode tuple
    if calldata.len() < 4 {
        return Err(DecodeError::DataTooShort { expected: 4, actual: calldata.len() });
    }
    let decoded = PingArgsType::abi_decode(&calldata[4..], true)
        .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;
    Ok(PingArgs {
        action: decoded.0.into(),
        topic: decoded.1.into(),
        data: decoded.2.to_vec(),
    })
}
```

No changes to `ProposalActionType` enum or `from_calldata` — `Ping` stays as-is.

#### `hermes-pipeline/src/pipelines/governance.rs`

Replace the `Ping => None` arm:

```rust
ProposalActionType::Ping => decode_ping_subspace_action(calldata),
```

New function `decode_ping_subspace_action` using `match` (codebase convention) with explicit error logging:

```rust
fn decode_ping_subspace_action(calldata: &[u8]) -> Option<proposal_action::Action> {
    let args = match decode_ping_args(calldata) {
        Ok(args) => args,
        Err(e) => {
            warn!(error = %e, calldata_len = calldata.len(), "Failed to decode ping calldata");
            return None;
        }
    };

    // Extract target from topic field — same byte layout as trust.rs
    let target = args.topic[16..32].to_vec();

    match args.action {
        x if x == actions::SUBSPACE_VERIFIED => {
            Some(proposal_action::Action::SubspaceVerified(SubspaceEdgeAction {
                target_space_id: target,
            }))
        }
        x if x == actions::SUBSPACE_UNVERIFIED => {
            Some(proposal_action::Action::SubspaceUnverified(SubspaceEdgeAction {
                target_space_id: target,
            }))
        }
        x if x == actions::SUBSPACE_RELATED => {
            Some(proposal_action::Action::SubspaceRelated(SubspaceEdgeAction {
                target_space_id: target,
            }))
        }
        x if x == actions::SUBSPACE_UNRELATED => {
            Some(proposal_action::Action::SubspaceUnrelated(SubspaceEdgeAction {
                target_space_id: target,
            }))
        }
        x if x == actions::SUBSPACE_TOPIC_DECLARED => {
            Some(proposal_action::Action::SubspaceTopicDeclared(SubspaceTopicAction {
                target_topic_id: target,
            }))
        }
        x if x == actions::SUBSPACE_TOPIC_REMOVED => {
            Some(proposal_action::Action::SubspaceTopicRemoved(SubspaceTopicAction {
                target_topic_id: target,
            }))
        }
        _ => None,  // Non-subspace ping — stored as Unknown
    }
}
```

#### Shared topic layout helper (`hermes-pipeline/src/decode.rs`)

Extract the `topic[16..32]` extraction into a shared helper to consolidate the byte-slicing knowledge between trust.rs and governance.rs:

```rust
/// Extract the target ID from a 32-byte topic field.
/// The target is always in the last 16 bytes (topic[16..32]).
/// Used by both trust pipeline (direct action events) and governance
/// pipeline (ping calldata within proposals).
pub fn extract_topic_target(topic: &[u8]) -> Option<&[u8]> {
    if topic.len() >= 32 {
        Some(&topic[16..32])
    } else {
        None
    }
}
```

### Phase 2: KG-Indexer

#### `kg-indexer/src/models/governance.rs`

Add 2 new `ProposalActionPayload` variants (matching the 2 proto message shapes):

```rust
/// Subspace edge action (verified/unverified/related/unrelated)
/// The specific edge type is determined by the action_type string in storage.
SubspaceEdge { target_space_id: Uuid },
/// Subspace topic action (topic declared/removed)
/// The specific operation is determined by the action_type string in storage.
SubspaceTopic { target_topic_id: Uuid },
```

#### `kg-indexer/src/handlers/governance.rs`

Add new arms in `map_proposal_action` for the 6 new proto `Action` oneof variants. The 4 edge variants all map to `SubspaceEdge`, the 2 topic variants to `SubspaceTopic`:

```rust
Some(Action::SubspaceVerified(a)) | Some(Action::SubspaceUnverified(a))
| Some(Action::SubspaceRelated(a)) | Some(Action::SubspaceUnrelated(a)) => {
    match bytes_to_uuid(&a.target_space_id) {
        Some(id) => ProposalActionPayload::SubspaceEdge { target_space_id: id },
        None => ProposalActionPayload::Unknown,
    }
}
Some(Action::SubspaceTopicDeclared(a)) | Some(Action::SubspaceTopicRemoved(a)) => {
    match bytes_to_uuid(&a.target_topic_id) {
        Some(id) => ProposalActionPayload::SubspaceTopic { target_topic_id: id },
        None => ProposalActionPayload::Unknown,
    }
}
```

Note: The `action_type` string is determined by which proto enum value accompanies the action, not by the payload variant. The kg-indexer should read `ProposalActionType` from the proto message to set the correct action_type string. If the proto doesn't carry the enum explicitly, the oneof field name discriminates it.

Update `derive_proposal_name` with human-readable labels:
- `SubspaceVerified` → `"Add Verified Subspace"`
- `SubspaceUnverified` → `"Remove Verified Subspace"`
- `SubspaceRelated` → `"Add Related Subspace"`
- `SubspaceUnrelated` → `"Remove Related Subspace"`
- `SubspaceTopicDeclared` → `"Declare Subspace Topic"`
- `SubspaceTopicRemoved` → `"Remove Subspace Topic"`

#### `kg-indexer/src/storage.rs`

Add new arms in `insert_proposal_actions`. Both variants use `target_id` — for edges it's a space ID, for topics it's a topic entity ID. The `action_type` string must be set per-variant (not per-payload-shape), so each oneof arm needs its own action_type string. The updated UNNEST SQL stays at 11 columns (no new column):

```rust
ProposalActionPayload::SubspaceEdge { target_space_id: id } => {
    target_id = Some(*id);
    // action_type is set from the proto enum, e.g. "SubspaceVerified"
    action_type_from_proto
}
ProposalActionPayload::SubspaceTopic { target_topic_id: id } => {
    target_id = Some(*id);
    // action_type is set from the proto enum, e.g. "SubspaceTopicDeclared"
    action_type_from_proto
}
```

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

No new columns needed — `target_id` is reused.

Run `drizzle-kit generate` to create the migration. The generated SQL will need manual adjustment because `ALTER TYPE ... ADD VALUE` cannot run inside a transaction. Each `ADD VALUE` statement must be a separate statement outside `BEGIN/COMMIT` (same as migration `0045`).

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

Add 2 new `ActionResponse` interfaces (collapsed from 6 since the shapes are identical within each group):

```typescript
interface SubspaceEdgeAction {
    actionType: "SUBSPACE_VERIFIED" | "SUBSPACE_UNVERIFIED" | "SUBSPACE_RELATED" | "SUBSPACE_UNRELATED"
    targetSpaceId: string
}

interface SubspaceTopicAction {
    actionType: "SUBSPACE_TOPIC_DECLARED" | "SUBSPACE_TOPIC_REMOVED"
    targetTopicId: string
}
```

Add to the `ActionResponse` discriminated union.

#### `api/src/proposals/router.ts`

Add new cases in `mapToActionResponse` with defensive null checks (existing convention — missing required fields → `UNKNOWN`):

```typescript
case "SubspaceVerified":
case "SubspaceUnverified":
case "SubspaceRelated":
case "SubspaceUnrelated":
    if (!action.targetId) return { actionType: "UNKNOWN" }
    return { actionType: ACTION_TYPE_MAP[action.actionType], targetSpaceId: action.targetId }

case "SubspaceTopicDeclared":
case "SubspaceTopicRemoved":
    if (!action.targetId) return { actionType: "UNKNOWN" }
    return { actionType: ACTION_TYPE_MAP[action.actionType], targetTopicId: action.targetId }
```

Note: `ACTION_TYPE_MAP` converts PascalCase DB values → SCREAMING_SNAKE_CASE API values (existing pattern).

## Acceptance Criteria

- [ ] Proposals with subspace ping actions are decoded and stored with the correct `action_type` (not `Unknown`)
- [ ] `GET /proposals?actionTypes=SubspaceVerified` returns only proposals with verified subspace actions
- [ ] Non-subspace ping actions still fall through to `Unknown`
- [ ] Malformed ping calldata logs a warning and is stored as `Unknown`
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

- **`ALTER TYPE ADD VALUE` is not transactional** — each statement must run outside a transaction. Drizzle-generated migration will need manual adjustment (same as migration `0045`).
- **Backward compatibility** — Old pipeline emitting `Ping` → new indexer: proto `action` is `None` → stored as `Unknown`. Fine.
- **Forward compatibility** — New pipeline → old indexer: unknown proto oneof variant → `action` is `None` → stored as `Unknown`. Fine, but the new enum values won't exist in the old database. Hence: deploy migration first.
- **Re-indexing** — Historical subspace proposals currently stored as `Unknown` will need re-indexing to be reclassified. The indexer rerun handles this.
- **`target_id` semantic overloading** — `target_id` is already polymorphic (space IDs for member/editor actions, no FK constraint). Subspace actions add space IDs (for edges) and entity IDs (for topics) to this column. The `action_type` enum disambiguates. Acceptable given the existing pattern.

## References

- Trust pipeline topic field decoding: `hermes-pipeline/src/pipelines/trust.rs:184-207`
- Existing proposal action decode pattern: `hermes-pipeline/src/pipelines/governance.rs:258-327`
- Action constants: `hermes-substream/src/lib.rs:113-152`
- Existing proposal action storage: `kg-indexer/src/storage.rs:844-950`
- Atlas topic model: `atlas/src/graph/state.rs` — `topic_edges: HashMap<SpaceId, HashSet<TopicId>>`
- Atlas events: `atlas/src/events.rs:76` — `Subtopic { target_topic_id: TopicId }`
- Subspace topics table: `api/src/services/storage/schema.ts:281-292` — `(space_id, topic_id)`
- Spaces table: `api/src/services/storage/schema.ts:100-105` — `topicId: uuid()` (space → topic association)
- DAOSpace contract `ping` function: `ping(bytes32 _action, bytes32 _topic, bytes calldata _data)` — generic passthrough that re-enters Space Registry
- Previous PR: #438 (wired typed subspace removal events end-to-end)
