# Versioned Diffing Specification

This document specifies the data model, algorithms, and design decisions for the versioned entity diffing system.

## Overview

The versioned diffing system computes differences between entity snapshots at different versions (edits). It supports:

- **Temporal versioning**: Query entity state at any historical edit
- **Value diffing**: Word-level text diffs and before/after comparisons for other types
- **Relation diffing**: ADD/REMOVE/UPDATE change detection
- **Block diffing**: Nested entity changes with type-aware formatting
- **Context-aware grouping**: Organize changes by relation type for inline rendering

## Data Model

### Version Keys

Versions are identified by edits, with ordering derived from chain metadata:

```
edit_versions
├── edit_id (uuid, PK)     -- GRC-20 edit identifier
├── block_number (bigint)   -- Blockchain block number
├── sequence (int)          -- Block-local ordering
├── created_at (timestamptz)
└── version_key (bigint)    -- Packed ordering key: (block_number << 32) | sequence
```

The `version_key` provides a single sortable value for temporal range queries.

### Value Versions

Values use temporal validity ranges for point-in-time queries:

```
value_versions
├── entity_id (uuid)
├── property_id (uuid)
├── space_id (uuid)
├── valid_from_key (bigint)     -- Version when value became active
├── valid_to_key (bigint, null) -- Version when value was superseded (null = current)
│
├── -- GRC-20 v2 data type columns (exactly one set per row)
├── text (text)
├── boolean (boolean)
├── integer (bigint)
├── float (double precision)
├── decimal (numeric)
├── bytes (bytea)
├── date (date)
├── time (time with time zone)
├── datetime (timestamptz)
├── schedule (jsonb)
├── point (text)            -- "lon,lat" or "lon,lat,alt"
├── embedding (jsonb)
│
├── -- Metadata
├── language (uuid)         -- For TEXT values
├── unit (uuid)             -- For numerical values
│
└── -- Context metadata (for grouping)
    ├── context_root_id (uuid)      -- Parent entity in edit context
    └── context_edge_type_id (uuid) -- Relation type from context edge
```

### Relation Versions

Relations also use temporal validity ranges:

```
relation_versions
├── relation_id (uuid, PK)
├── type_id (uuid)          -- Relation type (e.g., BLOCKS)
├── from_entity_id (uuid)
├── to_entity_id (uuid)
├── space_id (uuid)
├── position (text, null)   -- Fractional indexing for ordering
├── valid_from_key (bigint)
├── valid_to_key (bigint, null)
│
├── -- Optional fields
├── from_space_id (uuid)
├── to_space_id (uuid)
├── verified (boolean)
│
└── -- Context metadata
    ├── context_root_id (uuid)
    └── context_edge_type_id (uuid)
```

### Context Columns

Context metadata enables change grouping for inline rendering. Added via migration:

```sql
ALTER TABLE value_versions ADD COLUMN context_root_id uuid;
ALTER TABLE value_versions ADD COLUMN context_edge_type_id uuid;
ALTER TABLE relation_versions ADD COLUMN context_root_id uuid;
ALTER TABLE relation_versions ADD COLUMN context_edge_type_id uuid;
```

Context is extracted from GRC-20 edit operations:

```rust
// GRC-20 context structure
Context {
    root_id: Id,              // Parent entity (e.g., "Byron" page)
    edges: Vec<ContextEdge>,  // Path to changed entity
}

ContextEdge {
    type_id: Id,              // Relation type (e.g., BLOCKS)
    to_entity_id: Id,         // Target entity (e.g., TextBlock_9)
}
```

The indexer extracts `(root_id, first_edge_type_id)` from context and stores it with each value/relation version.

## Snapshot Computation

### Point-in-Time Query

To compute an entity snapshot at a version:

```sql
-- Values active at version_key
SELECT * FROM value_versions
WHERE entity_id = $1
  AND valid_from_key <= $version_key
  AND (valid_to_key IS NULL OR valid_to_key > $version_key)

-- Relations active at version_key
SELECT * FROM relation_versions
WHERE from_entity_id = $1
  AND valid_from_key <= $version_key
  AND (valid_to_key IS NULL OR valid_to_key > $version_key)
```

### Entity Snapshot Structure

```typescript
interface EntitySnapshot {
    id: string;
    values: VersionedValue[];
    relations: VersionedRelation[];  // Excludes block relations
    blocks: BlockSnapshot[];
}

interface BlockSnapshot {
    id: string;
    values: VersionedValue[];
    relations: VersionedRelation[];
}
```

### Grouped Entity Snapshot (Hybrid Mode)

For context-aware responses:

```typescript
interface GroupedEntitySnapshot {
    id: string;
    values: VersionedValue[];
    relations: VersionedRelation[];  // Excludes grouped relations
    blocks: BlockSnapshot[];         // Static key for BLOCKS type
    groupKeys: string[];             // Dynamic keys present (sorted)
    groups: Record<string, BlockSnapshot[]>;  // Dynamic groups by type ID
}
```

## Diffing Algorithms

### Value Diffing

Values are keyed by `(propertyId, spaceId)`. For each key:

1. **Added**: Key exists in `after` but not `before`
2. **Removed**: Key exists in `before` but not `after`
3. **Changed**: Key exists in both with different values

**Text values** include both raw strings and word-level diff:

```typescript
interface TextValueChange {
    propertyId: string;
    spaceId: string;
    type: "TEXT";
    before: string | null;
    after: string | null;
    diff: DiffChunk[];  // [{value, added?, removed?}]
}
```

**Other value types** use simple before/after comparison:

```typescript
interface SimpleValueChange {
    propertyId: string;
    spaceId: string;
    type: "BOOL" | "INT64" | "FLOAT64" | "DECIMAL" | ...;
    before: string | null;
    after: string | null;
}
```

### Relation Diffing

Relations are keyed by `relationId`. Change types:

- **ADD**: Relation exists in `after` but not `before`
- **REMOVE**: Relation exists in `before` but not `after`
- **UPDATE**: Same relation ID with changed `toEntityId`, `toSpaceId`, or `position`

```typescript
interface RelationChange {
    relationId: string;
    typeId: string;
    spaceId: string;
    changeType: "ADD" | "REMOVE" | "UPDATE";
    before?: { toEntityId, toSpaceId?, position? } | null;
    after?: { toEntityId, toSpaceId?, position? } | null;
}
```

### Block Diffing

Blocks are keyed by `id`. Block type determines diff format:

**Text blocks**: Raw strings and word-level diff of markdown content
```typescript
interface TextBlockChange {
    id: string;
    type: "textBlock";
    before: string | null;
    after: string | null;
    diff: DiffChunk[];
}
```

**Image blocks**: Before/after URL comparison
```typescript
interface ImageBlockChange {
    id: string;
    type: "imageBlock";
    before: string | null;
    after: string | null;
}
```

**Data blocks**: Before/after name comparison
```typescript
interface DataBlockChange {
    id: string;
    type: "dataBlock";
    before: string | null;
    after: string | null;
}
```

Block type is determined by checking the `TYPES_PROPERTY` relation:
- `toEntityId === TEXT_BLOCK` → textBlock
- `toEntityId === IMAGE_BLOCK` or `IMAGE` → imageBlock
- `toEntityId === DATA_BLOCK` → dataBlock

## Context-Aware Grouping

### Entity Discovery

Entities related to a root entity are discovered via two methods:

1. **Context-based discovery**: Query entities where `context_root_id = rootEntityId`
2. **Relation-based fallback**: Query entities via BLOCKS relations (backward compatibility)

```sql
-- Context-based discovery
SELECT DISTINCT entity_id, context_edge_type_id
FROM value_versions
WHERE context_root_id = $entity_id
  AND context_edge_type_id IS NOT NULL
  AND valid_from_key <= $version_key
  AND (valid_to_key IS NULL OR valid_to_key > $version_key)

UNION

SELECT DISTINCT entity_id, context_edge_type_id
FROM relation_versions
WHERE context_root_id = $entity_id
  AND context_edge_type_id IS NOT NULL
  AND valid_from_key <= $version_key
  AND (valid_to_key IS NULL OR valid_to_key > $version_key)
```

### Grouping Algorithm

Two passes: dedupe-then-sort. Context discovery wins over BLOCKS-relation fallback so the RFC's `edges[0].type_id` grouping takes precedence when both discovery paths surface the same entity. Diffs that lack context metadata go through the pure-fallback path and behave identically to pre-RFC behavior.

```typescript
function groupEntitiesByContext(
    entities: DiscoveredEntity[],
    fallbackTypeId: string = BLOCKS
): GroupedEntities {
    // Pass 1: dedupe. Prefer the entry that carries a contextEdgeTypeId
    // (context discovery) over a null-context entry (BLOCKS fallback), and
    // inherit `position` from whichever entry has one.
    const deduped = new Map<string, DiscoveredEntity>();
    for (const entity of entities) {
        const existing = deduped.get(entity.entityId);
        if (!existing) {
            deduped.set(entity.entityId, entity);
            continue;
        }
        deduped.set(entity.entityId, {
            entityId: entity.entityId,
            contextEdgeTypeId: existing.contextEdgeTypeId ?? entity.contextEdgeTypeId,
            position: existing.position ?? entity.position,
        });
    }

    // Pass 2: sort by position (nulls last) and route into buckets.
    const sorted = Array.from(deduped.values()).sort((a, b) => {
        if (a.position === null && b.position === null) return 0;
        if (a.position === null) return 1;
        if (b.position === null) return -1;
        return a.position.localeCompare(b.position);
    });

    const blocks: string[] = [];
    const dynamicGroups = new Map<string, string[]>();

    for (const entity of sorted) {
        // null contextEdgeTypeId means fallback discovery only.
        const typeId = entity.contextEdgeTypeId ?? fallbackTypeId;

        if (typeId === BLOCKS) {
            blocks.push(entity.entityId);
        } else {
            const group = dynamicGroups.get(typeId) ?? [];
            group.push(entity.entityId);
            dynamicGroups.set(typeId, group);
        }
    }

    const groupKeys = Array.from(dynamicGroups.keys()).sort();
    return { blocks, dynamicGroups, groupKeys };
}
```

### Grouped Diff Response

```typescript
interface GroupedEntityDiff {
    entityId: string;
    name: string | null;
    values: ValueChange[];
    relations: RelationChange[];     // Excludes grouped relation types
    blocks: BlockChange[];           // Static key for BLOCKS
    groupKeys: string[];             // Only keys with changes (sorted)
    groups: Record<string, BlockChange[]>;
}
```

**Key behaviors**:
- `groupKeys` only includes groups that have actual changes
- Relations whose type is used for grouping are filtered from `relations[]`
- Dynamic groups use the same `BlockChange` types as `blocks[]`

## Design Decisions

### Entity Name Resolution

**Decision**: Use name from "to" snapshot, fall back to "from" snapshot.

```typescript
name: getEntityName(to) ?? getEntityName(from)
```

**Rationale**: Ensures diff responses always include a displayable name when one exists in either version, even if the newer version deleted the name.

### Value Type Determination

**Decision**: Default to `"TEXT"` when no typed value column is set.

```typescript
return "TEXT"; // Default fallback
```

**Rationale**: Provides a safe fallback for edge cases where a value exists but no type-specific column is populated.

### Block Relations Filtering

**Decision**: BLOCKS relations are filtered out of the `relations` array.

```typescript
const relations = allRelations.filter((r) => r.typeId !== BLOCKS_TYPE_ID);
```

**Rationale**: Block relationships are represented in the `blocks` array instead, avoiding duplication. The diff consumer sees blocks as nested content, not as relations.

### Context Fallback for Null Edge Type

**Decision**: Entities with `null` contextEdgeTypeId are treated as BLOCKS (relation-based fallback).

```typescript
const typeId = entity.contextEdgeTypeId ?? fallbackTypeId;
```

**Rationale**: Backward compatibility. Data indexed before context support used BLOCKS relations for block discovery. The null context indicates relation-based discovery, which defaults to BLOCKS.

### Entity Deduplication

**Decision**: Context discovery wins over BLOCKS-relation fallback. When both discovery paths surface the same entity, the merged record inherits the non-null `contextEdgeTypeId` and the non-null `position`.

```typescript
deduped.set(entity.entityId, {
    entityId: entity.entityId,
    contextEdgeTypeId: existing.contextEdgeTypeId ?? entity.contextEdgeTypeId,
    position: existing.position ?? entity.position,
});
```

**Rationale**: The RFC specifies `edges[0].type_id` as the grouping key, so an entity tagged with context metadata must land under its dynamic key even if it's also reachable via a BLOCKS relation. Inheriting position from whichever entry has one preserves block ordering when context-discovery entries (which lack position) collide with relation-discovery entries (which carry it).

**Non-context behavior unchanged**: Diffs without any context metadata go through the pure BLOCKS-fallback path — all entries arrive with `contextEdgeTypeId: null`, are deduped by entityId, sorted by position, and land in `blocks`. This matches the pre-RFC behavior exactly.

### Position-Based Ordering

**Decision**: Entities are sorted by position; null positions go last.

```typescript
if (a.position === null && b.position === null) return 0;
if (a.position === null) return 1;
if (b.position === null) return -1;
return a.position.localeCompare(b.position);
```

**Rationale**: Blocks have a defined order via the `position` field (fractional indexing). Entities without position (e.g., discovered via context without position info) appear after positioned entities.

### Group Keys Sorting

**Decision**: `groupKeys` array is sorted alphabetically.

```typescript
const groupKeys = Array.from(dynamicGroups.keys()).sort();
```

**Rationale**: Provides deterministic ordering for API consumers iterating over dynamic groups.

### Block Type Detection Fallback

**Decision**: Use "to" block's type, fall back to "from" block's type.

```typescript
const blockType = getBlockType(toBlock) ?? getBlockType(fromBlock ?? toBlock);
```

**Rationale**: Similar to name resolution - ensures we can determine block type even when the block was deleted in the newer version.

### groupKeys Only Includes Changed Groups

**Decision**: In diff responses, `groupKeys` only lists groups that have actual changes.

```typescript
if (groupDiff.length > 0) {
    groups[key] = groupDiff;
}
const groupKeys = Object.keys(groups).sort();
```

**Rationale**: Reduces noise in diff responses. Consumers only iterate over groups with meaningful changes.

## Examples from Test Data

The following examples are derived from the actual test cases in `api/src/versioned/__tests__/diff.test.ts`.

### Value Diffing

#### Added Text Value

**Input:**
```typescript
from: []
to:   [{ propertyId: "prop-1", spaceId: "space-1", text: "new text" }]
```

**Output:**
```json
[{
    "propertyId": "prop-1",
    "spaceId": "space-1",
    "type": "TEXT",
    "before": null,
    "after": "new text",
    "diff": [{ "value": "new text", "added": true }]
}]
```

#### Changed Text Value (Word-Level Diff)

**Input:**
```typescript
from: [{ propertyId: "prop-1", spaceId: "space-1", text: "hello world" }]
to:   [{ propertyId: "prop-1", spaceId: "space-1", text: "hello universe" }]
```

**Output:**
```json
[{
    "propertyId": "prop-1",
    "spaceId": "space-1",
    "type": "TEXT",
    "before": "hello world",
    "after": "hello universe",
    "diff": [
        { "value": "hello " },
        { "value": "world", "removed": true },
        { "value": "universe", "added": true }
    ]
}]
```

#### Changed Integer Value

**Input:**
```typescript
from: [{ propertyId: "prop-1", spaceId: "space-1", integer: 10 }]
to:   [{ propertyId: "prop-1", spaceId: "space-1", integer: 20 }]
```

**Output:**
```json
[{
    "propertyId": "prop-1",
    "spaceId": "space-1",
    "type": "INT64",
    "before": "10",
    "after": "20"
}]
```

#### Changed Boolean Value

**Input:**
```typescript
from: [{ propertyId: "prop-1", spaceId: "space-1", boolean: false }]
to:   [{ propertyId: "prop-1", spaceId: "space-1", boolean: true }]
```

**Output:**
```json
[{
    "propertyId": "prop-1",
    "spaceId": "space-1",
    "type": "BOOL",
    "before": "false",
    "after": "true"
}]
```

### Relation Diffing

#### Added Relation

**Input:**
```typescript
from: []
to:   [{
    relationId: "rel-1",
    typeId: "type-1",
    fromEntityId: "from-entity",
    toEntityId: "to-1",
    spaceId: "space-1"
}]
```

**Output:**
```json
[{
    "relationId": "rel-1",
    "typeId": "type-1",
    "spaceId": "space-1",
    "changeType": "ADD",
    "before": null,
    "after": { "toEntityId": "to-1", "toSpaceId": null, "position": null }
}]
```

#### Updated Relation (Changed Target)

**Input:**
```typescript
from: [{ relationId: "rel-1", typeId: "type-1", toEntityId: "old-target", ... }]
to:   [{ relationId: "rel-1", typeId: "type-1", toEntityId: "new-target", ... }]
```

**Output:**
```json
[{
    "relationId": "rel-1",
    "typeId": "type-1",
    "spaceId": "space-1",
    "changeType": "UPDATE",
    "before": { "toEntityId": "old-target", "toSpaceId": null, "position": null },
    "after": { "toEntityId": "new-target", "toSpaceId": null, "position": null }
}]
```

#### Updated Relation (Changed Position)

**Input:**
```typescript
from: [{ relationId: "rel-1", toEntityId: "to-1", position: "a", ... }]
to:   [{ relationId: "rel-1", toEntityId: "to-1", position: "b", ... }]
```

**Output:**
```json
[{
    "relationId": "rel-1",
    "typeId": "type-1",
    "spaceId": "space-1",
    "changeType": "UPDATE",
    "before": { "toEntityId": "to-1", "toSpaceId": null, "position": "a" },
    "after": { "toEntityId": "to-1", "toSpaceId": null, "position": "b" }
}]
```

### Block Diffing

#### Added Text Block

**Input:**
```typescript
from: []
to:   [{
    id: "block-1",
    values: [{ propertyId: MARKDOWN_CONTENT, text: "new content" }],
    relations: [{ typeId: TYPES_PROPERTY, toEntityId: TEXT_BLOCK }]
}]
```

**Output:**
```json
[{
    "id": "block-1",
    "type": "textBlock",
    "before": null,
    "after": "new content",
    "diff": [{ "value": "new content", "added": true }]
}]
```

#### Changed Text Block

**Input:**
```typescript
from: [{ id: "block-1", values: [{ text: "old content" }], ... }]
to:   [{ id: "block-1", values: [{ text: "new content" }], ... }]
```

**Output:**
```json
[{
    "id": "block-1",
    "type": "textBlock",
    "before": "old content",
    "after": "new content",
    "diff": [
        { "value": "old", "removed": true },
        { "value": "new", "added": true },
        { "value": " content" }
    ]
}]
```

#### Added Image Block

**Input:**
```typescript
from: []
to:   [{
    id: "block-1",
    values: [{ propertyId: IMAGE_URL_PROPERTY, text: "https://example.com/image.png" }],
    relations: [{ typeId: TYPES_PROPERTY, toEntityId: IMAGE_BLOCK }]
}]
```

**Output:**
```json
[{
    "id": "block-1",
    "type": "imageBlock",
    "before": null,
    "after": "https://example.com/image.png"
}]
```

#### Changed Image Block

**Input:**
```typescript
from: [{ id: "block-1", values: [{ text: "https://old.com/image.png" }], ... }]
to:   [{ id: "block-1", values: [{ text: "https://new.com/image.png" }], ... }]
```

**Output:**
```json
[{
    "id": "block-1",
    "type": "imageBlock",
    "before": "https://old.com/image.png",
    "after": "https://new.com/image.png"
}]
```

#### Added Data Block

**Input:**
```typescript
from: []
to:   [{
    id: "block-1",
    values: [{ propertyId: NAME_PROPERTY, text: "My Data Block" }],
    relations: [{ typeId: TYPES_PROPERTY, toEntityId: DATA_BLOCK }]
}]
```

**Output:**
```json
[{
    "id": "block-1",
    "type": "dataBlock",
    "before": null,
    "after": "My Data Block"
}]
```

### Entity Snapshot Diffing

#### Combined Value, Relation, and Block Changes

**Input:**
```typescript
from: {
    id: "entity-1",
    values: [{ propertyId: "prop-1", text: "old value" }],
    relations: [{ relationId: "rel-1", toEntityId: "target-1" }],
    blocks: [{ id: "block-1", values: [{ text: "old block" }], ... }]
}
to: {
    id: "entity-1",
    values: [{ propertyId: "prop-1", text: "new value" }],
    relations: [{ relationId: "rel-1", toEntityId: "target-2" }],
    blocks: [{ id: "block-1", values: [{ text: "new block" }], ... }]
}
```

**Output:**
```json
{
    "entityId": "entity-1",
    "name": null,
    "values": [{
        "propertyId": "prop-1",
        "spaceId": "space-1",
        "type": "TEXT",
        "before": "old value",
        "after": "new value",
        "diff": [
            { "value": "old", "removed": true },
            { "value": "new", "added": true },
            { "value": " value" }
        ]
    }],
    "relations": [{
        "relationId": "rel-1",
        "typeId": "type-1",
        "spaceId": "space-1",
        "changeType": "UPDATE",
        "before": { "toEntityId": "target-1", "toSpaceId": null, "position": null },
        "after": { "toEntityId": "target-2", "toSpaceId": null, "position": null }
    }],
    "blocks": [{
        "id": "block-1",
        "type": "textBlock",
        "before": "old block",
        "after": "new block",
        "diff": [
            { "value": "old", "removed": true },
            { "value": "new", "added": true },
            { "value": " block" }
        ]
    }]
}
```

### Grouped Entity Diffing (Hybrid Mode)

#### Hybrid Mode with Blocks and Dynamic Groups

**Input:**
```typescript
from: {
    id: "entity-1",
    values: [],
    relations: [],
    blocks: [{ id: "block-1", values: [{ text: "old block" }], ... }],
    groupKeys: ["custom-type"],
    groups: {
        "custom-type": [{ id: "child-1", values: [{ text: "old child" }], ... }]
    }
}
to: {
    id: "entity-1",
    values: [],
    relations: [],
    blocks: [{ id: "block-1", values: [{ text: "new block" }], ... }],
    groupKeys: ["custom-type"],
    groups: {
        "custom-type": [{ id: "child-1", values: [{ text: "new child" }], ... }]
    }
}
```

**Output:**
```json
{
    "entityId": "entity-1",
    "name": null,
    "values": [],
    "relations": [],
    "blocks": [{
        "id": "block-1",
        "type": "textBlock",
        "before": "old block",
        "after": "new block",
        "diff": [
            { "value": "old", "removed": true },
            { "value": "new", "added": true },
            { "value": " block" }
        ]
    }],
    "groupKeys": ["custom-type"],
    "groups": {
        "custom-type": [{
            "id": "child-1",
            "type": "textBlock",
            "before": "old child",
            "after": "new child",
            "diff": [
                { "value": "old", "removed": true },
                { "value": "new", "added": true },
                { "value": " child" }
            ]
        }]
    }
}
```

#### Only Changed Groups Appear in groupKeys

**Input:**
```typescript
from: {
    groupKeys: ["type-a", "type-b"],
    groups: {
        "type-a": [{ id: "a-1", values: [{ text: "unchanged" }], ... }],
        "type-b": [{ id: "b-1", values: [{ text: "old" }], ... }]
    },
    ...
}
to: {
    groupKeys: ["type-a", "type-b"],
    groups: {
        "type-a": [{ id: "a-1", values: [{ text: "unchanged" }], ... }],
        "type-b": [{ id: "b-1", values: [{ text: "new" }], ... }]
    },
    ...
}
```

**Output:**
```json
{
    "entityId": "entity-1",
    "name": null,
    "values": [],
    "relations": [],
    "blocks": [],
    "groupKeys": ["type-b"],
    "groups": {
        "type-b": [{
            "id": "b-1",
            "type": "textBlock",
            "before": "old",
            "after": "new",
            "diff": [
                { "value": "old", "removed": true },
                { "value": "new", "added": true }
            ]
        }]
    }
}
```

Note: `type-a` is not in `groupKeys` because its content is unchanged.

## Test Coverage

### Grouping Tests (21 tests)

- Empty input handling
- BLOCKS context grouping to static `blocks` array
- Null context (relation fallback) grouping to `blocks`
- Custom fallback type support
- Dynamic grouping for non-BLOCKS types
- Sorted `groupKeys` for discoverability
- Hybrid mode (blocks + dynamic groups together)
- Context + fallback mixing
- Deduplication (first occurrence wins)
- Position-based ordering
- Null positions sorted last
- Ordering within dynamic groups

### Diff Tests (39 tests)

**Value diffing (10 tests)**:
- Empty/identical inputs
- Added/removed/changed text values with before/after strings
- Word-level diff chunks
- Added/changed integer and boolean values with before/after
- Multiple changes in single diff
- Space-based value distinction

**Relation diffing (6 tests)**:
- Empty/identical inputs
- Added/removed relations
- Changed relation target/position
- Multiple relation changes

**Block diffing (8 tests)**:
- Empty/identical inputs
- Added/removed/changed text blocks with before/after strings
- Image block URL changes with before/after
- Data block name changes with before/after
- Mixed block types

**Entity snapshot diffing (3 tests)**:
- Entity ID and name in response
- Combined value/relation/block diffs
- Name fallback from "to" to "from"

**Grouped entity diffing (10 tests)**:
- Response shape verification
- Empty arrays when identical
- Block diffs in static blocks array
- Dynamic group diff computation
- groupKeys only includes changed groups
- Added/removed dynamic group handling
- Hybrid mode (blocks + dynamic)
- groupKeys alphabetical sorting
- Name fallback behavior

## Known Gaps and Deferred Work

### Delete-with-Context Attribution

The `Grc20Op::DeleteRelation` indexer path does not write a tombstone version
row carrying the edit's context. The live `relations` row is removed before
the version table is touched, so by the time `insert_relation_versions` runs
the pre-delete state is unrecoverable in the same transaction. As a result,
relations deleted under a contextual edit appear in diffs only via the
closure of `valid_to_key` on the existing version row — `queryContextEntities`
does not surface them under their context group.

Fixing this requires either fetching the pre-delete state up the call chain
or moving the version-write to run before the live-table delete. Tracked as
follow-up work; not blocking for the create- and update-relation paths which
this RFC fully covers.

### Response-Size Caps and Truncation

The current implementation has **no caps** on the number of entities discovered, the number of dynamic groups, text diff length, or final response size. This is adequate for typical edits but creates a DoS surface for pathological inputs:

- An adversarial edit authoring 10k+ distinct `context` entries could inflate a single diff to arbitrary size.
- A single text block with a very large markdown payload forces `diffWords` into quadratic work.
- A root entity with a very large `context_root_id` fan-out returns an unbounded row set from the database.

Future hardening — to be scoped and implemented separately — should introduce layered caps with a visible `truncated` flag rather than silent cutoff:

| Layer | Proposed cap | Signal |
|---|---|---|
| DB row fetch (`queryContextEntities`) | `LIMIT 10_001` with overflow detection | `contextRowLimit` |
| DB row fetch (`queryBlocksRelationEntities`) | `LIMIT 10_001` with overflow detection | `relationRowLimit` |
| Per-group children in grouping | 1000 | `childrenPerGroupLimit` |
| Distinct dynamic group keys | 100 | `groupKeyLimit` |
| `computeTextDiff` inputs | 400k combined chars; fall back to `{removed, added}` chunk pair | `textDiffCharLimit` |
| Final serialized JSON size | 4 MB; return HTTP 413 | 413 response |

Response-shape additions would be:

```ts
interface GroupedEntityDiff {
    // ...existing fields
    truncated: boolean;           // true if any cap was hit
    truncationReasons: string[];  // sorted list of reason codes
}
```

Overflow semantics should be "drop silently within a group, surface a reason code at the top level" — consumers render partial results with a banner rather than seeing an error. The response-size 4 MB ceiling is the only hard failure (413), reserved for pathological cases where even truncated output wouldn't fit.

A prototype implementation was drafted and reverted in favor of shipping the context-aware grouping without behavior changes for non-context diffs; see git history for reference.

### Pagination

The RFC does not call for cursor pagination, and the diff endpoint is framed as "render the full diff inline on the entity page". If caps prove insufficient and we need to page through context-aware groups:

- Introduce `?cursor=` query parameter and `nextCursor?: string` response field.
- Cursor ordering would need a stable secondary sort (e.g., `(groupKey, entityId)`) because `position` can be null.
- This is deferred until observed demand — caps + truncation are strictly simpler for the renderer-inline UX.

### Cross-Space Context Edges

Context queries are currently scoped to the request's `spaceId`, matching the existing versioned-diff scoping. A context in space A will not surface children that live in space B. The RFC does not cover cross-space diffs; if we later need them, the query would drop the `space_id` filter and the response would need to disambiguate children by `spaceId`.
