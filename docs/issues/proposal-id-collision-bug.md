# Proposal ID Collision Bug

## Status

Open

## Summary

Proposals are currently keyed only by `id` (the user-provided onchain proposal ID), but this ID is not globally unique. This allows cross-space collisions, same-space reuse attacks, and cross-space hijacking of proposals.

## The Problem

### 1. Cross-space collision

Different spaces can create proposals with the same ID. The second one overwrites the first.

- Space A creates proposal `0x0001`
- Space B creates proposal `0x0001`
- Space A's proposal is overwritten

### 2. Same-space reuse

A rejected proposal can be republished with the same ID, corrupting or merging data with the original.

### 3. Cross-space hijacking

A `PROPOSAL_UPDATED` event from Space B can hijack Space A's proposal because `update_proposal` only checks `WHERE id = $1` with no space validation.

```rust
// kg-indexer/src/storage.rs
let result = sqlx::query(
    r#"
    UPDATE proposals
    SET space_id = $2,
        proposed_by = $3,
        ...
    WHERE id = $1  // No space_id check!
    "#,
)
```

Space B can send a `PROPOSAL_UPDATED` for proposal `0x0001` and change its `space_id` to B, effectively stealing the proposal.

## Affected Code

### Schema (`api/src/services/storage/schema.ts`)

```typescript
export const proposals = pgTable(
    "proposals",
    {
        id: uuid().primaryKey(),  // Only id, not scoped to space
        spaceId: uuid("space_id").notNull().references(() => spaces.id),
        // ...
    }
)
```

### Indexer (`kg-indexer/src/storage.rs`)

```sql
-- insert_proposals: ON CONFLICT overwrites regardless of space
ON CONFLICT (id) DO UPDATE SET
    executed_at = COALESCE(EXCLUDED.executed_at, proposals.executed_at),
    name = COALESCE(EXCLUDED.name, proposals.name)

-- update_proposal: No space validation
UPDATE proposals SET ... WHERE id = $1
```

### Affected Tables

| Table | Issue |
|-------|-------|
| `proposals` | PK is just `id` |
| `proposal_actions` | FK references `proposals.id`, action IDs derived from `proposal_id + index` |
| `proposal_votes` | PK is `(proposalId, voterId)`, references non-scoped `proposals.id` |

## Constraints

- Users expect the indexed proposal ID to match the ID they provided onchain
- A KG entity will soon be created with the same UUID as the proposal ID
- Current API (`GET /proposals/:id/status`) assumes globally unique IDs

## Proposed Fixes

### Option A: Composite Primary Key

Change PK from `id` to `(space_id, id)`.

**Schema change:**
```typescript
export const proposals = pgTable(
    "proposals",
    {
        id: uuid().notNull(),
        spaceId: uuid("space_id").notNull().references(() => spaces.id),
        // ...
    },
    (table) => [
        primaryKey({columns: [table.spaceId, table.id]}),
    ]
)
```

**Pros:**
- Same proposal ID can exist in different spaces (if that's a valid use case)
- Clean relational model

**Cons:**
- Schema migration required for PK and all FKs
- API must change to require `spaceId` in all proposal lookups
- Breaks `GET /proposals/:id/status` endpoint

### Option B: First-Writer-Wins with Space Validation

Keep `id` as PK, but add space-scoped conflict handling in the indexer.

**Indexer change:**
```sql
-- PROPOSAL_CREATED: Only insert/update if space matches
INSERT INTO proposals (...)
ON CONFLICT (id) DO UPDATE SET
    executed_at = COALESCE(EXCLUDED.executed_at, proposals.executed_at),
    name = COALESCE(EXCLUDED.name, proposals.name)
WHERE proposals.space_id = EXCLUDED.space_id

-- PROPOSAL_UPDATED: Only update if space matches  
UPDATE proposals SET ... WHERE id = $1 AND space_id = $2
```

**Pros:**
- No schema changes
- No API changes
- `id` remains the user-provided proposal ID

**Cons:**
- Two spaces cannot have the same proposal ID (first writer wins globally)
- Silent failure if a collision occurs (may want to log/alert)

### Option C: Unique Constraint + Space Validation

Add a unique constraint on `(space_id, id)` while keeping `id` as PK, plus space validation in queries.

**Pros:**
- Database enforces uniqueness per space
- Allows same proposal ID in different spaces
- Clearer error on collision

**Cons:**
- Still requires API changes to lookup by `(spaceId, proposalId)`
- Unique constraint without composite PK is unusual

## Recommendation

**Option B** if same proposal IDs across different spaces is not a legitimate use case. Minimal changes, no API breakage.

**Option A** if spaces legitimately need independent proposal ID namespaces. Requires coordinated API/frontend changes.

## References

- `api/src/services/storage/schema.ts` - Proposal table definitions
- `kg-indexer/src/storage.rs` - Insert/update logic
- `kg-indexer/src/handlers/governance.rs` - Event handling
- `hermes-schema/proto/governance.proto` - Event definitions
