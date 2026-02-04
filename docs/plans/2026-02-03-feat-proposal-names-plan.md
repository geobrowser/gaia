---
title: feat: Add human-readable names to proposals
type: feat
date: 2026-02-03
---

# feat: Add Human-Readable Names to Proposals

## Overview

Add human-readable names to proposals derived from their actions. For Publish actions, use the edit's `name` field (fetched from IPFS cache). For other actions, use the action type name. Multiple actions are concatenated with comma separator.

## Problem Statement / Motivation

Proposals currently have no human-readable identifier. Users see proposal IDs (UUIDs) but no context about what the proposal does. This makes it difficult to:
- Browse proposals in a list
- Understand what a proposal will do at a glance
- Search/filter proposals by content

## Proposed Solution

Flow the edit name through the pipeline:
1. **ipfs-indexer**: Store `name` in `ipfs_cache` when decoding edits
2. **hermes-pipeline**: Read name from cache, populate `PublishAction.name` proto field
3. **kg-indexer**: Concatenate action names, store in `proposals.name` column

### Action Type to Name Mapping

| Action Type | Display Name |
|-------------|-------------|
| AddMember | "Add Member" |
| RemoveMember | "Remove Member" |
| AddEditor | "Add Editor" |
| RemoveEditor | "Remove Editor" |
| UnflagEditor | "Unflag Editor" |
| Flag | "Flag" |
| Unflag | "Unflag" |
| UpdateVotingSettings | "Update Voting Settings" |
| Publish | `<edit.name>` from cache |
| Unknown | "Unknown Action" |

### Concatenation Rules

- Multiple actions joined with `, ` (comma + space)
- Max length: 500 characters (truncate with `...` if exceeded)
- Example: `"Add Member, My Edit Name, Flag"`

## Technical Considerations

### Cache Miss Handling

If the edit name cannot be retrieved from `ipfs_cache` for a Publish action, the action is considered invalid. This aligns with existing behavior where invalid edits are skipped.

### Ordering Guarantee

`hermes-ipfs-cache` runs AHEAD of `hermes-pipeline` (cache-ahead pattern). By the time hermes-pipeline processes a proposal, the edit should already be cached.

### Database Migration

- Add `name TEXT` to `ipfs_cache` table
- Add `name TEXT` to `proposals` table
- Existing proposals will have `NULL` name (acceptable - new proposals will have names)

## Acceptance Criteria

### Functional Requirements

- [ ] Proposals have a `name` field populated from action names
- [ ] Publish actions use the edit's name from IPFS cache
- [ ] Non-Publish actions use their action type as the name
- [ ] Multiple actions are concatenated with `, ` separator
- [ ] Names are truncated at 500 characters with `...`

### Technical Requirements

- [ ] `ipfs_cache.name` column added and populated during edit indexing
- [ ] `PublishAction.name` field added to proto
- [ ] `proposals.name` column added to schema
- [ ] kg-indexer populates `proposals.name` on proposal creation

## Implementation Phases

### Phase 1: Schema Changes

Add new columns and proto fields without changing behavior.

#### 1.1 ipfs_cache table schema

**File:** `api/src/services/storage/schema.ts` (lines 47-60)

```typescript
export const ipfsCache = pgTable("ipfs_cache", {
  id: serial(),
  data: bytea(),
  uri: text().notNull().unique(),
  isErrored: boolean().notNull().default(false),
  block: text().notNull(),
  space: uuid().notNull(),
  name: text(),  // ADD THIS
});
```

#### 1.2 proposals table schema

**File:** `api/src/services/storage/schema.ts` (lines 420-443)

```typescript
export const proposals = pgTable("proposals", {
  id: uuid().primaryKey(),
  spaceId: uuid("space_id").notNull().references(() => spaces.id),
  // ... existing fields ...
  createdAtBlock: text("created_at_block").notNull(),
  name: text(),  // ADD THIS
});
```

#### 1.3 PublishAction proto

**File:** `hermes-schema/proto/governance.proto` (lines 62-65)

```protobuf
message PublishAction {
  string content_uri = 1;  // Content URI (IPFS hash)
  bytes metadata = 2;      // Edit metadata
  string name = 3;         // ADD THIS - Edit name (from IPFS cache)
}
```

Run `cargo build` in hermes-schema to regenerate Rust types.

---

### Phase 2: ipfs-indexer Changes

Store edit name when caching.

#### 2.1 Update cache storage

**File:** `hermes-ipfs-cache/src/cache.rs` (lines 200-214)

Modify the INSERT query to include `name`:

```rust
// After decoding the edit to validate it, extract the name
let edit = grc_20::decode_edit(&bytes)?;
let name = edit.name.to_string();

// Update INSERT to include name
sqlx::query!(
    r#"
    INSERT INTO ipfs_cache (uri, data, block, space, is_errored, name)
    VALUES ($1, $2, $3, $4, $5, $6)
    ON CONFLICT (uri) DO NOTHING
    "#,
    uri,
    &bytes,
    block,
    space_id,
    false,
    name,  // ADD THIS
)
```

---

### Phase 3: hermes-pipeline Changes

Read name from cache and populate proto.

#### 3.1 Update CachedEdit struct

**File:** `hermes-pipeline/src/cache/mod.rs` (around line 55)

```rust
pub struct CachedEdit {
    pub cid: String,
    pub payload: Option<Vec<u8>>,
    pub is_errored: bool,
    pub space_id: Vec<u8>,
    pub name: Option<String>,  // ADD THIS
}
```

#### 3.2 Update PostgresCache query

**File:** `hermes-pipeline/src/cache/postgres.rs` (lines 37-74)

Update SELECT to include `name`:

```rust
let row = sqlx::query!(
    r#"SELECT data, space, is_errored, name FROM ipfs_cache WHERE uri = $1"#,
    ipfs_hash
)
.fetch_optional(&self.pool)
.await?;

// Include name in CachedEdit construction
CachedEdit {
    cid: ipfs_hash.to_string(),
    payload: row.data,
    is_errored: row.is_errored,
    space_id: row.space,
    name: row.name,  // ADD THIS
}
```

#### 3.3 Update governance pipeline

**File:** `hermes-pipeline/src/pipelines/governance.rs` (lines 263-269)

The governance pipeline needs access to the IPFS cache to look up edit names for Publish actions.

```rust
// For Publish actions, look up the edit name from cache
ProposalActionType::Publish => {
    let args = decode_publish_args(calldata).ok()?;
    
    // Look up edit name from cache
    let name = cache
        .get(&args.content_uri, space_id)
        .await
        .ok()
        .and_then(|cached| cached.name);
    
    Some(proposal_action::Action::Publish(PublishAction {
        content_uri: args.content_uri,
        metadata: args.metadata,
        name: name.unwrap_or_default(),  // Empty string if not found
    }))
}
```

**Note:** This requires passing the IPFS cache to the governance pipeline. The `transform` function signature will need to accept `&Arc<dyn IpfsCache>` similar to how `pipelines/edits.rs` does.

---

### Phase 4: kg-indexer Changes

Concatenate action names and store on proposal.

#### 4.1 Update ProposalItem model

**File:** `kg-indexer/src/models/governance.rs` (lines 51-66)

```rust
pub struct ProposalItem {
    pub id: Uuid,
    pub space_id: Uuid,
    pub proposed_by: Uuid,
    pub voting_mode: VotingMode,
    pub start_time: i64,
    pub end_time: i64,
    pub quorum: i64,
    pub threshold: i64,
    pub executed_at: Option<i64>,
    pub created_at: i64,
    pub created_at_block: i64,
    pub name: Option<String>,  // ADD THIS
}
```

#### 4.2 Update proposal handler

**File:** `kg-indexer/src/handlers/governance.rs`

Add function to derive proposal name from actions:

```rust
/// Derive proposal name from actions
fn derive_proposal_name(actions: &[ProposalAction]) -> Option<String> {
    if actions.is_empty() {
        return None;
    }
    
    let names: Vec<String> = actions
        .iter()
        .map(|action| action_to_name(action))
        .collect();
    
    let joined = names.join(", ");
    
    // Truncate at 500 chars (UTF-8 safe)
    Some(truncate_with_ellipsis(&joined, 500))
}

/// UTF-8 safe truncation with ellipsis
fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    
    // Find a valid UTF-8 boundary before max_len - 3 (for "...")
    let mut end = max_len - 3;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    
    format!("{}...", &s[..end])
}

fn action_to_name(action: &ProposalAction) -> String {
    match &action.action {
        Some(proposal_action::Action::AddMember(_)) => "Add Member".to_string(),
        Some(proposal_action::Action::RemoveMember(_)) => "Remove Member".to_string(),
        Some(proposal_action::Action::AddEditor(_)) => "Add Editor".to_string(),
        Some(proposal_action::Action::RemoveEditor(_)) => "Remove Editor".to_string(),
        Some(proposal_action::Action::UnflagEditor(_)) => "Unflag Editor".to_string(),
        Some(proposal_action::Action::Flag(_)) => "Flag".to_string(),
        Some(proposal_action::Action::Unflag(_)) => "Unflag".to_string(),
        Some(proposal_action::Action::UpdateVotingSettings(_)) => "Update Voting Settings".to_string(),
        Some(proposal_action::Action::Publish(p)) => {
            if p.name.is_empty() {
                "Publish".to_string()
            } else {
                p.name.clone()
            }
        }
        None => "Unknown Action".to_string(),
    }
}
```

Update `map_proposal_message` to call `derive_proposal_name`:

```rust
let name = derive_proposal_name(actions);

let proposal = ProposalItem {
    // ... existing fields ...
    name,
};
```

#### 4.3 Update storage layer

**File:** `kg-indexer/src/storage.rs` (lines 635-705)

Update `insert_proposals` to include `name` in the INSERT:

```sql
INSERT INTO proposals (
    id, space_id, proposed_by, voting_mode, 
    start_time, end_time, quorum, threshold, 
    created_at, created_at_block, name
) 
SELECT * FROM UNNEST(
    $1::uuid[], $2::uuid[], $3::uuid[], $4::text[],
    $5::bigint[], $6::bigint[], $7::bigint[], $8::bigint[],
    $9::text[], $10::text[], $11::text[]
)
```

---

## Success Metrics

- All new proposals have human-readable names
- Edit names correctly appear for Publish actions
- Action type names correctly appear for non-Publish actions
- Multiple actions are properly concatenated
- No regression in proposal processing performance

## Dependencies & Risks

### Dependencies

1. **Database migration** must be applied before deploying code changes
2. **hermes-schema** proto changes must be deployed to all services simultaneously
3. **ipfs-indexer** must be deployed before hermes-pipeline changes

### Risks

| Risk | Mitigation |
|------|------------|
| Proto incompatibility | Deploy all services together |
| Cache miss during transition | ipfs-indexer already runs ahead |
| Existing proposals have NULL names | Acceptable - only affects old data |

## References & Research

### Internal References

- Brainstorm: `docs/brainstorms/2026-02-03-proposal-names-brainstorm.md`
- ipfs_cache schema: `api/src/services/storage/schema.ts:47-60`
- proposals schema: `api/src/services/storage/schema.ts:420-443`
- PublishAction proto: `hermes-schema/proto/governance.proto:62-65`
- governance pipeline: `hermes-pipeline/src/pipelines/governance.rs:263-269`
- kg-indexer governance handler: `kg-indexer/src/handlers/governance.rs`

### Institutional Learnings Applied

- **Cache-ahead pattern**: ipfs-indexer runs ahead of hermes-pipeline
- **Nullable columns first**: Add columns as nullable, no backfill needed
- **Proto changes affect multiple services**: Deploy together
