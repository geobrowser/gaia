---
title: feat: Migrate API to Base58-encoded IDs
type: feat
date: 2026-02-09
---

# feat: Migrate API to Base58-Encoded IDs

## Overview

Change the API layer to accept and return Base58-encoded IDs instead of hex UUIDs. GRC-20 protocol IDs are 16 bytes (128-bit), same as UUIDs — Base58 is just a shorter string encoding (~22 chars vs 32 hex chars). The database stays `uuid`, the indexer pipeline stays unchanged. Base58 conversion only happens at HTTP/GraphQL boundaries.

## Problem Statement / Motivation

The GRC-20 ecosystem uses Base58-encoded IDs as the canonical string representation. Our API currently returns dashless hex UUIDs (`550e8400e29b41d4a716446655440000`). This forces every API consumer to convert between formats. We're already seeing consumers send Base58 IDs to the API and getting `Invalid UUID` errors (Sentry trace `ae1434810a6340c89edd6ef5d14f88c4` — ~95 error events from a single session).

The API should speak the same ID language as the rest of the GRC-20 ecosystem.

## Proposed Solution

Follow the same pattern as the dashless UUID migration (`9fa22e1` → `5435a7e`):
1. **Accept all three formats on input**: dashed hex, dashless hex, Base58
2. **Return Base58 on output**: all ID fields in GraphQL and REST responses
3. **Keep internal canonical format as dashless hex** (`NormalizedUuid`): minimize internal changes, convert at boundaries only

### Architecture

```
Client (Base58) ──► API boundary: decode to NormalizedUuid (dashless hex)
                        │
                        ▼
                    Internal: NormalizedUuid (unchanged)
                        │
                        ▼
                    DB boundary: toDashedUuid (unchanged)
                        │
                        ▼
                    PostgreSQL: native uuid (unchanged)
                        │
                        ▼
                    DB result: normalizeUuid (unchanged)
                        │
                        ▼
                    API boundary: toBase58(uuid) ──► Client (Base58)
```

### Format Disambiguation

Parse in this order (hex wins when ambiguous for backward compatibility):
1. 36 chars matching `[0-9a-f-]` with dashes in UUID positions → dashed hex
2. 32 chars matching `[0-9a-f]` → dashless hex
3. Otherwise → try Base58 decode, reject if invalid

This is unambiguous in practice: Base58 alphabet excludes `0`, `O`, `I`, `l`, so any string containing `0` (which all hex strings do statistically) is never valid Base58. A 32-char string of only `[1-9a-f]` is theoretically ambiguous but hex parse wins by ordering.

## Technical Considerations

### Internal format stays as dashless hex

The `NormalizedUuid` branded type remains dashless hex internally. Base58 conversion only happens at I/O boundaries (serialize responses, parse inputs). This means:
- No changes to database queries, Drizzle schema, or SQL functions
- No changes to `proposal-diff.ts` internal logic, `idToUuid()`, or `toDashedUuid()`
- The refactor surface is limited to I/O boundaries

### Proposal cursors stay opaque

Proposal list cursors (`{order_value}|{proposal_id}`) keep hex UUIDs internally — the `proposal_id` portion is cast to `::uuid` in SQL, which doesn't understand Base58. Cursors are opaque tokens; clients should not parse them. `parseCursor` will be updated to accept Base58 IDs in the cursor for forward compatibility.

### Search requires a translation layer

OpenSearch stores dashed UUIDs. The search handler currently passes `space_id` and `type_ids` through to OpenSearch as-is. After migration:
- Input: decode Base58 → dashed hex before querying OpenSearch
- Output: convert dashed hex → Base58 in search results

This also fixes the pre-existing inconsistency where search returns dashed UUIDs while all other endpoints return dashless.

### UUID-as-search-query detection

The OpenSearch client detects UUID-pasted-as-search-text via regex (dashed UUID pattern only). This needs to also detect Base58 IDs, decode to dashed UUID, and do a `term` lookup.

### Variable-length Base58

The Rust encoder produces variable-length output (no zero-padding). UUID `00000000-...01` encodes to `2` (1 char). The TypeScript implementation must match this exactly. The nil UUID (`00000000-...00`) encodes to empty string — reject at the boundary since nil UUIDs shouldn't exist in the system.

### `contentId` stays hex

`contentId` in Flag/Unflag proposal actions is raw hex bytes (`encode(content_id, 'hex')`), not a UUID column. It must not be Base58-encoded. Every ID field needs to be audited: UUID columns → Base58, raw bytes → stay hex.

### Response cache

The response cache keys on raw query + variables (before scalar parsing). During transition, the same entity queried as hex vs Base58 produces different cache keys. This is a temporary cache hit rate drop (10s TTL, self-healing). No correctness issue.

### Error messages

Error messages that echo the user's input (e.g., `Proposal '${id}' not found`) should echo Base58, not the internal hex. Preserve the original input format for error reporting.

## Acceptance Criteria

### Functional Requirements

- [ ] GraphQL UUID scalar accepts Base58, dashed hex, and dashless hex on input
- [ ] GraphQL UUID scalar serializes all IDs as Base58 on output
- [ ] REST endpoints accept Base58 IDs in URL params and query strings
- [ ] REST endpoints return Base58 IDs in all response bodies
- [ ] Search accepts Base58 `space_id` and `type_ids` params
- [ ] Search returns Base58 IDs in results (fixing the dashed-hex inconsistency)
- [ ] Search detects Base58 IDs pasted as search queries and does term lookup
- [ ] Proposal list cursors work correctly (hex internally, Base58-containing cursors accepted on input)
- [ ] Profile endpoints accept and return Base58 space IDs
- [ ] Batch profile endpoint accepts Base58 space IDs in request body
- [ ] `contentId` in Flag/Unflag actions remains hex (not Base58)
- [ ] Error messages echo the user's original input format

### Technical Requirements

- [ ] TypeScript Base58 codec produces identical output to Rust `indexer_utils/src/id.rs` (verified with shared test vectors)
- [ ] `NormalizedUuid` branded type unchanged — internal canonical format stays dashless hex
- [ ] All existing tests pass (update expected values from hex to Base58 where needed)
- [ ] Integration tests assert ID fields match Base58 pattern (`^[1-9A-HJ-NP-Za-km-z]+$`)
- [ ] Cross-implementation test vectors cover: nil UUID (rejected), max UUID, leading-zero UUIDs, known roundtrip pairs
- [ ] OpenAPI parameter schemas updated from `format: "uuid"` to Base58 pattern

## Implementation Phases

### Phase 1: Base58 Codec + Accept on Input

Add Base58 support without changing any output format. Zero breaking changes.

**Files:**

| File | Change |
|------|--------|
| `api/src/utils/base58.ts` | **New.** `encodeBase58(uuid: NormalizedUuid): string`, `decodeBase58(base58: string): NormalizedUuid`, ported from `indexer_utils/src/id.rs` |
| `api/src/utils/base58.test.ts` | **New.** Test vectors matching Rust implementation, edge cases |
| `api/src/utils/uuid.ts` | Update `normalizeUuid()` and `isValidUuid()` to accept Base58 input (parse order: dashed hex → dashless hex → Base58) |
| `api/src/kg/uuidScalarPlugin.ts` | Update `parseValue` / `parseLiteral` to accept Base58 |
| `api/src/search/index.ts` | After validation, convert `space_id` / each `type_id` to dashed hex via `toDashedUuid(normalizeUuid(input))` before building the OpenSearch query object (currently passes raw input through) |
| `api/src/services/search/opensearch.ts` | Extend `UUID_PATTERN` to also detect Base58 IDs in search query text; decode to **dashed** hex for the `term` query against OpenSearch |
| `api/src/profile/index.ts` | Update `isValidUuid(spaceId)` calls to accept Base58 |
| `api/src/proposals/router.ts` | Update `isValidUuid` calls for `:id`, `voterId` params to accept Base58 |
| `api/src/proposals/queries.ts` | Update `parseCursor` to accept Base58 in the cursor's ID portion (decode to dashed hex for the `::uuid` cast in SQL) |

**Verification:** All existing consumers continue working (they send hex, get hex back). Consumers sending Base58 stop getting errors.

### Phase 2: Base58 Output

Change all output to Base58. This is the breaking change for consumers expecting hex.

**Files:**

| File | Change |
|------|--------|
| `api/src/kg/uuidScalarPlugin.ts` | Change `serialize` to return Base58 instead of dashless hex |
| `api/src/versioned/queries.ts` | `mapValueRow`, `mapRelationRow`, `mapVersionRow`: wrap UUID fields with `toBase58()`. `editId` in `VersionEntry` also needs conversion. |
| `api/src/versioned/router.ts` | Preserve original input format for error messages (don't echo internal hex when user sent Base58) |
| `api/src/versioned/proposal-diff.ts` | All `normalizeUuid()` calls on response fields (lines 139-140, 254, 398, 407-408) need `toBase58()` wrapping for the `PaginatedProposalDiff` response |
| `api/src/versioned/diff.ts` | System ID constants (lines 28-35) stay as `NormalizedUuid` for internal comparisons; diff output types (`EntityDiff`, `BlockChange`) contain IDs that appear in responses — these need `toBase58()` |
| `api/src/proposals/router.ts` | `computeResponseFields` (line 173-176): convert `proposalId`, `spaceId`, `proposedBy` to Base58. `mapToActionResponse`: convert `targetId`, `voterId` to Base58. `contentId` stays hex. |
| `api/src/proposals/queries.ts` | `extractCursorValue`: cursor `proposal_id` stays hex internally (opaque token). `mapBaseFields` returns raw DB strings — conversion happens in router's `computeResponseFields`. |
| `api/src/profile/index.ts` | Response `spaceId` fields converted to Base58 via `mapProfileRow` |
| `api/src/profile/queries.ts` | `mapProfileRow` returns Base58 `spaceId` |
| `api/src/services/search/opensearch.ts` | Convert result `entity_id`, `space_id`, `type_relations[].entity_to_id` from dashed hex to Base58 |

**Verification:** All UUID ID fields in responses are Base58. `contentId` stays hex. Hex input still accepted. Integration tests updated to assert Base58 pattern.

### Phase 3: Cleanup + Documentation

| File | Change |
|------|--------|
| `api/src/search/index.ts` | `MAX_SPACE_ID_LENGTH` is 36 (dashed UUID) — still larger than Base58's ~22 chars, so it works but is misleading. Rename to `MAX_ID_LENGTH` or lower the value. |
| OpenAPI route descriptions | Update `format: "uuid"` to Base58 pattern in all `describeRoute` calls |
| `api/src/kg/uuidScalarPlugin.ts` | Update scalar description to mention Base58 |
| `api/src/versioned/types.ts` | Document the convention: `NormalizedUuid` is internal (dashless hex), `toBase58()` is applied at serialization boundaries only. No new branded type needed — keep it simple. |
| Integration tests (16 test files) | Assert all UUID ID fields match `^[1-9A-HJ-NP-Za-km-z]+$`. Key files: `uuidScalarPlugin.test.ts`, `router.test.ts`, `integration.test.ts`, `proposal-diff-edit-flow.test.ts`, `queries.test.ts` (proposals), `queries.test.ts` (profile), `search/index.test.ts` |

## Success Metrics

- Zero `Invalid UUID` errors from consumers sending Base58 IDs (currently ~95/trace in Sentry)
- All API responses use Base58 IDs
- No change to database query performance (conversion is sub-microsecond per ID at the boundary)

## Dependencies & Risks

| Risk | Mitigation |
|------|------------|
| **Breaking change for consumers expecting hex output** | Phase 1 is additive (input only). Phase 2 can be feature-flagged with an env var controlling output format. Announce deprecation of hex output before deploying Phase 2. |
| **TypeScript Base58 codec diverges from Rust** | Shared test vectors. Port the exact algorithm from `indexer_utils/src/id.rs`. Test roundtrip: Rust encode → TS decode and TS encode → Rust decode. |
| **Proposal cursor breakage** | Keep hex internally in cursors. `parseCursor` accepts both formats. Old cursors continue working. |
| **Search results change format** | Search already had an inconsistency (dashed vs dashless). This fixes it. Consumers of `/search` need to handle the format change — call out in release notes. |
| **Cache hit rate temporary drop** | 10s TTL, self-healing. No action needed. |

## References & Research

### Internal References

- Dashless UUID migration precedent: `9fa22e1`, `b28e3ed`, `5435a7e`
- Existing Rust Base58 codec: `indexer_utils/src/id.rs`
- UUID utility: `api/src/utils/uuid.ts`
- GraphQL UUID scalar: `api/src/kg/uuidScalarPlugin.ts`
- Search handler: `api/src/search/index.ts`, `api/src/services/search/opensearch.ts`
- Map key mismatch bug from dashless migration: `9770ff0` (PR #356)
- Sentry trace with Base58 ID errors: `ae1434810a6340c89edd6ef5d14f88c4` (issue GAIA-API-D)

### Institutional Learnings Applied

- From dashless UUID migration: additive on input first, then change output. Branded types prevent regressions. Normalize at I/O boundaries only.
- From proposal-id-collision-bug (`docs/issues/`): "users expect the indexed ID to match the ID they provided onchain" — Base58 alignment satisfies this.
- From Map key mismatch bug (`9770ff0`): when DB returns dashed UUIDs and code uses a different format as Map keys, lookups silently fail. Internal canonical format must be consistent.
