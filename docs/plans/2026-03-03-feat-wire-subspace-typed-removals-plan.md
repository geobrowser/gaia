---
title: Wire up typed subspace removal events
type: feat
date: 2026-03-03
---

# feat: Wire up typed subspace removal events

## Overview

The blockchain contract emits typed subspace removal actions (`SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, `SUBSPACE_TOPIC_REMOVED`) but the hermes-pipeline silently drops them. The deprecated `SUBSPACE_REMOVED` action loses its removal semantics at Kafka serialization, so the kg-indexer always inserts — never deletes. Additionally, the `subspaces` table has no `type` column, so only verified subspaces are stored, and topic-based subspace edges aren't stored at all.

This plan wires up all three typed removal actions end-to-end: proto schema → pipeline → Kafka → kg-indexer → database. It also restructures storage to properly model the two kinds of subspace relationships:

- **Explicit edges** (verified, related): space→space, stored in `subspaces` with a `type` column
- **Topic edges** (subtopic): space→topic, stored in a new `subspace_topics` table. Implicit membership is derived at query time via `spaces.topicId`.

## Problem Statement / Motivation

**Three classes of bugs exist today:**

1. **Silently dropped events**: `SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, and `SUBSPACE_TOPIC_REMOVED` actions are emitted by the contract but never matched in the pipeline's `transform()` function. These events vanish.

2. **Broken removal path**: The deprecated `SUBSPACE_REMOVED` action is matched but always produces a `VerifiedExtension` proto. The `TrustEvent.is_removal` flag is a Rust-only field that's never serialized to Kafka. The kg-indexer receives what looks like a verified addition and inserts a row — the opposite of the intended behavior.

3. **Missing type information**: The `subspaces` table only stores `(parent_space_id, child_space_id)` with no type discrimination. Related extensions are logged and discarded. Topic extensions are a fundamentally different relationship shape (space→topic, not space→space) but have nowhere to live.

**Atlas already handles all 8 actions correctly** (see `atlas/src/convert.rs` and `atlas/src/graph/state.rs`), modeling explicit edges and topic edges as separate data structures. This plan brings the kg-indexer in line with that approach.

## Proposed Solution

Use **proto-based signaling** with new `oneof` variants for removals. This makes messages self-describing and follows the codebase philosophy of encoding meaning in types rather than out-of-band headers.

### Data model

Atlas models two kinds of edges separately, and we follow the same pattern:

```
┌─────────────────────────────────────────────────────────┐
│ Explicit subspaces (space → space)                      │
│                                                         │
│ subspaces table:                                        │
│   parent_space_id  child_space_id  type                 │
│   ─────────────────────────────────────                 │
│   space_A          space_B         verified              │
│   space_A          space_C         related               │
│                                                         │
│ Events: SUBSPACE_VERIFIED / SUBSPACE_UNVERIFIED         │
│         SUBSPACE_RELATED  / SUBSPACE_UNRELATED          │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│ Topic subspaces (space → topic → implicit members)      │
│                                                         │
│ subspace_topics table:                                  │
│   space_id   topic_id                                   │
│   ─────────────────────                                 │
│   space_A    topic_T    ("space_A trusts topic_T")      │
│                                                         │
│ spaces table (existing):                                │
│   id         topicId                                    │
│   ─────────────────────                                 │
│   space_D    topic_T    ("space_D belongs to topic_T")  │
│   space_E    topic_T    ("space_E belongs to topic_T")  │
│                                                         │
│ Query: space_A's topic subspaces = {space_D, space_E}   │
│   SELECT s.id FROM subspace_topics st                   │
│   JOIN spaces s ON s.topic_id = st.topic_id             │
│   WHERE st.space_id = space_A                           │
│                                                         │
│ Events: SUBSPACE_TOPIC_DECLARED / SUBSPACE_TOPIC_REMOVED│
└─────────────────────────────────────────────────────────┘

Query for ALL subspaces of space_A:

  -- Explicit (verified + related)
  SELECT child_space_id, type FROM subspaces
  WHERE parent_space_id = space_A
  UNION ALL
  -- Implicit (via topics)
  SELECT s.id, 'topic' FROM subspace_topics st
  JOIN spaces s ON s.topic_id = st.topic_id
  WHERE st.space_id = space_A
```

### Changes by component:

#### 1. Protobuf schema (`hermes-schema/proto/space.proto`)

Add three removal variants to the `HermesSpaceTrustExtension` oneof:

```protobuf
// space.proto

message VerifiedRemoval {
    bytes target_space_id = 1;
}

message RelatedRemoval {
    bytes target_space_id = 1;
}

message SubtopicRemoval {
    bytes target_topic_id = 1;
}

message HermesSpaceTrustExtension {
    bytes source_space_id = 1;

    oneof extension {
        VerifiedExtension verified = 2;
        RelatedExtension related = 3;
        SubtopicExtension subtopic = 4;
        // Removal variants
        VerifiedRemoval verified_removal = 6;
        RelatedRemoval related_removal = 7;
        SubtopicRemoval subtopic_removal = 8;
    }

    blockchain_metadata.BlockchainMetadata meta = 5;
}
```

> Field numbers 6, 7, 8 — skip 5 since it's used by `meta`. Existing messages are wire-compatible because oneof variants with unknown field numbers are silently ignored by older consumers.

#### 2. Pipeline trust transform (`hermes-pipeline/src/pipelines/trust.rs`)

**Remove `TrustEvent` wrapper.** The `is_removal: bool` field on `TrustEvent` is the root cause of bug #2 — it's never serialized to Kafka, creating a dual source of truth. Now that the proto `oneof` variants encode removal semantics directly, `TrustEvent` is redundant. Replace `Vec<TrustEvent>` with `Vec<HermesSpaceTrustExtension>` in `TransformResult`.

> **Note:** `mark_sequence_as_last` and `max_sequence` at `main.rs:284,307` require the `HasMeta` trait. This will continue to work without changes because `HermesSpaceTrustExtension` already implements `HasMeta` via the macro in `hermes-schema/src/mod.rs:58-76`.

The debug log at `main.rs:494` currently reads `trust_event.is_removal`. Since `TrustEvent` is removed, drop the `is_removal` field from the log — the `extension_type` field (from `get_extension_type()`) already distinguishes additions from removals (e.g. `"verified"` vs `"verified_removal"`). No `is_removal()` helper needed.

**Add three new branches** in `transform()` for `SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, `SUBSPACE_TOPIC_REMOVED`. Add corresponding `convert_*` functions following the atlas pattern (`action.topic[16..32]` for target extraction). Each new convert function should include a byte layout comment at the top:

```rust
// action.topic layout: [subspace_id: 16 bytes | topic_id: 16 bytes]
```

**Add new counters** to `TransformResult`: `unverified`, `unrelated`, `topic_removed`. **Update `total()`** to include the new counters — this is critical because `trust.total() > 0` gates event emission at `main.rs:488`. Without this, blocks containing only typed removal events would be silently skipped.

```rust
pub fn total(&self) -> u64 {
    self.verified + self.related + self.topic_declared + self.removed
        + self.unverified + self.unrelated + self.topic_removed
}
```

**Deprecate `SUBSPACE_REMOVED` branch.** Keep the existing branch for backward compatibility but add a structured `warn!` with space IDs and an explicit code comment:

```rust
// BUG: SUBSPACE_REMOVED produces VerifiedExtension (an INSERT, not a DELETE).
// The removal signal is lost at Kafka serialization. Kept for backward
// compatibility until the contract stops emitting SUBSPACE_REMOVED.
// New code should use SUBSPACE_UNVERIFIED / SUBSPACE_UNRELATED / SUBSPACE_TOPIC_REMOVED.
} else if actions::matches(action_type, &actions::SUBSPACE_REMOVED) {
    warn!(
        source = %hex::encode(&action.from_id),
        target = %hex::encode(&action.topic[16..32]),
        "SUBSPACE_REMOVED is deprecated — removal will be treated as INSERT. \
         Use typed removal actions (SUBSPACE_UNVERIFIED, etc.) instead."
    );
    // ...existing convert_removed logic...
}
```

**Clean up stale `SUBSPACE_ADDED` doc comment.** The `transform()` doc comment at line 67 references `SUBSPACE_ADDED` but it's not matched in the function. Remove it from the doc comment. (`SUBSPACE_ADDED` is a deprecated action constant that's only used in governance/spaces pipeline test data — it's not handled by the trust pipeline.)

New convert functions produce `Extension::VerifiedRemoval`, `Extension::RelatedRemoval`, `Extension::SubtopicRemoval` variants respectively.

#### 3. Pipeline emit headers & `get_extension_type()` (mechanical)

The compiler will enforce updating the `extension-type` header match in `emit.rs` and `get_extension_type()` in `trust.rs` when the new proto variants are added. Add arms for the removal variants:

- `Extension::VerifiedRemoval(_)` → `"VERIFIED_REMOVAL"` / `"verified_removal"`
- `Extension::RelatedRemoval(_)` → `"RELATED_REMOVAL"` / `"related_removal"`
- `Extension::SubtopicRemoval(_)` → `"SUBTOPIC_REMOVAL"` / `"subtopic_removal"`

#### 4. Pipeline main.rs metrics

Add the new counters to `counts_by_event_type` in the block summary logging:

```rust
counts_by_event_type.insert("SUBSPACE_UNVERIFIED".to_string(), trust.unverified as u64);
counts_by_event_type.insert("SUBSPACE_UNRELATED".to_string(), trust.unrelated as u64);
counts_by_event_type.insert("SUBSPACE_TOPIC_REMOVED".to_string(), trust.topic_removed as u64);
```

Update the trust event emission loop debug log: remove the `is_removal` field (which came from the now-deleted `TrustEvent` wrapper). The `extension_type` field already distinguishes additions from removals.

#### 5. Database schema (`api/src/services/storage/schema.ts`)

> **Important:** All schema changes must be made in `api/src/services/storage/schema.ts` and migrations generated via Drizzle (`drizzle-kit generate` / `drizzle-kit migrate`). Do NOT write raw SQL migration files by hand — Drizzle is the source of truth for this project's schema.

##### 5a. Update `subspaces` table — add `type` column for explicit edges

Add a `subspaceType` enum and column. The `subspaces` table stores space→space edges only (verified and related):

```typescript
export const subspaceTypeEnum = pgEnum("subspaceType", ["verified", "related"])

export const subspaces = pgTable(
    "subspaces",
    {
        parentSpaceId: uuid().notNull(),
        childSpaceId: uuid().notNull(),
        type: subspaceTypeEnum().notNull().default("verified"),
    },
    (table) => [
        primaryKey({columns: [table.parentSpaceId, table.childSpaceId, table.type]}),
        index("subspaces_parent_space_id_idx").on(table.parentSpaceId),
        index("subspaces_child_space_id_idx").on(table.childSpaceId),
    ],
)
```

**Migration:**
1. Add `subspaceType` enum and `type` column as `NOT NULL DEFAULT 'verified'` — all existing rows get `'verified'` which is correct since the handler only stored verified extensions. On PostgreSQL 11+, `ADD COLUMN ... NOT NULL DEFAULT` is a metadata-only operation (no table rewrite).
2. Drop old PK, create new PK on `(parent_space_id, child_space_id, type)`. This requires an `ACCESS EXCLUSIVE` lock, but the table is small (only verified subspaces stored today).

**Pre-migration safety check:** Run `SELECT count(*) FROM subspaces` before executing. If unexpectedly large (>100k rows), consider using `CREATE UNIQUE INDEX CONCURRENTLY` + `ALTER TABLE ... ADD CONSTRAINT ... USING INDEX` to minimize lock time.

##### 5b. New `subspace_topics` table — topic edges

A separate table for space→topic edges. This models "space A trusts topic T" — a fundamentally different relationship from space→space edges. Implicit membership is derived at query time by joining `subspace_topics.topic_id` → `spaces.topicId`.

```typescript
export const subspaceTopics = pgTable(
    "subspace_topics",
    {
        spaceId: uuid("space_id").notNull(),
        topicId: uuid("topic_id").notNull(),
    },
    (table) => [
        primaryKey({columns: [table.spaceId, table.topicId]}),
        index("subspace_topics_space_id_idx").on(table.spaceId),
        index("subspace_topics_topic_id_idx").on(table.topicId),
    ],
)

export const subspaceTopicsRelations = drizzleRelations(subspaceTopics, ({one}) => ({
    space: one(spaces, {
        fields: [subspaceTopics.spaceId],
        references: [spaces.id],
        relationName: "subspaceTopicSpace",
    }),
}))
```

> **Topic→members resolution is a raw join, not a Drizzle relation.** The core query `subspace_topics.topic_id → spaces.topicId` is a non-PK to non-PK join, which Drizzle's relational API (`db.query.*.findMany({ with })`) can't express. Use Drizzle's select API with an explicit join instead: `db.select().from(subspaceTopics).leftJoin(spaces, eq(subspaceTopics.topicId, spaces.topicId))`. This matches atlas's approach where topic member resolution is a dedicated method, not a generic ORM traversal. Also update `spacesRelations` to include the reverse `many(subspaceTopics)` for the `spaceId` direction.

**Migration:** Simple `CREATE TABLE` — no existing data to worry about.

**Query pattern for all subspaces of a space:**

```sql
-- Explicit subspaces (verified + related)
SELECT child_space_id, type FROM subspaces
WHERE parent_space_id = $1
UNION ALL
-- Implicit subspaces via topics
SELECT s.id, 'topic' FROM subspace_topics st
JOIN spaces s ON s.topic_id = st.topic_id
WHERE st.space_id = $1
```

#### 6. KG-indexer model (`kg-indexer/src/models/subspaces.rs`)

Two model types — one for explicit edges, one for topic edges:

```rust
/// Type of explicit subspace relationship (space → space)
pub enum SubspaceType {
    Verified,
    Related,
}

/// An explicit subspace edge (verified or related)
pub struct SubspaceItem {
    pub subspace_id: Uuid,
    pub parent_space_id: Uuid,
    pub subspace_type: SubspaceType,
}

/// A topic subspace edge (space → topic)
pub struct SubspaceTopicItem {
    pub space_id: Uuid,
    pub topic_id: Uuid,
}
```

The handler returns an enum that distinguishes all four operations:

```rust
pub enum SubspaceChange {
    /// Insert an explicit edge (verified/related)
    InsertExplicit(SubspaceItem),
    /// Remove an explicit edge (verified/related)
    RemoveExplicit(SubspaceItem),
    /// Insert a topic edge
    InsertTopic(SubspaceTopicItem),
    /// Remove a topic edge
    RemoveTopic(SubspaceTopicItem),
}
```

#### 7. KG-indexer handler (`kg-indexer/src/handlers/subspaces.rs`)

Return `Result<Option<SubspaceChange>, HandlerError>`. Every known variant maps to `Some(change)`, but `None` extension returns `Ok(None)` with a warning (for rolling deployment resilience — see §9):

```rust
pub fn handle_trust_extension(
    event: &HermesSpaceTrustExtension,
) -> Result<Option<SubspaceChange>, HandlerError> {
    let source_space_id = Uuid::from_slice(&event.source_space_id)?;

    match &event.extension {
        // Explicit additions (space → space)
        Some(Extension::Verified(v)) => {
            let target = Uuid::from_slice(&v.target_space_id)?;
            Ok(SubspaceChange::InsertExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Verified,
            }))
        }
        Some(Extension::Related(r)) => {
            let target = Uuid::from_slice(&r.target_space_id)?;
            Ok(SubspaceChange::InsertExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Related,
            }))
        }
        // Topic additions (space → topic)
        Some(Extension::Subtopic(s)) => {
            let topic = Uuid::from_slice(&s.target_topic_id)?;
            Ok(SubspaceChange::InsertTopic(SubspaceTopicItem {
                space_id: source_space_id,
                topic_id: topic,
            }))
        }
        // Explicit removals (space → space)
        Some(Extension::VerifiedRemoval(v)) => {
            let target = Uuid::from_slice(&v.target_space_id)?;
            Ok(SubspaceChange::RemoveExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Verified,
            }))
        }
        Some(Extension::RelatedRemoval(r)) => {
            let target = Uuid::from_slice(&r.target_space_id)?;
            Ok(SubspaceChange::RemoveExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Related,
            }))
        }
        // Topic removals (space → topic)
        Some(Extension::SubtopicRemoval(s)) => {
            let topic = Uuid::from_slice(&s.target_topic_id)?;
            Ok(SubspaceChange::RemoveTopic(SubspaceTopicItem {
                space_id: source_space_id,
                topic_id: topic,
            }))
        }
        None => {
            warn!("Received trust extension with no extension variant (unknown proto field?)");
            Ok(None)
        }
    }
}
```

**Add observability logging** for all paths (additions and removals) with structured fields including the change type.

#### 8. KG-indexer storage (`kg-indexer/src/storage.rs`)

**Update existing functions** for `subspaces` table (now includes `type` column).

> **Enum name:** After running `drizzle-kit generate`, check the generated migration SQL for the exact Postgres enum name (e.g., `"subspaceType"` or `subspace_type`) and use that name exactly in the `storage.rs` casts below. sqlx compile-time checks will catch a mismatch.

```sql
-- insert_subspaces: add type
INSERT INTO subspaces (child_space_id, parent_space_id, type)
SELECT child_space_id, parent_space_id, type
FROM UNNEST($1::uuid[], $2::uuid[], $3::"subspaceType"[])
AS t(child_space_id, parent_space_id, type)
ON CONFLICT (parent_space_id, child_space_id, type) DO NOTHING

-- remove_subspaces: type-aware DELETE
DELETE FROM subspaces
WHERE (child_space_id, parent_space_id, type) IN (
    SELECT child_space_id, parent_space_id, type
    FROM UNNEST($1::uuid[], $2::uuid[], $3::"subspaceType"[])
    AS t(child_space_id, parent_space_id, type)
)
```

**Add new functions** for `subspace_topics` table:

```sql
-- insert_subspace_topics
INSERT INTO subspace_topics (space_id, topic_id)
SELECT space_id, topic_id
FROM UNNEST($1::uuid[], $2::uuid[])
AS t(space_id, topic_id)
ON CONFLICT (space_id, topic_id) DO NOTHING

-- remove_subspace_topics
DELETE FROM subspace_topics
WHERE (space_id, topic_id) IN (
    SELECT space_id, topic_id
    FROM UNNEST($1::uuid[], $2::uuid[])
    AS t(space_id, topic_id)
)
```

**Return `rows_affected()`** from both remove functions for observability.

Remove the `#[allow(dead_code)]` from `remove_subspaces()`.

#### 9. KG-indexer main dispatch (`kg-indexer/src/main.rs`)

**Both dispatch sites** must be updated. There are two `TrustExtension` match arms:
- `process_message()` at line ~1083 (non-buffered path)
- Block-buffered path at line ~1446

Update both to route all four change types:

```rust
KgMessage::TrustExtension(trust_event) => {
    // Record trace context
    let extension_type = match &trust_event.extension {
        Some(TrustExtensionType::Verified(_)) => "verified",
        Some(TrustExtensionType::Related(_)) => "related",
        Some(TrustExtensionType::Subtopic(_)) => "subtopic",
        Some(TrustExtensionType::VerifiedRemoval(_)) => "verified_removal",
        Some(TrustExtensionType::RelatedRemoval(_)) => "related_removal",
        Some(TrustExtensionType::SubtopicRemoval(_)) => "subtopic_removal",
        None => "unknown",
    };
    event_span.record("extension_type", extension_type);

    let change = handlers::subspaces::handle_trust_extension(trust_event)?;
    match &change {
        SubspaceChange::InsertExplicit(item) => {
            event_span.record("parent_space_id", display(item.parent_space_id));
            event_span.record("child_space_id", display(item.subspace_id));
            storage.insert_subspaces(&[item], &mut tx).await?;
        }
        SubspaceChange::RemoveExplicit(item) => {
            event_span.record("parent_space_id", display(item.parent_space_id));
            event_span.record("child_space_id", display(item.subspace_id));
            storage.remove_subspaces(&[item], &mut tx).await?;
        }
        SubspaceChange::InsertTopic(item) => {
            event_span.record("space_id", display(item.space_id));
            event_span.record("topic_id", display(item.topic_id));
            storage.insert_subspace_topics(&[item], &mut tx).await?;
        }
        SubspaceChange::RemoveTopic(item) => {
            event_span.record("space_id", display(item.space_id));
            event_span.record("topic_id", display(item.topic_id));
            storage.remove_subspace_topics(&[item], &mut tx).await?;
        }
    }
    1
}
```

> **Rolling deployment safety:** When an old kg-indexer receives new proto variants it doesn't know about, protobuf silently drops the unknown `oneof` field, so `extension = None`. Today the handler returns `Err(HandlerError::MissingPayload)`, which propagates as a hard error that fails the entire block transaction — the consumer retries the block indefinitely. To avoid this, **change the `None` arm** from `Err(HandlerError::MissingPayload)` to a `warn!` log + skip (return `Ok(None)` or equivalent). This makes the kg-indexer resilient to unknown extension variants during rolling deploys. The handler return type stays `Result<SubspaceChange>` but the dispatch site wraps it in an `Option` check.

#### 10. Documentation (`hermes-pipeline/docs/action-data-mapping.md`)

Update the Trust/Topology section to:
- Add `SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, `SUBSPACE_TOPIC_REMOVED` rows with their proto removal variants
- Mark `SUBSPACE_ADDED` and `SUBSPACE_REMOVED` as deprecated
- Document the new proto removal variants and their `extension-type` header values
- Document the `extension-type` header values for removals (`VERIFIED_REMOVAL`, `RELATED_REMOVAL`, `SUBTOPIC_REMOVAL`)

## Technical Considerations

### Wire compatibility
Adding new `oneof` variants (field numbers 6, 7, 8) is backward-compatible per protobuf spec. Older consumers that don't know about these variants will see `extension = None` and skip the event (which is the current behavior for unknown types in the kg-indexer handler — it returns `MissingPayload` error). **Deploy the kg-indexer first** so it understands the new variants before the pipeline starts emitting them.

### Deployment order
1. **Proto schema** — commit new `.proto` + regenerate (must be first per GOTCHAS.md)
2. **Schema migration** — add `type` column to `subspaces`, create `subspace_topics` table
3. **KG-indexer** — deploy with new handler (understands new proto variants, writes to both tables)
4. **Pipeline** — deploy with new `transform()` branches (starts emitting removal variants)

This order ensures no component sees data it can't handle. During the kg-indexer→pipeline rollout window, the new kg-indexer receives only old-format messages (which it handles correctly — they all map to `SubspaceChange::InsertExplicit` with `SubspaceType::Verified`, same as before).

### Why separate tables for explicit and topic edges

Topic edges are semantically different from explicit edges:
- **Explicit edges** (verified, related) are space→space relationships stored directly.
- **Topic edges** are space→topic relationships. The implicit members are derived at query time by joining `subspace_topics.topic_id → spaces.topicId`.

Atlas models these as separate data structures for the same reason (`explicit_edges` vs `topic_edges` in `graph/state.rs`). Combining them in one table would either require a semantically misleading `child_space_id` column that sometimes holds topic IDs, or nullable columns where exactly one is populated depending on type.

### Idempotency
- `insert_subspaces()` and `insert_subspace_topics()` use `ON CONFLICT DO NOTHING` — safe for replays
- `remove_subspaces()` and `remove_subspace_topics()` are DELETEs of specific tuples — no-op if row doesn't exist, safe for replays
- Single-partition Kafka topic guarantees ordering within a consumer group
- `BlockchainMetadata` carries block number for observability, but explicit sequence-based deduplication is not required given the at-least-once + idempotent operations design

### Legacy `SUBSPACE_REMOVED`
Keep the existing branch with a structured `warn!` log including space IDs and an explicit code comment documenting the known bug. **Do NOT change `SUBSPACE_REMOVED` to emit a removal variant.** It continues to emit `VerifiedExtension` (insert behavior). The only change is adding the deprecation warning. This is acceptable because:
- The contract is transitioning to typed removals
- Once the contract stops emitting `SUBSPACE_REMOVED`, the branch becomes dead code
- A follow-up can remove it

### Historical data
After deployment, only *new* Related and Subtopic events will be stored. Historical Related/Subtopic events that were previously dropped are lost. If historical completeness is needed, a reindex from the blockchain would be required. This is acceptable for the initial rollout.

## Acceptance Criteria

### Proto
- [ ] `space.proto` has `VerifiedRemoval`, `RelatedRemoval`, `SubtopicRemoval` message types in the `HermesSpaceTrustExtension` oneof

### Pipeline
- [ ] `TrustEvent` wrapper removed — `TransformResult.events` is `Vec<HermesSpaceTrustExtension>` directly
- [ ] `transform()` matches `SUBSPACE_UNVERIFIED`, `SUBSPACE_UNRELATED`, `SUBSPACE_TOPIC_REMOVED` and produces the corresponding removal proto variants
- [ ] New convert functions have byte layout comment (`// action.topic layout: [subspace_id: 16 bytes | topic_id: 16 bytes]`)
- [ ] Debug log in emission loop drops `is_removal` field, relies on `extension_type` only
- [ ] `TransformResult` has `unverified`, `unrelated`, `topic_removed` counters; `total()` includes all 7 counters
- [ ] `emit.rs` headers include `VERIFIED_REMOVAL`, `RELATED_REMOVAL`, `SUBTOPIC_REMOVAL` values
- [ ] Unit tests for all 3 new convert functions + updated `test_transform_counts`
- [ ] Stale `SUBSPACE_ADDED` reference removed from `transform()` doc comment
- [ ] Deprecated `SUBSPACE_REMOVED` branch has structured `warn!` with space IDs and explicit BUG comment

### Schema
- [ ] `subspaces` table has a `type` column (enum: `verified`, `related`) with `DEFAULT 'verified'`
- [ ] `subspaces` PK is `(parent_space_id, child_space_id, type)`
- [ ] New `subspace_topics` table with `(space_id, topic_id)` PK and indexes on both columns
- [ ] Drizzle relations defined for `subspace_topics`

### KG-indexer
- [ ] Handler returns `Result<Option<SubspaceChange>>` with four variants: `InsertExplicit`, `RemoveExplicit`, `InsertTopic`, `RemoveTopic`; `None` extension logs a warning and skips (rolling deploy resilience)
- [ ] `Related` additions stored in `subspaces` with `type = 'related'`
- [ ] `Subtopic` additions stored in `subspace_topics`
- [ ] Removal events call the appropriate remove function with correct table and type
- [ ] Both remove functions return `rows_affected` for observability; `remove_subspaces()` no longer has `#[allow(dead_code)]`
- [ ] **Both** dispatch sites updated (`process_message` ~line 1083 AND block-buffered ~line 1446)
- [ ] `extension_type` tracing match updated with removal variant arms in both dispatch sites
- [ ] Handler has `debug!` logging for all insert/remove paths with structured fields

### Docs
- [ ] `action-data-mapping.md` lists all 8 subspace actions with correct proto mappings and deprecation notes

### Tests
- [ ] Integration test: insert a verified subspace, then unverify it, verify row is gone
- [ ] Integration test: insert verified + related for same pair, remove one, verify the other survives
- [ ] Integration test: insert a topic edge, then remove it, verify row is gone from `subspace_topics`

## Success Metrics

- Zero silently dropped subspace events (all 8 action types produce Kafka messages or explicit logs)
- Subspace removals reflected in the database (verified by querying after an unverify action)
- Explicit (verified, related) and topic subspace relationships queryable from their respective tables
- Remove functions log `rows_affected` for operational debugging

## Dependencies & Risks

| Dependency | Risk | Mitigation |
|---|---|---|
| Proto change must land first | Build breaks if Rust references new variants before proto is generated | Commit proto changes in a separate commit before pipeline/kg-indexer code |
| Schema migration on live DB | PK change on `subspaces` requires `ACCESS EXCLUSIVE` lock | Table is small (only verified subspaces stored today). Run `SELECT count(*)` pre-migration. If >100k rows, use `CREATE UNIQUE INDEX CONCURRENTLY` approach. PostgreSQL 11+ makes `ADD COLUMN NOT NULL DEFAULT` metadata-only. |
| New `subspace_topics` table | New table creation is non-blocking | Simple `CREATE TABLE`, no risk |
| Rolling deployment window | Old kg-indexer receives new proto variants → `MissingPayload` error | Deploy kg-indexer before pipeline. Verify `MissingPayload` is handled gracefully (not a consumer crash). |
| SQL enum type cast | `$3::text[]` won't work with a pgEnum column | Use `$3::"subspaceType"[]` in UNNEST. Verify with sqlx compile-time checks. |
| Historical data gap | Previous Related/Subtopic events were dropped, not stored | Acceptable for initial rollout. Reindex from blockchain if historical completeness needed. |

## References & Research

### Internal References
- Atlas graph state (explicit + topic edges): `atlas/src/graph/state.rs:24-43`
- Atlas topic edge add/remove: `atlas/src/graph/state.rs:104-105, 127-128, 157-183`
- Atlas transitive topic resolution: `atlas/src/graph/transitive.rs:314-330`
- Atlas removal handling (reference impl): `atlas/src/convert.rs:212-263`
- Atlas events enum: `atlas/src/events.rs:68-93`
- Pipeline trust transform: `hermes-pipeline/src/pipelines/trust.rs:71-131`
- Pipeline emit headers: `hermes-pipeline/src/emit.rs:145-165`
- KG-indexer subspace handler: `kg-indexer/src/handlers/subspaces.rs:15-51`
- KG-indexer storage (insert + dead remove): `kg-indexer/src/storage.rs:576-634`
- KG-indexer main dispatch (both sites): `kg-indexer/src/main.rs:1083-1090` and `kg-indexer/src/main.rs:1446-1466`
- KG-indexer `extension_type` tracing match: `kg-indexer/src/main.rs:1448-1453`
- Action constants: `hermes-relay/src/actions.rs:54-61`
- Proto schema: `hermes-schema/proto/space.proto:52-74`
- DB schema: `api/src/services/storage/schema.ts:265-276`
- Spaces table `topicId` column: `api/src/services/storage/schema.ts:104`
- Membership pattern (reference for add/remove dispatch): `kg-indexer/src/main.rs:1425-1444`

### Institutional Learnings
- Proto changes must be committed before Rust code referencing them (`hermes-pipeline/docs/GOTCHAS.md`)
- Processing order: spaces → membership → trust → moderation → topics → governance → voting → edits
- `is_last` flag / block buffering semantics (`kg-indexer/docs/GOTCHAS.md`)
- `ON CONFLICT DO NOTHING` for idempotent inserts (existing pattern in `storage.rs`)
