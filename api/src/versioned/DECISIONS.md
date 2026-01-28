# Versioned Diffing: Design Decisions

This document captures implicit decisions, fallbacks, and defaults in the versioned diffing and grouping logic.

## Entity Name Resolution

**Decision:** Use name from "to" snapshot, fall back to "from" snapshot.

```typescript
name: getEntityName(to) ?? getEntityName(from)
```

**Why:** Ensures diff responses always include a displayable name when one exists in either version, even if the newer version deleted the name.

**Location:** `diff.ts:478`

## Value Type Determination

**Decision:** Default to `"TEXT"` when no typed value column is set.

```typescript
return "TEXT"; // Default fallback
```

**Why:** Provides a safe fallback for edge cases where a value exists but no type-specific column is populated.

**Location:** `diff.ts:87`

## Block Relations Filtering

**Decision:** BLOCKS relations are filtered out of the `relations` array.

```typescript
const relations = allRelations.filter((r) => r.typeId !== BLOCKS_TYPE_ID);
```

**Why:** Block relationships are represented in the `blocks` array instead, avoiding duplication. The diff consumer sees blocks as nested content, not as relations.

**Location:** `queries.ts:313, 334`

## Context-Based Entity Discovery

**Decision:** Entities with `null` contextEdgeTypeId are treated as BLOCKS (relation-based fallback).

```typescript
const typeId = entity.contextEdgeTypeId ?? fallbackTypeId;
```

**Why:** Backward compatibility. Data indexed before context support used BLOCKS relations for block discovery. The null context indicates relation-based discovery, which defaults to BLOCKS.

**Location:** `grouping.ts:69`

## Entity Deduplication

**Decision:** First occurrence wins when the same entity appears multiple times.

```typescript
if (seen.has(entity.entityId)) continue;
seen.add(entity.entityId);
```

**Why:** An entity might be discovered via both context metadata AND relation fallback. We keep the first occurrence (which has context info if available) and skip duplicates.

**Location:** `grouping.ts:63-64`

## Position-Based Ordering

**Decision:** Entities are sorted by position; null positions go last.

```typescript
const sorted = [...entities].sort((a, b) => {
  if (a.position === null && b.position === null) return 0;
  if (a.position === null) return 1;
  if (b.position === null) return -1;
  return a.position.localeCompare(b.position);
});
```

**Why:** Blocks have a defined order via the `position` field. Entities without position (e.g., discovered via context without position info) appear after positioned entities.

**Location:** `grouping.ts:54-60`

## Dynamic Group Keys Sorting

**Decision:** `groupKeys` array is sorted alphabetically.

```typescript
const groupKeys = Array.from(dynamicGroups.keys()).sort();
```

**Why:** Provides deterministic ordering for API consumers iterating over dynamic groups.

**Location:** `grouping.ts:80`

## Block Type Detection

**Decision:** Use "to" block's type, fall back to "from" block's type.

```typescript
const blockType = getBlockType(toBlock) ?? getBlockType(fromBlock ?? toBlock);
```

**Why:** Similar to name resolution - ensures we can determine block type even when the block was deleted in the newer version.

**Location:** `diff.ts:341`
