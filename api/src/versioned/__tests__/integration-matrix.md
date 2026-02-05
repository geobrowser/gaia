# Versioned API Integration Test Matrix

This document defines the comprehensive test matrix for versioned entity and proposal diff APIs.

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /versioned/entities/:id` | Entity snapshot at version |
| `GET /versioned/entities/:id/versions` | List versions affecting entity |
| `GET /versioned/entities/:id/diff` | Diff between two versions |
| `GET /versioned/proposals/:id/diff` | Proposal diff (proposed vs base state) |

---

## 1. Entity Snapshot (`GET /versioned/entities/:id`)

### Value Types (13 types)

| Type | Test Case | Expected |
|------|-----------|----------|
| TEXT | Entity has text value | `text: "value"` |
| BOOL | Entity has boolean value | `boolean: true/false` |
| INT64 | Entity has integer value | `integer: "42"` (string) |
| FLOAT64 | Entity has float value | `float: 3.14` |
| DECIMAL | Entity has decimal value | `decimal: "123.456"` |
| BYTES | Entity has bytes value | `bytes: "base64..."` |
| DATE | Entity has date value | `date: "2024-01-15"` |
| TIME | Entity has time value | `time: "14:30:00"` |
| DATETIME | Entity has datetime value | `datetime: "2024-01-15T14:30:00Z"` |
| SCHEDULE | Entity has schedule value | `schedule: {...}` |
| POINT | Entity has point value | `point: "lat,lng"` |
| RECT | Entity has rect value | `rect: "..."` |
| EMBEDDING | Entity has embedding value | `embedding: [...]` |

### Relations

| Scenario | Test Case | Expected |
|----------|-----------|----------|
| No relations | Entity has no relations | `relations: []` |
| Single relation | Entity has one relation | `relations: [{...}]` |
| Multiple relations | Entity has many relations | `relations: [{...}, {...}]` |
| Cross-space relation | Relation points to other space | `toSpaceId: "..."` |
| Positioned relation | Relation has position | `position: "a0"` |
| Verified relation | Relation is verified | `verified: true` |

### Blocks

| Scenario | Test Case | Expected |
|----------|-----------|----------|
| No blocks | Entity has no BLOCKS relations | `blocks: []` |
| Text block | Entity has text block child | Block with `text` value |
| Image block | Entity has image block child | Block with image URL |
| Data block | Entity has data block child | Block with name |
| Multiple blocks | Entity has multiple blocks | `blocks: [{...}, {...}]` |
| Nested blocks | Block has its own blocks | Recursive structure |

### Edge Cases

| Scenario | Test Case | Expected |
|----------|-----------|----------|
| Entity doesn't exist | Query non-existent entity | Empty snapshot |
| Entity deleted at version | Entity was deleted before version | Empty snapshot |
| Entity created after version | Entity doesn't exist at requested version | Empty snapshot |
| Multiple values same property | Different spaces have same property | Multiple values |

---

## 2. Entity Versions (`GET /versioned/entities/:id/versions`)

| Scenario | Test Case | Expected |
|----------|-----------|----------|
| No versions | Entity never modified | `versions: []` |
| Single version | Entity created once | 1 version entry |
| Multiple versions | Entity modified multiple times | Multiple entries |
| Pagination | Many versions with limit | Respects limit |
| Space filter | Versions filtered by space | Only matching space |

---

## 3. Entity Diff (`GET /versioned/entities/:id/diff`)

### Value Changes

| Change Type | Scenario | Expected |
|-------------|----------|----------|
| ADD | Value didn't exist, now exists | `before: null, after: "value"` |
| REMOVE | Value existed, now doesn't | `before: "value", after: null` |
| UPDATE | Value changed | `before: "old", after: "new"` |
| NO CHANGE | Value same in both | Not in diff |

### Text Diff Chunks

| Scenario | Expected Diff |
|----------|---------------|
| Word added | `[{value: "hello "}, {value: "world", added: true}]` |
| Word removed | `[{value: "hello", removed: true}, {value: " world"}]` |
| Word changed | `[{value: "old", removed: true}, {value: "new", added: true}]` |
| Multiple changes | Complex diff array |

### Relation Changes

| Change Type | Scenario | Expected |
|-------------|----------|----------|
| ADD | Relation created | `changeType: "ADD", before: null, after: {...}` |
| REMOVE | Relation deleted | `changeType: "REMOVE", before: {...}, after: null` |
| UPDATE | Relation target changed | `changeType: "UPDATE", before: {...}, after: {...}` |
| Position change | Only position changed | `changeType: "UPDATE"` with position diff |

### Block Changes

| Block Type | Change | Expected |
|------------|--------|----------|
| textBlock | Text changed | `type: "textBlock", diff: [...]` |
| textBlock | Added | `before: null, after: "text"` |
| textBlock | Removed | `before: "text", after: null` |
| imageBlock | URL changed | `type: "imageBlock", before: "url1", after: "url2"` |
| dataBlock | Name changed | `type: "dataBlock", before: "name1", after: "name2"` |

### Edge Cases

| Scenario | Expected |
|----------|----------|
| Same version (v1 → v1) | Empty diff |
| Reverse direction (v2 → v1) | Inverted diff |
| Entity created between versions | All values as ADDs |
| Entity deleted between versions | All values as REMOVEs |

---

## 4. Proposal Diff (`GET /versioned/proposals/:id/diff`)

### Proposal Status

| Status | Condition | Base State |
|--------|-----------|------------|
| active | `now < end_time` | Current live state |
| closed | `now >= end_time && !executed_at` | Versioned state at end_time |
| executed | `executed_at != null` | Versioned state at end_time |

### Edit Blob Scenarios

| Scenario | Expected |
|----------|----------|
| No publish action | Empty entities array |
| Empty edit (no ops) | Empty entities array |
| Single entity create | 1 entity diff with all ADDs |
| Single entity update | 1 entity diff with changes |
| Multiple entities | Multiple entity diffs |
| Entity delete | Entity with all REMOVEs |

### Op Types to Test

| Op Type | Test Case |
|---------|-----------|
| createEntity | New entity with values |
| updateEntity | Set/unset values |
| deleteEntity | Entity removed |
| restoreEntity | Entity restored |
| createRelation | New relation |
| updateRelation | Relation modified |
| deleteRelation | Relation removed |
| restoreRelation | Relation restored |
| createValueRef | Value reference |

### Pagination

| Scenario | Expected |
|----------|----------|
| Fewer entities than limit | `hasMore: false, cursor: null` |
| More entities than limit | `hasMore: true, cursor: "..."` |
| Second page | Continues from cursor |
| Invalid cursor | 400 error |

### Error Cases

| Scenario | Expected |
|----------|----------|
| Proposal not found | 404 |
| Space mismatch | 400 |
| Edit blob not cached | 404 |
| Edit decode error | 500 |

---

## Test Data Requirements

### Entities
- Entity with all 13 value types
- Entity with relations (various configurations)
- Entity with blocks (text, image, data)
- Entity that changes between versions
- Entity that gets deleted
- Entity that gets created between versions

### Edits
- At least 3 edit versions for testing ranges
- Edits that create, update, delete entities

### Proposals
- Active proposal (end_time in future)
- Closed proposal (end_time in past)
- Executed proposal (executed_at set)
- Proposal with no publish action
- Proposal with publish action and cached edit blob

### Relations
- Same-space relations
- Cross-space relations
- Positioned relations
- BLOCKS relations (for block content)
