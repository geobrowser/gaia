# Versioned Entity API Plan (REST-first)

## Goals
- Provide versioned entity reads that match the existing `FullEntity` GraphQL shape.
- Use a stable, chain-derived ordering key (not Kafka stream position).
- Avoid N+1 by using a small, fixed number of SQL queries per request.
- Keep scope limited to entity-level endpoints (no full graph snapshot).

## Current Query Shape (Reference)
From `geogenesis/apps/web/core/io/v2/fragments.tsx`:

- `Entity` fields: `id`, `name`, `description`, `spaceIds`, `updatedAt`
- `types`: `{ id, name }`
- `valuesList`:
  - `spaceId`
  - `property`: `id`, `name`, `dataType`, `renderableType`, `format`, `unit`, `relationValueTypes { id, name }`
  - scalar fields: `string`, `number`, `point`, `boolean`, `time`, `language`, `unit`
- `relationsList`:
  - `id`, `spaceId`, `position`, `verified`, `entityId`, `toSpaceId`
  - `fromEntity { id, name }`
  - `toEntity { id, name, types { id, name }, valuesList { propertyId, string, number, point, boolean, time } }`
  - `type { id, name, renderableType }`

This plan aims to return the same shape from REST for `GET /entities/:id?editId=...`.

## Version Ordering & Stability
- Use chain-derived ordering from Hermes edit metadata:
  - `block_number`
  - `sequence` (block-local order; from `blockchain_metadata.sequence`)
- Expose `edit_id` to clients; internal queries resolve `edit_id -> (block_number, sequence)` and compute a packed `version_key`.
- Avoid Kafka/ingest stream position in API and query contracts.
Note: This plan assumes `(block_number, sequence)` is unique per edit, so no edit-id tie-breaker is required in range fields.
Validated: `sequence` reflects on-chain order for edits, so `(block_number, sequence)` is a stable, exact ordering key.

## Storage Model (Temporal Ranges)
Note: These are **new tables**, added alongside existing `values`/`relations` to avoid breaking current queries. The existing tables remain the latest-state snapshot used by current APIs. This is not the optimal long-term storage pattern; in the future we intend to rely only on temporal/versioned tables and derive “latest” from them.
Add versioned tables that allow querying active rows at an edit:

- `edit_versions`:
  - `edit_id` (uuid, PK)
  - `block_number` (bigint)
  - `sequence` (int)
  - `created_at` (timestamptz)
  - `version_key` (bigint, packed from block+sequence)
  - unique `(block_number, sequence)`

- `value_records` (immutable, content-addressed):
  - `value_record_id` (uuid, PK)
  - `entity_id`, `property_id`, `space_id`, `language`, `unit`
  - value fields: `string`, `number`, `boolean`, `time`, `point`

- `value_versions` (temporal range mapping):
  - `entity_id`, `property_id`, `space_id`, `value_record_id`
  - `valid_from_key` (bigint, packed block+sequence)
  - `valid_to_key` (bigint, nullable)

- `relation_records` (immutable, content-addressed)
  - `relation_record_id` (uuid, PK)
  - relation fields (matching existing `relations`)

- `relation_versions` (temporal range mapping)
  - `relation_id`, `entity_id`, `relation_record_id`
  - `valid_from_key`, `valid_to_key`

Notes:
- Content-addressed IDs avoid duplicate row copies for unchanged values/relations.
- Range updates use `UPDATE ... SET valid_to_key = new` + `INSERT new`.

## REST Endpoints (Planned)

### 1) `GET /entities/:id?editId=...`
Return `FullEntity` shape at a specific edit.

Query strategy:
- Resolve `editId` -> `version_key` using `edit_versions`.
- Accept optional `spaceId` and apply it to `valuesList` and `relationsList` (mirrors GraphQL filters).
- Fetch values + relations for the entity via temporal range filters.
- Batch-resolve property details and entity types.
- Include `toEntity` data needed for rendering (types + minimal `valuesList`, e.g. image fields) to avoid an extra lookup.

Target: 2–3 SQL queries per request:
1. Base entity + values + relations (with IDs for batch enrichment).
2. Properties + relation types + entity types (batched).
3. `toEntity` types + `toEntity.valuesList` (batched).

### 2) `GET /entities/:id/versions`
Return list of edits that affected the entity, ordered by `(block_number, sequence, edit_id)`.

### 3) `GET /entities/:id/diff?fromEdit=...&toEdit=...`
Compute diff between two snapshots:
- Fetch values/relations at both edits.
- Diff server-side (structured response).
- Require `spaceId` and return fully hydrated value/relation payloads.

Proposed diff response format (ordering not significant):
```

## Proposal Diffs (On-Demand)
- Do not store proposals as versions.
- Compute proposal diffs by applying proposed edit ops to the current live snapshot and diffing.

Flow:
1) Load proposal + actions and extract edit ops.
   - Proposed edit ops are stored in the IPFS cache (kg-indexer DB currently acts as the cache).
2) Identify affected entities/relations from ops.
3) Fetch current live state for those entities (space-filtered).
4) Apply ops in memory to build a proposed snapshot.
5) Diff live vs proposed and return the same diff shape as version diffs.
Notes:
- Assume one edit per proposal in v1.
- Diff response format matches version diffs (fully hydrated).

## Proposal Diffs (Precomputed Option)
Large edits can be too expensive to decode in the API. Alternative design:

### Storage
```
proposal_diffs (
  proposal_id uuid,
  space_id uuid,
  diff_jsonb jsonb,
  generated_at timestamptz
)
```

`diff_jsonb` stores the fully computed diff payload (same shape as the API response).

### Indexing options
1) **Separate proposal-diff indexer**
   - Consume proposal events or watch proposal table changes.
   - Fetch edit blob from IPFS cache.
   - Compute diff and write `proposal_diffs`.
   - Async/throttled; does not block main kg-indexer writes.

2) **Job queue + worker**
   - Enqueue diff job when a proposal is created/updated.
   - Worker fetches edit blob, computes diff, writes `proposal_diffs`.

### API behavior
- If `proposal_diffs` exists, return it directly.
- Otherwise fall back to on-demand diff (if edit size is small) or return a 202/placeholder indicating diff is being generated.
{
  "entityId": "...",
  "fromEditId": "...",
  "toEditId": "...",
  "spaceId": "...",
  "values": {
    "added": [FullValue],
    "removed": [FullValue],
    "changed": [
      { "propertyId": "...", "spaceId": "...", "before": FullValue, "after": FullValue }
    ]
  },
  "relations": {
    "added": [FullRelation],
    "removed": [FullRelation],
    "changed": [
      { "relationId": "...", "before": FullRelation, "after": FullRelation }
    ]
  }
}
```

### 4) `GET /entities?editId=...`
List entities at a specific edit with filtering:
- `spaceId`, `typeId`, pagination
- Use aggregated/filtered queries to avoid N+1.

## Indexing Plan (High-level)

Assumption: only entity-scoped version reads are supported (no direct versioned values/relations reads).

`edit_versions`:
- Unique index on `(block_number, sequence)` for lookups and ordering.

`value_versions`:
- `(entity_id, valid_from_key)`
- `(entity_id, valid_to_key)`
- Partial `(entity_id)` where `valid_to_key IS NULL` for fast range closes

`relation_versions`:
- `(entity_id, valid_from_key)`
- Partial `(relation_id)` where `valid_to_key IS NULL` for fast range closes

## E2E Audit Checklist (Before Implementation)
- Confirm Hermes edit metadata includes `block_number` and `sequence` on all edits.
- Validate whether any edits are missing `sequence` and how to handle defaults.
- Confirm property/type semantics match current derived fields (`name`, `description`, `types`).
- Verify existing SQL helper functions that compute name/description/types and decide whether to replicate logic in REST queries.
- Validate how "spaceIds" are computed (values-based or relation-based) in current schema.
- Check how `relationsList.toEntity.valuesList` is restricted (fields and property filters).

## Open Questions
- Are `name` and `description` derived from specific property IDs? If yes, list them for REST logic.
- Should `valuesList`/`relationsList` accept `spaceId` filter in REST as in GraphQL?
- What is the expected behavior for entities that exist but have zero values/relations at a given edit?

## Next Steps (after audit)
- Draft SQL for endpoint #1 with batched queries.
- Extend kg-indexer to populate `edit_versions` and temporal range tables.
- Add REST handlers and tests for snapshot consistency.

## Aggregation Mode (Spaces DAG)
- REST query params:
  - `spaceScope=local|transitive|canonical` (default `local`)
  - `conflictPolicy=nearest` (default when `spaceScope` is not `local`)
- `nearest` selects the value/relation from the closest descendant space in the transitive set.
- If only local data is desired, callers should use `spaceScope=local` with a `spaceId`.
Note: Voting/ranking signals exist per space; v1 will not use them for conflict resolution. Consider as a future enhancement.
