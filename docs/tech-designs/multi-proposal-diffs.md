# Multi-Proposal Diffs — Tech Design

**Date:** 2026-04-14
**Status:** WIP
**RFC:** [0004-multi-proposal-diff-groups](../rfcs/0004-multi-proposal-diff-groups.md)
**PR:** [geobrowser/gaia#585](https://github.com/geobrowser/gaia/pull/585)

---

# Background

## Current System

The Gaia API provides a proposal diff endpoint that computes on-demand diffs for a single governance proposal:

```
GET /versioned/proposals/:id/diff?spaceId=<uuid>
```

The pipeline works as follows:

1. Load the proposal and its `Publish` action from the `proposals` / `proposal_actions` tables
2. Fetch the encoded edit blob from the `ipfs_cache` table using the proposal's `content_uri`
3. Decode the blob into GRC-20 ops using `decodeEditAuto` from `@geoprotocol/grc-20`
4. Extract all affected entity IDs from the ops
5. Determine proposal status (`active` / `closed` / `executed`) and select the appropriate base state:
   - Active proposals diff against the current live KG state
   - Closed/executed proposals diff against versioned state at the relevant timestamp
6. Fetch base state snapshots (values, relations, blocks) for each affected entity
7. Apply ops to each entity's base snapshot to produce the proposed state
8. Compute a structured diff between base and proposed states
9. Return a paginated `EntityDiff[]` response

The response includes per-entity value changes, relation changes, and block changes with text-level word diffs for TEXT properties.

**Key source files:**
- `api/src/versioned/proposal-diff.ts` — core pipeline
- `api/src/versioned/diff.ts` — snapshot diffing engine
- `api/src/versioned/router.ts` — HTTP route handlers
- `api/src/versioned/queries.ts` — database queries

## Frontend Post-Processing Problem

The backend returns diffs as a flat list of ID-heavy entity changes. The frontend ([`postProcessDiffs` in geogenesis](https://github.com/geobrowser/geogenesis/blob/master/apps/web/core/utils/diff/diff.ts#L203-L680)) performs ~480 lines of post-processing to transform these into display-ready diffs:

1. **Name resolution** — batch-fetches entity names for property IDs, relation type IDs, and relation target IDs via additional GraphQL queries
2. **Block folding** — re-nests block entity diffs (text blocks, image blocks, data blocks) under their parent entity's `blocks[]` array
3. **Media-property filtering** — identifies IMAGE/VIDEO-type entities that are property values (not page blocks), filters them out, and injects their URLs into the parent entity's relation
4. **Orphan block resolution** — blocks that appear in the diff without a BLOCKS relation are resolved via backlink queries to find their parent
5. **Synthetic block diffs** — blocks referenced by BLOCKS relations but missing from the diff are synthesized from committed state
6. **Data block enrichment** — block configuration relations are merged into the data block entity

This post-processing adds latency (multiple round-trips to the GraphQL API from the browser), duplicates logic that should be authoritative on the backend, and creates a maintenance burden.

---

# General

This project extends the proposal diff system with two capabilities:

1. **Multi-proposal grouped diffs** — a new API endpoint that diffs 2-20 proposals as a single ordered changeset, returning one combined entity-centered diff
2. **Display-ready enrichment** — backend-side name resolution that eliminates the most impactful piece of frontend post-processing

Both capabilities are additive. The existing single-proposal endpoint is preserved unchanged.

## Architecture Overview

```
                                   ┌─────────────────────────┐
                                   │   Frontend (geogenesis)  │
                                   └────────┬────────────────┘
                                            │
                        ┌───────────────────┼───────────────────┐
                        │                   │                   │
               Single proposal        Multi-proposal       (Future)
                        │                   │              Block folding
                        ▼                   ▼                   │
              GET /versioned/      GET /versioned/              ▼
              proposals/:id/diff   proposal-groups/diff    Phase 2b
                        │                   │
                        ▼                   ▼
                 ┌──────────────────────────────────┐
                 │   computeProposalDiff()           │
                 │   computeGroupedProposalDiff()    │
                 │   (shared pipeline tail)          │
                 └──────────────┬───────────────────┘
                                │
                                ▼
                 ┌──────────────────────────────────┐
                 │   enrichEntityDiffs()             │
                 │   (name resolution)               │
                 └──────────────┬───────────────────┘
                                │
                                ▼
                        JSON Response
```

## Requirements

1. **Multi-proposal endpoint** — diff 2-20 proposals as one ordered changeset in a single request
2. **Existing endpoint unchanged** — `GET /versioned/proposals/:id/diff` behavior is identical
3. **Edit-timestamp ordering** — grouped edits are applied in chronological order (`created_at ASC`, `proposal_id ASC` tiebreaker)
4. **Homogeneous mode** — all proposals must be in the same mode (all active OR all historical); mixed groups are rejected
5. **Single-space scope** — all proposals must belong to the same space
6. **Name enrichment** — all diff responses include resolved human-readable names for property IDs, relation type IDs, and entity IDs
7. **Backward-compatible** — new response fields are additive (optional). Existing consumers are unaffected
8. **Paginated** — same cursor-based pagination model as the single-proposal endpoint

---

# In-Depth

## API Contract

### New Endpoint: Grouped Proposal Diff

```
GET /versioned/proposal-groups/diff
```

**Query parameters:**

| Parameter | Type | Required | Description |
|---|---|---|---|
| `spaceId` | UUID | Yes | Space that all proposals belong to |
| `proposalIds` | comma-separated UUIDs | Yes | 2-20 proposal IDs |
| `cursor` | string | No | Base64-encoded pagination cursor |
| `limit` | integer (1-100) | No | Max entities per page (default: 50) |

**Validation rules:**

| Rule | HTTP Status | Error |
|---|---|---|
| `spaceId` missing or invalid UUID | 400 | `Invalid parameter` |
| `proposalIds` missing | 400 | `Invalid parameter` |
| Fewer than 2 proposal IDs | 400 | `Invalid parameter` |
| Invalid UUID in `proposalIds` | 400 | `Invalid parameter` |
| More than 20 proposal IDs | 400 | `Group size {N} exceeds maximum of 20` |
| Duplicate proposal IDs | 400 | `Duplicate proposal IDs are not allowed` |
| Proposal not found | 404 | `One or more proposals not found` |
| Proposal in wrong space | 400 | `One or more proposals do not belong to the specified space` |
| Missing Publish action | 422 | `All proposals in a group must have a Publish action` |
| Mixed active + historical | 400 | `Cannot mix active ({N}) and historical ({N}) proposals in a group` |
| Edit blob not cached | 404 | `Edit blob not cached for one or more proposals` |
| Invalid cursor | 400 | `Invalid pagination cursor` |

**Response shape:**

```json
{
  "proposalIds": [
    "aabbccdd00001111222233334444aaaa",
    "aabbccdd00001111222233334444bbbb"
  ],
  "spaceId": "aabbccdd00001111222233335555cccc",
  "mode": "active",
  "entities": [
    {
      "entityId": "aabbccdd00001111222233336666dddd",
      "name": "My Entity",
      "values": [
        {
          "propertyId": "a126ca530c8e48d5b88882c734c38935",
          "propertyName": "Name",
          "spaceId": "aabbccdd00001111222233335555cccc",
          "type": "TEXT",
          "before": "Old name",
          "after": "New name",
          "diff": [
            { "value": "Old", "removed": true },
            { "value": "New", "added": true },
            { "value": " name" }
          ]
        }
      ],
      "relations": [
        {
          "relationId": "...",
          "typeId": "...",
          "typeName": "Member Of",
          "spaceId": "...",
          "changeType": "ADD",
          "before": null,
          "after": {
            "toEntityId": "...",
            "toEntityName": "Geo DAO",
            "toSpaceId": null,
            "position": "a0"
          }
        }
      ],
      "blocks": [
        {
          "id": "...",
          "type": "textBlock",
          "before": null,
          "after": "Hello world",
          "diff": [
            { "value": "Hello world", "added": true }
          ]
        }
      ]
    }
  ],
  "pagination": {
    "cursor": "eyJlbnRpdHlJbmRleCI6NTAsInRvdGFsRW50aXRpZXMiOjEyM30=",
    "hasMore": true,
    "totalEntities": 123
  }
}
```

### Enriched Fields on Existing Endpoint

The existing `GET /versioned/proposals/:id/diff` response is enriched with the same name fields. These are additive — no fields are removed or renamed:

| Field | Location | Type | Description |
|---|---|---|---|
| `propertyName` | `ValueChange` | `string \| null` | Human-readable name for `propertyId` |
| `typeName` | `RelationChange` | `string \| null` | Human-readable name for `typeId` |
| `toEntityName` | `RelationChange.before` | `string \| null` | Human-readable name for `before.toEntityId` |
| `toEntityName` | `RelationChange.after` | `string \| null` | Human-readable name for `after.toEntityId` |

These fields are resolved from the live `values` table using `NAME_PROPERTY` (`a126ca530c8e48d5b88882c734c38935`). If the entity has no name, the field is `null`.

### `mode` Field

The `mode` field in the grouped response communicates which base-state strategy was used:

| Mode | When | Base State |
|---|---|---|
| `"active"` | All proposals are active (end_time in future) | Current live KG state |
| `"historical"` | All proposals are closed or executed | Versioned KG state just before the earliest edit timestamp |

Mixed groups (some active, some historical) are rejected with a 400 error rather than producing ambiguous diffs.

### Ordering Semantics

Grouped proposals are applied in ascending order by the **edit timestamp** recorded in the decoded GRC-20 edit's `createdAt` field (microseconds), with `proposalId` as a deterministic tiebreaker:

```
ORDER BY edit.createdAt ASC, proposalId ASC
```

This reflects the real chronology of changes rather than proposal creation or voting timestamps.

### Pagination

Both endpoints use the same cursor-based pagination. The cursor encodes the current position in the sorted entity list:

```json
{
  "entityIndex": 50,
  "totalEntities": 123
}
```

Base64-encoded in the `cursor` query parameter. The `totalEntities` field is a consistency check — if the entity count changes between pages (rare, possible if a proposal is updated mid-pagination), the API logs a warning but continues.

## Frontend Integration Guide

### Consuming the Grouped Diff Endpoint

**Request:**
```typescript
const response = await fetch(
  `${API_URL}/versioned/proposal-groups/diff?` +
  `spaceId=${spaceId}&` +
  `proposalIds=${proposalIds.join(",")}&` +
  `limit=50`
);
const data = await response.json();
// data.mode: "active" | "historical"
// data.entities: EntityDiff[]
// data.pagination: { cursor, hasMore, totalEntities }
```

**Pagination loop:**
```typescript
let allEntities: EntityDiff[] = [];
let cursor: string | null = null;

do {
  const url = new URL(`${API_URL}/versioned/proposal-groups/diff`);
  url.searchParams.set("spaceId", spaceId);
  url.searchParams.set("proposalIds", proposalIds.join(","));
  if (cursor) url.searchParams.set("cursor", cursor);

  const res = await fetch(url);
  const data = await res.json();
  allEntities.push(...data.entities);
  cursor = data.pagination.cursor;
} while (cursor);
```

### Using Enriched Name Fields

With name enrichment, the frontend no longer needs to call `getBatchEntities` to resolve names for diff display. The names are pre-populated:

```typescript
// Before (frontend post-processing required):
const propertyLabel = nameMap.get(valueChange.propertyId) ?? valueChange.propertyId;

// After (names come from API):
const propertyLabel = valueChange.propertyName ?? valueChange.propertyId;
```

**Important:** `propertyName`, `typeName`, and `toEntityName` may be `null` if the entity has no name in the knowledge graph. Always fall back to the ID.

### What Frontend Can Remove After Each Phase

| Phase | Frontend code to remove | What replaces it |
|---|---|---|
| Phase 1 (grouped endpoint) | Client-side multi-proposal fetching + merging | Single API call |
| Phase 2a (name enrichment) | `getBatchEntities` call in `postProcessDiffs` step 4, name mapping in step 9 | `propertyName`, `typeName`, `toEntityName` from API |
| Phase 2b (block folding, future) | Steps 1-3 and 5-8 of `postProcessDiffs`, `groupBlocksUnderParents`, `entityDiffToBlockChange` | Server-side block folding + media URL injection |

After Phase 2b, `postProcessDiffs` can be deleted entirely (~480 lines).

### Error Handling

The grouped endpoint returns structured JSON errors. Frontend should handle:

```typescript
switch (response.status) {
  case 200: // Success
    break;
  case 400: // Validation error (bad input)
    // data.error: "Invalid parameter"
    // data.message: human-readable description
    break;
  case 404: // Proposal or edit blob not found
    break;
  case 422: // Proposal has no Publish action, or edit blob failed validation
    break;
  case 500: // Server error
    break;
}
```

## Off-Chain

### Implementation Details

The grouped diff is implemented as request-time squashing — no new database tables, indexes, or background jobs.

**Core function:** `computeGroupedProposalDiff()` in `api/src/versioned/proposal-diff.ts`

**Pipeline:**

```
1. Validate inputs ──────────────── (no DB calls)
      │
2. Batch-load proposals ─────────── SELECT ... FROM proposals p
      │                              LEFT JOIN proposal_actions pa
      │                              WHERE p.id = ANY($1::uuid[])
      │
3. Validate proposals ───────────── (in-memory: space match, mode, content_uri)
      │
4. Fetch edit blobs ─────────────── SELECT data, is_errored FROM ipfs_cache
      │                              WHERE uri = $1  (×N, in parallel)
      │
5. Decode + sort + concat ops ───── decodeEditAuto() × N, sort by (createdAt, proposalId)
      │
6. Extract affected entities ────── (from ops, with relation lookups for update/delete)
      │
7. Paginate entity IDs ─────────── slice(startIndex, startIndex + limit)
      │
8. Resolve base version key ─────── (active: skip; historical: query edit_versions)
      │
9. Fetch base state ─────────────── 3-6 SQL queries (values, relations, block relations,
      │                              block snapshots)
      │
10. Apply ops per entity ──────────  (in-memory, reuses applyOpsToSnapshot)
      │
11. Diff per entity ───────────────  (in-memory, reuses diffEntitySnapshots)
      │
12. Enrich names ──────────────────  1 SQL query (batch name lookup)
      │
13. Return paginated response ─────  JSON
```

**Total DB queries per request (typical, N=2 proposals, 1 page):**
- 1 batch proposal load
- 2 IPFS cache lookups (parallel)
- 1 version key resolution (historical only)
- 2 base state queries (values + relations)
- 1 block relations query
- 0-2 block snapshot queries (if blocks exist)
- 1 name enrichment query
- **Total: 7-9 queries**

**Name enrichment query:**

```sql
SELECT entity_id, text
FROM "values"
WHERE entity_id = ANY($1::uuid[])
  AND property_id = 'a126ca530c8e48d5b88882c734c38935'::uuid
  AND text IS NOT NULL
```

Single query regardless of how many IDs need resolution. Entity IDs are deduplicated before the query.

### Configuration

| Setting | Value | Description |
|---|---|---|
| Max group size | 20 | Maximum proposals per grouped diff request |
| Default page size | 50 | Entities per page |
| Max page size | 100 | Upper bound for `limit` parameter |

These are compile-time constants. If runtime configurability is needed, they can be moved to environment variables.

### Known Limitations

These limitations are inherited from the single-proposal endpoint and documented in the `KNOWN LIMITATIONS` header comment of `api/src/versioned/proposal-diff.ts`. The OpenAPI `describeRoute` for `GET /versioned/proposal-groups/diff` links back to this section so callers see them in the generated spec.

1. **`restoreEntity` ops** — the op contains only the entity ID, not the values/relations to restore. We'd need to fetch historical state to show what's being restored. Currently the diff cannot display the restored contents.
2. **`restoreRelation` ops** — the op contains only the relation ID. Since the relation doesn't exist in the live table (it was deleted), we can't look up which entity is affected. **These ops are silently skipped** — they produce no diff entry.
3. **`deleteEntity` ops** — shows removal of all current values/relations, but doesn't account for the entity potentially being in a deleted state already. Treating a double-delete as a regular delete is generally harmless but may produce a misleading "removed" diff.
4. **Cross-space groups** — not supported in v1. All proposals must belong to one space.
5. **Name resolution uses live state** — names are fetched from the current live values table, not versioned state. For historical diffs, a recently renamed entity will show its current name, not the name at the time of the proposals.
6. **Group size** — capped at `MAX_GROUP_SIZE = 20` proposals per request. Exported from `proposal-diff.ts` and surfaced in the route description.

Properly supporting (1)-(3) requires restructuring the op-extraction code to (a) pass version context into entity extraction, (b) fetch historical state for restore ops, and (c) track entity/relation deletion state. Out of scope for v1.

## Invariants

1. The existing `GET /versioned/proposals/:id/diff` endpoint returns identical responses before and after this change (except for the new additive name fields)
2. Grouped diffs with a single proposal ID are rejected (minimum 2)
3. All proposals in a group must belong to the requested `spaceId`
4. All proposals in a group must be in the same mode (all active or all historical)
5. Ops from different proposals are applied in `(createdAt ASC, proposalId ASC)` order — the same ops always produce the same diff regardless of the order in which `proposalIds` are passed
6. Paginated responses are stable: the same cursor against the same proposals returns the same page (assuming no proposal updates between requests)
7. Name enrichment never fails the request — missing names degrade to `null`

---

# External Requirements

## Geogenesis Frontend

The frontend team should plan integration in phases:

1. **Phase 1 adoption** — replace any client-side multi-proposal diff merging with a single call to the grouped endpoint
2. **Phase 2a adoption** — remove `getBatchEntities` name-resolution from `postProcessDiffs` and use `propertyName`/`typeName`/`toEntityName` from the API response
3. **Phase 2b adoption** (future) — once block folding lands server-side, delete `postProcessDiffs` entirely (~480 lines in `apps/web/core/utils/diff/diff.ts`)

No frontend changes are required for the backend to deploy. The new fields are additive and the new endpoint is opt-in.

---

# Milestones and Estimates

| Phase | Scope | Status | Estimate |
|---|---|---|---|
| **Phase 1** | Multi-proposal grouped diff endpoint | Complete (PR #585) | 1 week |
| **Phase 2a** | Name resolution enrichment | Complete (PR #585, commit 2) | 0.5 weeks |
| **Phase 2b** | Block folding + media URLs | Not started | 1-2 weeks |
| **Frontend integration** | Adopt grouped endpoint + remove postProcessDiffs | Not started | 1 week |

Phase 1 and Phase 2a are complete and in PR review. Phase 2b is the most complex piece and will be a separate PR.

### Future Optimization

If grouped diff latency becomes a problem (many proposals, large edits), the next optimization is **indexed proposal ops** — storing decoded ops in Postgres at ingest time to eliminate repeated blob fetch + decode work. This is described in [RFC 0004, "Indexed Proposal Ops" section](../rfcs/0004-multi-proposal-diff-groups.md).

---

# Open Questions and Thoughts

1. **Per-proposal metadata in grouped response** — should the response include per-proposal status and edit timestamp alongside the merged diffs? This would help the frontend show which proposal contributed which change.
2. **Name resolution for historical diffs** — currently uses live names. Should we resolve names at the historical version for accuracy?
3. **Phase 2b complexity** — block folding involves orphan resolution (backlink queries), media-property filtering, and synthetic diff generation. Should this be an opt-in query parameter (`?enrich=blocks`) to avoid breaking existing consumers?
4. **Max group size** — 20 is a conservative default. Should it be configurable via environment variable?

---

# Signatures

- _____ at _____ 
- _____ at _____
